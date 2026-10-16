//! Durable pre-Run identity shared by automatic, manual and legacy submissions.
//! This store never starts an engine, acquires credentials, or authorizes tools.
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod commands;
pub(crate) mod live;

const DAY_MS: i64 = 86_400_000;
const RETENTION_MS: i64 = 7 * DAY_MS;

pub(crate) fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    // Upgrade the old closed manual constraint transactionally, preserving guards.
    let old: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_submissions'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(sql) = old.filter(|sql| !sql.contains("'auto'")) {
        let tx = conn.unchecked_transaction()?;
        let objects: Vec<(String, String, String)> = tx.prepare("SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL AND (type='trigger' AND sql LIKE '%agent_submissions%' OR type='index' AND tbl_name='agent_submissions')")?
            .query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        for (kind, name, _) in &objects {
            tx.execute_batch(&format!("DROP {} \"{}\"", kind, name.replace('"', "\"\"")))?;
        }
        tx.execute_batch(
            &sql.replacen("agent_submissions", "agent_submissions_upgrade", 1)
                .replace("'opencode'", "'opencode','auto'"),
        )?;
        tx.execute_batch("INSERT INTO agent_submissions_upgrade SELECT * FROM agent_submissions; DROP TABLE agent_submissions; ALTER TABLE agent_submissions_upgrade RENAME TO agent_submissions;")?;
        for (_, _, sql) in objects {
            tx.execute_batch(&sql)?;
        }
        tx.commit()?;
    }
    // Deliberately no cascading foreign keys: deleting a Thread must not erase
    // a request identity and turn a retransmission into a new execution.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_submissions (
            id TEXT PRIMARY KEY,
            client_request_id TEXT NOT NULL UNIQUE,
            intent_salt TEXT NOT NULL,
            intent_hash TEXT NOT NULL,
            requested_thread_id TEXT,
            thread_id TEXT,
            requested_engine TEXT NOT NULL CHECK(requested_engine IN ('hermes','pi','claude_code','opencode','auto')),
            state TEXT NOT NULL CHECK(state IN ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering','succeeded','failed','cancelled','interrupted')),
            generation INTEGER NOT NULL DEFAULT 0 CHECK(generation >= 0),
            run_id TEXT UNIQUE,
            gone INTEGER NOT NULL DEFAULT 0 CHECK(gone IN (0,1)),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            tombstoned_at INTEGER
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_submissions_active_thread
        ON agent_submissions(thread_id)
        WHERE thread_id IS NOT NULL AND state IN
          ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering');",
    )?;
    let has_cancel: bool = conn
        .prepare("PRAGMA table_info(agent_submissions)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "cancel_requested");
    if !has_cancel {
        conn.execute("ALTER TABLE agent_submissions ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1))", [])?;
    }
    let columns = conn
        .prepare("PRAGMA table_info(agent_submissions)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == "selected_engine") {
        conn.execute(
            "ALTER TABLE agent_submissions ADD COLUMN selected_engine TEXT",
            [],
        )?;
    }
    if !columns.iter().any(|name| name == "failure_reason") {
        conn.execute(
            "ALTER TABLE agent_submissions ADD COLUMN failure_reason TEXT",
            [],
        )?;
    }
    if !columns.iter().any(|name| name == "recovery_json") {
        conn.execute(
            "ALTER TABLE agent_submissions ADD COLUMN recovery_json TEXT",
            [],
        )?;
    }
    // Separate trigger also upgrades installations with existing lifecycle triggers.
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS submission_consume_draft
        AFTER UPDATE OF run_id,gone ON agent_submissions
        WHEN NEW.run_id IS NOT NULL OR NEW.gone=1
        BEGIN UPDATE agent_submissions SET recovery_json=NULL WHERE id=NEW.id; END;",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Accepted,
    Preparing,
    WaitingCredentials,
    Prepared,
    Dispatching,
    Running,
    Recovering,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl State {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    fn pre_dispatch(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Preparing | Self::WaitingCredentials | Self::Prepared
        )
    }

    fn allows(self, next: Self) -> bool {
        use State::*;
        matches!(
            (self, next),
            (Accepted, Preparing | Failed)
                | (Preparing, WaitingCredentials | Prepared | Failed)
                | (WaitingCredentials, Prepared | Failed)
                | (Dispatching, Running | Recovering)
                | (Running, Succeeded | Failed | Cancelled | Recovering)
                | (
                    Recovering,
                    Running | Succeeded | Failed | Cancelled | Interrupted
                )
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Preparing => "preparing",
            Self::WaitingCredentials => "waiting_credentials",
            Self::Prepared => "prepared",
            Self::Dispatching => "dispatching",
            Self::Running => "running",
            Self::Recovering => "recovering",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub id: String,
    pub thread_id: Option<String>,
    pub state: State,
    pub generation: i64,
    pub run_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub gone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    RequestExpired,
    RequestConflict,
    RequestGone,
    ThreadMissing,
    ThreadBusy,
    ResourcesBusy,
    Missing,
    Stale,
    InvalidTransition,
    RequiresReconciliation,
    Storage,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "提交标识或引擎无效",
            Self::RequestExpired => "提交已过期或系统时钟异常，请保留输入后重新发送",
            Self::RequestConflict => "同一提交标识的内容不一致，请重新发送",
            Self::RequestGone => "此提交所属会话已清理，不能重复执行",
            Self::ThreadMissing => "会话不存在",
            Self::ResourcesBusy => "当前任务执行名额已满，请等待已有任务结束或完成恢复",
            Self::ThreadBusy => "此会话正在准备、执行或等待恢复",
            Self::Missing => "提交不存在",
            Self::Stale => "提交状态已变化，请重新获取状态",
            Self::InvalidTransition => "提交不能进入此状态",
            Self::RequiresReconciliation => "请求可能已派发，请先核实运行状态",
            Self::Storage => "无法保存提交状态，请稍后重试",
        })
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}

pub struct Admission {
    pub submission: Submission,
    /// A new identity may already be terminal (automatic admission rejection).
    /// Only a newly created Accepted record may receive a preparation lease.
    pub created: bool,
}

/// `intent` is Host-canonical task input (no credentials), never an untrusted
/// client-provided digest. Explicit engine and Thread are also bound to the hash.
pub struct Request<'a> {
    pub client_request_id: &'a str,
    pub thread_id: Option<&'a str>,
    pub engine: &'a str,
    pub intent: &'a serde_json::Value,
}

/// Shared by ordinary admission and internal child Run claims. Unknown native
/// executions retain their slot until their owning adapter proves termination.
pub(crate) fn occupied_native_slots(conn: &Connection) -> rusqlite::Result<i64> {
    let occupied: i64 = conn.query_row("SELECT (SELECT count(*) FROM agent_submissions WHERE state IN ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering')) + (SELECT count(*) FROM agent_runs r WHERE lower(trim(r.status,'\"')) IN ('queued','running','waiting_approval') AND NOT EXISTS(SELECT 1 FROM agent_submissions s WHERE s.run_id=r.id))", [], |r|r.get(0))?;
    let workflows: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_executions')",
        [],
        |r| r.get(0),
    )?;
    if !workflows {
        return Ok(occupied);
    }
    let unknown: i64 = conn.query_row("SELECT count(*) FROM agent_executions e
        WHERE e.run_id IS NOT NULL AND e.state IN ('dispatching','running','cancelling','recovering')
        AND NOT EXISTS(SELECT 1 FROM agent_submissions s WHERE s.run_id=e.run_id AND s.state IN ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering'))
        AND NOT EXISTS(SELECT 1 FROM agent_runs r WHERE r.id=e.run_id AND lower(trim(r.status,'\"')) IN ('queued','running','waiting_approval'))", [], |r|r.get(0))?;
    Ok(occupied + unknown)
}

pub struct SubmissionStore<'a> {
    conn: &'a mut Connection,
}

impl<'a> SubmissionStore<'a> {
    pub fn new(conn: &'a mut Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, id: &str) -> Result<Submission, Error> {
        read(self.conn, id)
    }

    pub fn admit(&mut self, request: Request<'_>, now_ms: i64) -> Result<Admission, Error> {
        if now_ms < 0
            || (request.engine != "auto"
                && super::registry::EngineId::parse(request.engine).is_err())
            || request
                .thread_id
                .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err(Error::InvalidRequest);
        }
        if let Some(thread) = request.thread_id {
            if super::orchestration::sessions::is_internal(self.conn, thread)? {
                return Err(Error::ThreadMissing);
            }
        }
        let id =
            uuid::Uuid::parse_str(request.client_request_id).map_err(|_| Error::InvalidRequest)?;
        if id.get_version_num() != 7 || id.get_variant() != uuid::Variant::RFC4122 {
            return Err(Error::InvalidRequest);
        }
        let request_id = id.to_string(); // case/format aliases share one identity
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String, String)> = tx.query_row(
            "SELECT id,intent_salt,intent_hash FROM agent_submissions WHERE client_request_id=?1",
            [&request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id, salt, hash)) = existing {
            let submission = read(&tx, &id)?;
            if submission.gone {
                return Err(Error::RequestGone);
            }
            if fingerprint(&request, &salt) != hash {
                return Err(Error::RequestConflict);
            }
            tx.commit()?;
            return Ok(Admission {
                submission,
                created: false,
            });
        }
        let timestamp = id.as_bytes()[..6]
            .iter()
            .fold(0i64, |acc, b| (acc << 8) | i64::from(*b));
        if timestamp < now_ms.saturating_sub(DAY_MS) || timestamp > now_ms.saturating_add(300_000) {
            return Err(Error::RequestExpired);
        }
        let mut rejection = None;
        if let Some(thread) = request.thread_id {
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_threads WHERE id=?1)",
                [thread],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::ThreadMissing);
            }
            let busy: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_submissions WHERE thread_id=?1 AND state IN
                 ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering'))
                 OR EXISTS(SELECT 1 FROM agent_runs WHERE thread_id=?1 AND lower(trim(status,'\"')) IN ('queued','running','waiting_approval'))",
                [thread], |r| r.get(0))?;
            if busy || super::orchestration::sessions::root_busy(&tx, thread)? {
                rejection = Some(Error::ThreadBusy);
            }
        }
        // A waiting/unknown native execution still owns capacity. Legacy Runs are
        // counted separately only when they are not already represented by a submission.
        let occupied = occupied_native_slots(&tx)?;
        if occupied >= 2 && rejection.is_none() {
            rejection = Some(Error::ResourcesBusy);
        }
        if request.engine != "auto" {
            if let Some(error) = rejection {
                return Err(error);
            }
        }
        let recovery = if request.engine == "auto" {
            let serialized =
                serde_json::to_string(request.intent).map_err(|_| Error::InvalidRequest)?;
            if serialized.len() > 4 * 1024 * 1024 {
                return Err(Error::InvalidRequest);
            }
            Some(serialized)
        } else {
            None
        };
        tx.execute("UPDATE agent_submissions SET recovery_json=NULL WHERE run_id IS NULL AND state IN ('failed','cancelled','interrupted') AND updated_at < ?1", [now_ms.saturating_sub(RETENTION_MS)])?;
        let submission_id = format!("submission-{}", uuid::Uuid::new_v4());
        let salt = uuid::Uuid::new_v4().to_string();
        // A bounded valid automatic request rejected for contention still owns a
        // durable identity and draft, but never an execution slot or lease.
        let state = if rejection.is_some() {
            State::Failed
        } else {
            State::Accepted
        };
        tx.execute("INSERT INTO agent_submissions
            (id,client_request_id,intent_salt,intent_hash,requested_thread_id,thread_id,requested_engine,state,created_at,updated_at,recovery_json,failure_reason)
            VALUES (?1,?2,?3,?4,?5,?5,?6,?9,?7,?7,?8,?10)",
            params![submission_id, request_id, salt, fingerprint(&request,&salt), request.thread_id, request.engine, now_ms, recovery, state.as_str(), rejection.map(|error| error.to_string())])?;
        let submission = read(&tx, &submission_id)?;
        tx.commit()?;
        Ok(Admission {
            submission,
            created: true,
        })
    }

    /// generation changes on every transition; a late prepare/ack cannot use an old receipt.
    /// Post-dispatch terminal transitions require native reconciliation by the caller.
    pub fn advance(
        &mut self,
        id: &str,
        generation: i64,
        next: State,
        now_ms: i64,
    ) -> Result<Submission, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = fenced(&tx, id, generation, now_ms)?;
        if !current.state.allows(next) {
            return Err(Error::InvalidTransition);
        }
        update(&tx, id, next, now_ms)?;
        let result = read(&tx, id)?;
        tx.commit()?;
        Ok(result)
    }

    /// Only cancels work proven not dispatched. In-flight cancellation belongs to
    /// the native Run cancellation/reconciliation path, not this shortcut.
    pub fn cancel_preparation(&mut self, id: &str, now_ms: i64) -> Result<Submission, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read(&tx, id)?;
        if current.gone {
            return Err(Error::RequestGone);
        }
        if current.state.terminal() {
            return Ok(current);
        }
        if !current.state.pre_dispatch() {
            return Err(Error::RequiresReconciliation);
        }
        if now_ms < current.updated_at {
            return Err(Error::Stale);
        }
        update(&tx, id, State::Cancelled, now_ms)?;
        let result = read(&tx, id)?;
        tx.commit()?;
        Ok(result)
    }

    /// Call only under an exclusive Host/data-root lock after proving the previous
    /// Host is gone. Never call merely because a window or Channel disconnected.
    pub fn recover_after_host_exit(&mut self, now_ms: i64) -> Result<usize, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count = tx.execute("UPDATE agent_submissions SET
            state=CASE WHEN state IN ('accepted','preparing','waiting_credentials','prepared') THEN 'interrupted' ELSE 'recovering' END,
            generation=generation+1, updated_at=max(updated_at,?1)
            WHERE state IN ('accepted','preparing','waiting_credentials','prepared','dispatching','running')", [now_ms])?;
        tx.commit()?;
        Ok(count)
    }

    pub fn tombstone(&mut self, id: &str, now_ms: i64) -> Result<(), Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read(&tx, id)?;
        if !current.state.terminal() {
            return Err(Error::RequiresReconciliation);
        }
        if now_ms < current.updated_at {
            return Err(Error::Stale);
        }
        tx.execute("UPDATE agent_submissions SET gone=1,thread_id=NULL,requested_thread_id=NULL,run_id=NULL,
            intent_salt='',intent_hash='',generation=generation+1,updated_at=?2,tombstoned_at=?2 WHERE id=?1 AND gone=0", params![id,now_ms])?;
        tx.commit()?;
        Ok(())
    }

    /// Retain identities seven days after removal, and never prune an ID still
    /// inside the new-request acceptance window (including future clock skew).
    pub fn prune_tombstones(&mut self, now_ms: i64) -> Result<usize, Error> {
        Ok(self.conn.execute(
            "DELETE FROM agent_submissions WHERE gone=1 AND tombstoned_at < ?1 AND created_at < ?2",
            params![
                now_ms.saturating_sub(RETENTION_MS),
                now_ms.saturating_sub(DAY_MS + 300_000)
            ],
        )?)
    }
}

/// Called in the same transaction that creates the Run/user message/route audit.
/// Requires an actual matching Run; it does not call a native engine or constitute
/// full PreparedExecution authorization. Configuration/scope leases are still future work.
pub fn bind_run(
    tx: &Transaction<'_>,
    id: &str,
    generation: i64,
    run_id: &str,
    now_ms: i64,
) -> Result<Submission, Error> {
    bind_on_connection(tx, id, generation, run_id, now_ms)
}

fn bind_on_connection(
    tx: &Connection,
    id: &str,
    generation: i64,
    run_id: &str,
    now_ms: i64,
) -> Result<Submission, Error> {
    if tx.is_autocommit() {
        return Err(Error::InvalidTransition);
    }
    let current = fenced(tx, id, generation, now_ms)?;
    if current.state != State::Prepared {
        return Err(Error::InvalidTransition);
    }
    if now_ms.saturating_sub(current.updated_at) > 30_000 {
        return Err(Error::Stale);
    }
    let (thread, engine, status): (String, String, String) = tx
        .query_row(
            "SELECT thread_id,engine,lower(trim(status,'\"')) FROM agent_runs WHERE id=?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(Error::Missing)?;
    if status != "queued" {
        return Err(Error::InvalidTransition);
    }
    let requested: String = tx.query_row(
        "SELECT COALESCE(selected_engine,requested_engine) FROM agent_submissions WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    if current
        .thread_id
        .as_ref()
        .is_some_and(|value| value != &thread)
        || requested != engine
    {
        return Err(Error::RequestConflict);
    }
    if !tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_threads WHERE id=?1)",
        [&thread],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::ThreadMissing);
    }
    let busy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE thread_id=?1 AND id<>?2
         AND lower(trim(status,'\"')) IN ('queued','running','waiting_approval'))
         OR EXISTS(SELECT 1 FROM agent_submissions WHERE thread_id=?1 AND id<>?3
         AND state IN ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering'))",
        params![thread, run_id, id], |r| r.get(0))?;
    if busy {
        return Err(Error::ThreadBusy);
    }
    tx.execute("UPDATE agent_submissions SET thread_id=?2,run_id=?3,state='dispatching',generation=generation+1,updated_at=?4 WHERE id=?1",
        params![id,thread,run_id,now_ms])?;
    read(tx, id)
}

fn fingerprint(request: &Request<'_>, salt: &str) -> String {
    let canonical = serde_json::json!([1, salt, request.thread_id, request.engine, request.intent]);
    format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()))
}

fn read(conn: &Connection, id: &str) -> Result<Submission, Error> {
    conn.query_row("SELECT id,thread_id,state,generation,run_id,created_at,updated_at,gone FROM agent_submissions WHERE id=?1", [id], |r| {
        let state: String = r.get(2)?;
        let state = serde_json::from_value(serde_json::Value::String(state)).map_err(|e|
            rusqlite::Error::FromSqlConversionFailure(2,rusqlite::types::Type::Text,Box::new(e)))?;
        Ok(Submission { id:r.get(0)?, thread_id:r.get(1)?,state,generation:r.get(3)?,run_id:r.get(4)?,created_at:r.get(5)?,updated_at:r.get(6)?,gone:r.get(7)? })
    }).optional()?.ok_or(Error::Missing)
}

fn fenced(conn: &Connection, id: &str, generation: i64, now_ms: i64) -> Result<Submission, Error> {
    let current = read(conn, id)?;
    if current.gone {
        return Err(Error::RequestGone);
    }
    if current.generation != generation || now_ms < current.updated_at {
        return Err(Error::Stale);
    }
    Ok(current)
}

fn update(conn: &Connection, id: &str, state: State, now_ms: i64) -> Result<(), Error> {
    conn.execute(
        "UPDATE agent_submissions SET state=?2,generation=generation+1,updated_at=?3 WHERE id=?1",
        params![id, state.as_str(), now_ms],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
