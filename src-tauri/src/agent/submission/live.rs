//! Legacy-send integration. This is submission ownership, not execution authorization.
use super::{Error, Request, State, SubmissionStore};
use rusqlite::{params, Connection};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Debug, Clone)]
pub(crate) struct Receipt {
    pub(crate) id: String,
    pub(crate) generation: i64,
}

struct HostOwner {
    _lock: File,
}
impl Drop for HostOwner {
    fn drop(&mut self) {
        // Closing our descriptor alone can leave the lock held by a concurrent
        // fork until its exec. Ownership ends here even if that copy still exists.
        let _ = self._lock.unlock();
    }
}
static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Arc<HostOwner>>>> = OnceLock::new();

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn acquire_owner(db: &Path) -> Result<HostOwner, String> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(db.with_extension("agent-host.lock"))
        .map_err(|_| "无法打开智能体宿主锁")?;
    lock.try_lock()
        .map_err(|_| "另一个应用实例正在管理智能体任务，请关闭该实例后重试")?;
    let mut conn = Connection::open(db).map_err(|_| "无法打开提交数据库")?;
    SubmissionStore::new(&mut conn)
        .recover_after_host_exit(now_ms())
        .map_err(|e| e.to_string())?;
    crate::light_response::store::create_schema(&conn).map_err(|_| "无法初始化响应记录")?;
    crate::light_response::store::recover(&conn, now_ms()).map_err(|_| "无法恢复响应记录")?;
    if super::super::orchestration::supervisor::installed(&conn).map_err(|_| "无法读取协作记录")?
    {
        super::super::orchestration::store::WorkflowStore::new(&mut conn)
            .recover_after_exit(now_ms())
            .map_err(|_| "无法恢复协作记录")?;
    }
    reconcile_terminals(&conn).map_err(|_| "无法恢复提交状态")?;
    Ok(HostOwner { _lock: lock })
}

pub(crate) fn ensure_owner(db: &Path) -> Result<(), String> {
    let mut owners = OWNERS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "智能体宿主锁不可用")?;
    if !owners.contains_key(db) {
        owners.insert(db.to_owned(), Arc::new(acquire_owner(db)?));
    }
    Ok(())
}

pub(crate) struct LiveSubmission {
    db: PathBuf,
    pub(crate) receipt: Receipt,
}

impl LiveSubmission {
    pub(crate) fn begin(
        db: &Path,
        request: &super::super::commands::AgentRunStartArgs,
        engine: &str,
    ) -> Result<Self, String> {
        let now = now_ms();
        let mut bytes = *uuid::Uuid::new_v4().as_bytes();
        bytes[..6].copy_from_slice(&now.to_be_bytes()[2..]);
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        let request_id = uuid::Uuid::from_bytes(bytes).to_string();
        Self::admit(db, request, engine, &request_id)?
            .1
            .ok_or_else(|| "提交标识已存在".into())
    }

    pub(crate) fn admit(
        db: &Path,
        request: &super::super::commands::AgentRunStartArgs,
        engine: &str,
        request_id: &str,
    ) -> Result<(super::Submission, Option<Self>), String> {
        let db = db.canonicalize().map_err(|_| "提交数据库不可访问")?;
        ensure_owner(&db)?;
        let now = now_ms();
        let intent = serde_json::to_value(request).map_err(|_| "无法生成提交意图")?;
        let mut conn = Connection::open(&db).map_err(|_| "无法打开提交数据库")?;
        let mut store = SubmissionStore::new(&mut conn);
        let admitted = store
            .admit(
                Request {
                    client_request_id: request_id,
                    thread_id: request.thread_id.as_deref(),
                    engine,
                    intent: &intent,
                },
                now,
            )
            .map_err(|e| e.to_string())?;
        if !admitted.created || admitted.submission.state != State::Accepted {
            return Ok((admitted.submission, None));
        }
        let mut lease = Self {
            db,
            receipt: Receipt {
                id: admitted.submission.id,
                generation: 0,
            },
        };
        let prepared = store
            .advance(&lease.receipt.id, 0, State::Preparing, now)
            .map_err(|e| e.to_string())?;
        lease.receipt.generation = prepared.generation;
        Ok((prepared, Some(lease)))
    }

    pub(crate) fn select_engine(
        &mut self,
        engine: super::super::registry::EngineId,
    ) -> Result<(), String> {
        let conn = Connection::open(&self.db).map_err(|_| "无法打开提交数据库")?;
        let changed = conn.execute("UPDATE agent_submissions SET selected_engine=?3,generation=generation+1 WHERE id=?1 AND generation=?2 AND state='preparing' AND selected_engine IS NULL AND cancel_requested=0", params![self.receipt.id,self.receipt.generation,engine.as_str()]).map_err(|_| "无法记录自动路由")?;
        if changed != 1 {
            return Err("任务已停止或路由状态变化".into());
        }
        self.receipt.generation += 1;
        Ok(())
    }

    pub(crate) fn failed_reason(&self, error: &str) {
        // Store only allowlisted guidance, never native errors, URLs, or keys.
        let reason = if error.contains("已变化") {
            "模型配置或任务范围已变化，请确认后重新发送"
        } else if error.contains("凭据") || error.contains("API Key") || error.contains("系统授权")
        {
            "无法读取模型凭据，请在模型设置中重新保存或完成系统授权"
        } else if error.contains("自动安排已关闭") {
            "自动安排已关闭，请在设置中恢复任务执行配置"
        } else if error.contains("运行时模型配置尚未同步") {
            "运行时模型配置尚未同步，请在设置中重启运行时后重试"
        } else if error.contains("运行时无法确认所选模型") {
            "运行时无法确认所选模型，请检查模型目录与连接验证结果"
        } else if error.contains("模型目录检查超时") {
            "模型目录检查超时，请稍后重试"
        } else if error.contains("原生命令尚未接入宿主权限检查") {
            "当前原生命令或技能尚未接入权限检查，请使用普通任务描述"
        } else if error.contains("没有可用") || error.contains("运行时") {
            "没有可用的任务执行环境，请检查模型配置与设置中的运行时状态"
        } else {
            "提交准备失败，请检查模型配置与任务资料范围"
        };
        if let Ok(conn) = Connection::open(&self.db) {
            let _ = conn.execute("UPDATE agent_submissions SET failure_reason=?2 WHERE id=?1 AND run_id IS NULL AND state='preparing'",params![self.receipt.id,reason]);
        }
        self.failed();
    }

    pub(crate) fn failed(&self) {
        self.settle_unclaimed(State::Failed);
    }

    fn settle_unclaimed(&self, state: State) {
        let result = Connection::open(&self.db).and_then(|conn| conn.execute(
            "UPDATE agent_submissions SET state=?2,generation=generation+1,updated_at=max(updated_at,?3)
             WHERE id=?1 AND run_id IS NULL AND state IN ('accepted','preparing','waiting_credentials','prepared')",
            params![self.receipt.id,state.as_str(),now_ms()]));
        if result.is_err() {
            eprintln!("[submission] 无法收口准备状态，保留恢复门禁");
        }
    }
}
impl Drop for LiveSubmission {
    fn drop(&mut self) {
        self.settle_unclaimed(State::Interrupted);
    }
}

/// Called inside each adapter's write transaction, including paths without a receipt.
pub(crate) fn check_claim(
    conn: &Connection,
    thread: &str,
    receipt: Option<&Receipt>,
) -> Result<(), Error> {
    if conn.is_autocommit() {
        return Err(Error::InvalidTransition);
    }
    if super::super::orchestration::sessions::is_internal(conn, thread)? {
        return Err(Error::ThreadMissing);
    }
    if super::super::orchestration::sessions::root_busy(conn, thread)? {
        return Err(Error::ThreadBusy);
    }
    let other: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_submissions WHERE thread_id=?1
        AND (?2 IS NULL OR id<>?2) AND state IN ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering'))",
        params![thread,receipt.map(|r|r.id.as_str())],|r|r.get(0))?;
    if other {
        return Err(Error::ThreadBusy);
    }
    if let Some(receipt) = receipt {
        let current = super::fenced(conn, &receipt.id, receipt.generation, now_ms())?;
        if now_ms().saturating_sub(current.created_at) > 120_000 {
            return Err(Error::Stale);
        }
        if current.state != State::Preparing
            || current.thread_id.as_deref().is_some_and(|id| id != thread)
        {
            return Err(Error::Stale);
        }
    }
    Ok(())
}

pub(crate) fn bind_claim(
    conn: &Connection,
    receipt: Option<&Receipt>,
    run: &str,
    now: i64,
) -> Result<(), Error> {
    let Some(receipt) = receipt else {
        return Ok(());
    };
    if conn.is_autocommit() {
        return Err(Error::InvalidTransition);
    }
    let current = super::fenced(conn, &receipt.id, receipt.generation, now)?;
    if current.state != State::Preparing {
        return Err(Error::Stale);
    }
    super::update(conn, &receipt.id, State::Prepared, now)?;
    super::bind_on_connection(conn, &receipt.id, receipt.generation + 1, run, now)?;
    Ok(())
}

fn reconcile_terminals(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("UPDATE agent_submissions SET state=(SELECT CASE lower(trim(status,'\"'))
        WHEN 'completed' THEN 'succeeded' WHEN 'failed' THEN 'failed' ELSE 'cancelled' END FROM agent_runs WHERE id=run_id),
        generation=generation+1,updated_at=max(updated_at,?1)
        WHERE gone=0 AND state IN ('dispatching','running','recovering') AND run_id IN
        (SELECT id FROM agent_runs WHERE lower(trim(status,'\"')) IN ('completed','failed','cancelled'))", [now_ms()])
}

/// Installed after the existing Agent tables/columns. Triggers make lifecycle
/// updates atomic even for legacy recovery/deletion paths using direct SQL.
pub(crate) fn install_triggers(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(r#"
        CREATE TRIGGER IF NOT EXISTS submission_run_status AFTER UPDATE OF status ON agent_runs
        WHEN lower(trim(NEW.status,'"')) IN ('completed','failed','cancelled','interrupted')
        BEGIN
          UPDATE agent_submissions SET state=CASE lower(trim(NEW.status,'"'))
            WHEN 'completed' THEN 'succeeded' WHEN 'failed' THEN 'failed'
            WHEN 'cancelled' THEN 'cancelled' ELSE 'recovering' END,
            generation=generation+1,updated_at=max(updated_at,NEW.updated_at)
          WHERE run_id=NEW.id AND gone=0 AND state IN ('dispatching','running','recovering');
        END;
        CREATE TRIGGER IF NOT EXISTS submission_thread_delete_guard BEFORE DELETE ON agent_threads
        WHEN EXISTS(SELECT 1 FROM agent_submissions WHERE thread_id=OLD.id AND state IN
          ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering'))
        BEGIN SELECT RAISE(ABORT,'此会话正在准备、执行或等待恢复，不能删除'); END;
        CREATE TRIGGER IF NOT EXISTS submission_thread_move_guard BEFORE UPDATE OF project_id,archived_at ON agent_threads
        WHEN (NEW.project_id IS NOT OLD.project_id OR NEW.archived_at IS NOT OLD.archived_at)
          AND EXISTS(SELECT 1 FROM agent_submissions WHERE thread_id=OLD.id AND state IN
          ('accepted','preparing','waiting_credentials','prepared','dispatching','running','recovering'))
        BEGIN SELECT RAISE(ABORT,'此会话正在准备、执行或等待恢复，不能移动或归档'); END;
        CREATE TRIGGER IF NOT EXISTS submission_thread_tombstone AFTER DELETE ON agent_threads
        BEGIN
          UPDATE agent_submissions SET gone=1,thread_id=NULL,requested_thread_id=NULL,run_id=NULL,
            intent_salt='',intent_hash='',generation=generation+1,
            updated_at=max(updated_at,CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)),
            tombstoned_at=max(updated_at,CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
          WHERE thread_id=OLD.id AND gone=0;
        END;
    "#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{commands::AgentRunStartArgs, store::RunStore, types::RunStatus};

    struct Fixture {
        dir: PathBuf,
        db: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/submission-live-tests")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("test.db");
            let conn = Connection::open(&db).unwrap();
            crate::db::create_schema(&conn).unwrap();
            RunStore::new(conn)
                .create_thread("t", "fixture", None, now_ms() as u64)
                .unwrap();
            Self { dir, db }
        }
        fn request(&self) -> AgentRunStartArgs {
            serde_json::from_value(
                serde_json::json!({"threadId":"t","engine":"pi","message":"private task"}),
            )
            .unwrap()
        }
        fn state(&self, id: &str) -> State {
            let mut conn = Connection::open(&self.db).unwrap();
            SubmissionStore::new(&mut conn).get(id).unwrap().state
        }
        fn store(&self) -> RunStore {
            RunStore::new(Connection::open(&self.db).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn claim(
        f: &Fixture,
        receipt: &Receipt,
        fail: bool,
    ) -> Result<(), crate::agent::store::RunStoreError> {
        let store = f.store();
        let now = now_ms() as u64;
        store.with_submission_claim("t", "pi", Some(receipt), || {
            store.create_run("r", "t", None, "p", "m", None, 6, now)?;
            store.set_run_engine("r", "pi", "fixture")?;
            store.save_message("m", "t", "r", "user", "private task", now)?;
            store.bind_submission("r", Some(receipt), now)?;
            if fail {
                return Err(crate::agent::store::RunStoreError::Generic(
                    "injected".into(),
                ));
            }
            Ok(())
        })
    }

    #[test]
    fn legacy_prepare_excludes_overlap_and_drop_releases_only_unclaimed() {
        let f = Fixture::new();
        let request = f.request();
        let lease = LiveSubmission::begin(&f.db, &request, "pi").unwrap();
        let id = lease.receipt.id.clone();
        assert_eq!(f.state(&id), State::Preparing);
        assert!(LiveSubmission::begin(&f.db, &request, "pi").is_err());
        assert!(f.store().with_run_claim("t", "pi", || Ok(())).is_err());
        drop(lease);
        assert_eq!(f.state(&id), State::Interrupted);
        let next = LiveSubmission::begin(&f.db, &request, "pi").unwrap();
        next.failed();
        assert_eq!(f.state(&next.receipt.id), State::Failed);
    }

    #[test]
    fn run_message_and_submission_roll_back_then_terminal_follows_run_atomically() {
        let f = Fixture::new();
        let lease = LiveSubmission::begin(&f.db, &f.request(), "pi").unwrap();
        assert!(claim(&f, &lease.receipt, true).is_err());
        assert!(f.store().get_run("r").unwrap().is_none());
        assert!(f.store().get_messages("t").unwrap().is_empty());
        assert_eq!(f.state(&lease.receipt.id), State::Preparing);
        claim(&f, &lease.receipt, false).unwrap();
        let id = lease.receipt.id.clone();
        drop(lease);
        assert_eq!(f.state(&id), State::Dispatching);
        f.store()
            .update_run_status("r", &RunStatus::Completed, now_ms() as u64)
            .unwrap();
        assert_eq!(f.state(&id), State::Succeeded);
        f.store()
            .update_run_status("r", &RunStatus::Failed, now_ms() as u64)
            .unwrap();
        assert_eq!(f.state(&id), State::Succeeded); // late terminal never rewrites submission
    }

    #[test]
    fn cancelled_preparation_cannot_claim_or_persist_user_message() {
        let f = Fixture::new();
        let lease = LiveSubmission::begin(&f.db, &f.request(), "pi").unwrap();
        let mut conn = Connection::open(&f.db).unwrap();
        SubmissionStore::new(&mut conn)
            .cancel_preparation(&lease.receipt.id, now_ms())
            .unwrap();
        assert!(claim(&f, &lease.receipt, false).is_err());
        assert!(f.store().get_run("r").unwrap().is_none());
        assert!(f.store().get_messages("t").unwrap().is_empty());
    }

    #[test]
    fn delete_and_move_guard_preserve_children_and_terminal_delete_tombstones() {
        let f = Fixture::new();
        let lease = LiveSubmission::begin(&f.db, &f.request(), "pi").unwrap();
        claim(&f, &lease.receipt, false).unwrap();
        assert!(f.store().delete_thread("t").is_err());
        assert!(f.store().get_run("r").unwrap().is_some());
        assert_eq!(f.store().get_messages("t").unwrap().len(), 1);
        assert!(f.store().archive_thread("t", now_ms() as u64).is_err());
        let mut conn = Connection::open(&f.db).unwrap();
        assert!(conn
            .execute(
                "UPDATE agent_threads SET project_id='other' WHERE id='t'",
                []
            )
            .is_err());
        f.store()
            .update_run_status("r", &RunStatus::Cancelled, now_ms() as u64)
            .unwrap();
        f.store().delete_thread("t").unwrap();
        assert!(
            SubmissionStore::new(&mut conn)
                .get(&lease.receipt.id)
                .unwrap()
                .gone
        );
    }

    #[test]
    fn another_owner_cannot_recover_active_preparation() {
        let f = Fixture::new();
        let lease = LiveSubmission::begin(&f.db, &f.request(), "pi").unwrap();
        assert!(acquire_owner(&f.db).is_err());
        assert_eq!(f.state(&lease.receipt.id), State::Preparing);
    }

    #[test]
    fn run_interruption_stays_locked_for_native_reconciliation() {
        let f = Fixture::new();
        let lease = LiveSubmission::begin(&f.db, &f.request(), "pi").unwrap();
        claim(&f, &lease.receipt, false).unwrap();
        f.store()
            .update_run_status("r", &RunStatus::Interrupted, now_ms() as u64)
            .unwrap();
        assert_eq!(f.state(&lease.receipt.id), State::Recovering);
        assert!(LiveSubmission::begin(&f.db, &f.request(), "pi").is_err());
        assert!(f.store().delete_thread("t").is_err());
        f.store()
            .update_run_status("r", &RunStatus::Failed, now_ms() as u64)
            .unwrap();
        assert_eq!(f.state(&lease.receipt.id), State::Failed);
    }
    #[test]
    fn releasing_owner_unlocks_even_when_a_spawn_inherits_the_descriptor() {
        let f = Fixture::new();
        let owner = acquire_owner(&f.db).unwrap();
        // A concurrent fork can temporarily retain the open file description
        // before exec closes CLOEXEC descriptors. Model that lifetime explicitly.
        let inherited = owner._lock.try_clone().unwrap();
        drop(owner);
        let next = acquire_owner(&f.db);
        assert!(next.is_ok(), "released owner must not retain its lock");
        drop(inherited);
    }

    #[test]
    fn new_owner_recovers_only_unclaimed_work_and_holds_exclusive_lock() {
        let f = Fixture::new();
        let now = now_ms();
        let stamp = format!("{now:012x}");
        let request_id = format!("{}-{}-7000-8000-000000000001", &stamp[..8], &stamp[8..]);
        let mut conn = Connection::open(&f.db).unwrap();
        let mut submissions = SubmissionStore::new(&mut conn);
        let admitted = submissions
            .admit(
                Request {
                    client_request_id: &request_id,
                    thread_id: Some("t"),
                    engine: "pi",
                    intent: &serde_json::json!({}),
                },
                now,
            )
            .unwrap();
        submissions
            .advance(&admitted.submission.id, 0, State::Preparing, now)
            .unwrap();
        let owner = acquire_owner(&f.db).unwrap();
        assert_eq!(f.state(&admitted.submission.id), State::Interrupted);
        assert!(acquire_owner(&f.db).is_err());
        drop(owner);
        assert!(acquire_owner(&f.db).is_ok());
    }
}

#[cfg(test)]
#[path = "crash_tests.rs"]
mod crash_tests;
