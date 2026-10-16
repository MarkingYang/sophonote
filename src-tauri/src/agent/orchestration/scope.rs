//! Durable, revocable read-only grants. Only Host-owned paths enter this module;
//! a model sees opaque context reference IDs, never this snapshot or DB location.
use crate::agent::pi::tools::Scope;
use rusqlite::{params, Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct ReadLease {
    database: PathBuf,
    id: String,
    generation: i64,
    execution: Option<(String, i64)>,
}

fn now_ms() -> i64 {
    super::super::submission::live::now_ms()
}

pub fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_scope_snapshots (
        id TEXT PRIMARY KEY, scope_generation INTEGER NOT NULL CHECK(scope_generation>=0),
        scope_json TEXT NOT NULL, created_at INTEGER NOT NULL, deadline_ms INTEGER NOT NULL,
        revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0,1))
    );
    CREATE TRIGGER IF NOT EXISTS immutable_agent_scope
    BEFORE UPDATE OF scope_json,created_at,deadline_ms ON agent_scope_snapshots
    BEGIN SELECT RAISE(ABORT,'Scope snapshots are immutable'); END;
    CREATE TRIGGER IF NOT EXISTS no_restore_agent_scope
    BEFORE UPDATE OF revoked,scope_generation ON agent_scope_snapshots
    WHEN NEW.revoked<OLD.revoked OR NEW.scope_generation<OLD.scope_generation
    BEGIN SELECT RAISE(ABORT,'Scope grants cannot be restored'); END;",
    )?;
    super::evidence::create_schema(conn)
}

/// Use the workflow's claim transaction to insert this snapshot alongside its
/// owner. An opaque ID alone never creates permission: concrete reads recheck it.
pub fn persist_readonly(
    conn: &Connection,
    parent: &Scope,
    paths: &[PathBuf],
    web: bool,
    generation: i64,
    deadline_ms: i64,
    now: i64,
) -> Result<String, String> {
    if generation < 0
        || now < 0
        || deadline_ms <= now
        || deadline_ms.saturating_sub(now) > super::contracts::WORKFLOW_MS
    {
        return Err("只读授权代次或有效期无效".into());
    }
    // Single layer: do not detach an existing child's revocation chain by copying
    // its paths into a new independent lease.
    if parent.read_lease.is_some() {
        return Err("子任务不能再次派生授权".into());
    }
    let scope = parent.restrict_readonly(paths, web)?;
    let encoded = serde_json::to_string(&scope).map_err(|_| "无法保存只读范围")?;
    if encoded.len() > 64 * 1024 {
        return Err("只读范围超过限制".into());
    }
    let id = format!("scope-{}", uuid::Uuid::new_v4());
    conn.execute("INSERT INTO agent_scope_snapshots(id,scope_generation,scope_json,created_at,deadline_ms) VALUES (?1,?2,?3,?4,?5)",params![id,generation,encoded,now,deadline_ms]).map_err(|_| "无法保存只读授权")?;
    Ok(id)
}

impl ReadLease {
    pub fn verify_read(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        self.validate()?;
        let conn = Connection::open_with_flags(&self.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "只读授权存储不可用")?;
        super::evidence::verify_read(&conn, &self.id, path, bytes, now_ms())
    }
    pub fn validate(&self) -> Result<(), String> {
        let conn = Connection::open_with_flags(&self.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "只读授权存储不可用")?;
        let now = now_ms();
        let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_scope_snapshots WHERE id=?1 AND scope_generation=?2 AND revoked=0 AND created_at<=?3 AND deadline_ms>?3)",params![self.id,self.generation,now],|row|row.get(0)).map_err(|_| "无法核对只读授权")?;
        if !valid {
            return Err("子任务授权已撤销、到期或发生变化".into());
        }
        if let Some((id, generation)) = &self.execution {
            let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_executions e JOIN agent_workflows w ON w.id=e.workflow_id WHERE e.id=?1 AND e.generation=?2 AND e.state IN ('dispatching','running') AND e.deadline_ms>?3 AND w.state='running' AND w.deadline_ms>?3 AND json_extract(w.policy_json,'$.scopeSnapshotId')=?4)",params![id,generation,now,self.id],|r|r.get(0)).map_err(|_| "无法核对子任务身份")?;
            if !active {
                return Err("子任务已停止或执行身份已失效".into());
            }
        }
        Ok(())
    }
}

pub fn restore(database: &Path, id: &str, generation: i64) -> Result<Scope, String> {
    let database = database.canonicalize().map_err(|_| "只读授权存储不可用")?;
    let conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "只读授权存储不可用")?;
    let encoded: String = conn
        .query_row(
            "SELECT scope_json FROM agent_scope_snapshots WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|_| "只读授权不存在")?;
    if encoded.len() > 64 * 1024 {
        return Err("只读授权数据无效".into());
    }
    let mut scope: Scope = serde_json::from_str(&encoded).map_err(|_| "只读授权数据无效")?;
    if scope.mode != "plan" || scope.readonly.is_none() {
        return Err("授权不是只读子任务范围".into());
    }
    let lease = ReadLease {
        database,
        id: id.into(),
        generation,
        execution: None,
    };
    lease.validate()?;
    scope.read_lease = Some(lease);
    Ok(scope)
}

pub fn restore_for_execution(
    database: &Path,
    id: &str,
    generation: i64,
    execution: &str,
    attempt_generation: i64,
    paths: &[PathBuf],
) -> Result<Scope, String> {
    let mut scope = restore(database, id, generation)?.restrict_readonly(paths, false)?;
    let lease = scope.read_lease.as_mut().ok_or("子任务缺少持久授权")?;
    lease.execution = Some((execution.into(), attempt_generation));
    lease.validate()?;
    Ok(scope)
}

pub fn revoke(conn: &Connection, id: &str) -> Result<bool, String> {
    conn.execute("UPDATE agent_scope_snapshots SET revoked=1,scope_generation=scope_generation+1 WHERE id=?1 AND revoked=0",[id])
        .map(|changed|changed==1).map_err(|_| "无法撤销子任务授权".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::pi::tools::{execute, Request};
    use tokio_util::sync::CancellationToken;
    #[tokio::test]
    async fn snapshot_survives_reopen_and_revocation_reaches_already_loaded_tools() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("scope-snapshot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let database = root.join("state.db");
        let conn = Connection::open(&database).unwrap();
        create_schema(&conn).unwrap();
        create_schema(&conn).unwrap();
        let file = root.join("evidence.txt");
        std::fs::write(&file, "approved evidence").unwrap();
        let parent = Scope {
            read_lease: None,
            readonly: None,
            workspace: root.clone(),
            read_paths: vec![],
            workcopy: None,
            protected: vec![database.clone()],
            mode: "autoEdit".into(),
        };
        let now = now_ms();
        let id = persist_readonly(
            &conn,
            &parent,
            std::slice::from_ref(&file),
            false,
            7,
            now + 60_000,
            now,
        )
        .unwrap();
        let evidence =
            super::super::evidence::FrozenEvidence::capture_file(&parent, "ref-1", &file).unwrap();
        super::super::evidence::persist(&conn, &id, &evidence, now).unwrap();
        assert!(restore(&database, &id, 6).is_err());
        let child = restore(&database, &id, 7).unwrap();
        let read = Request {
            name: "read".into(),
            args: serde_json::json!({"path":file}),
        };
        assert!(execute(&child, &read, &CancellationToken::new())
            .await
            .unwrap()
            .contains("approved evidence"));
        assert!(persist_readonly(&conn, &child, &[file], false, 7, now + 60_000, now).is_err());
        assert!(conn
            .execute(
                "UPDATE agent_scope_snapshots SET scope_json='{}' WHERE id=?1",
                [&id]
            )
            .is_err());
        assert!(revoke(&conn, &id).unwrap());
        assert!(!revoke(&conn, &id).unwrap());
        assert!(execute(&child, &read, &CancellationToken::new())
            .await
            .is_err());
        assert!(restore(&database, &id, 7).is_err());
        assert!(conn
            .execute(
                "UPDATE agent_scope_snapshots SET revoked=0 WHERE id=?1",
                [&id]
            )
            .is_err());
        drop(conn);
        std::fs::remove_dir_all(root).unwrap();
    }
}
