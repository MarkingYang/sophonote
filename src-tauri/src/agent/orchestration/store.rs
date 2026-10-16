//! Durable scheduling intent. No method in this store dispatches a process,
//! allocates a native Session, or grants file access.
use super::contracts::{
    Capability, ChildResult, ContextRef, Delegation, ResultStatus, TaskPackage, MAX_CHILDREN,
    WORKFLOW_MS,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub enum Error {
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    Invalid(String),
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "协作存储失败：{error}"),
            Self::Json(_) => write!(f, "协作数据格式无效"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

pub fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    super::scope::create_schema(conn)?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_workflows (
        id TEXT PRIMARY KEY, root_thread_id TEXT NOT NULL, root_run_id TEXT NOT NULL UNIQUE,
        client_request_id TEXT NOT NULL UNIQUE, policy_json TEXT NOT NULL,
        state TEXT NOT NULL CHECK(state IN ('running','cancelling','recovering','succeeded','partial','failed','cancelled','interrupted')),
        generation INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        deadline_ms INTEGER NOT NULL, final_result_ref TEXT
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_root_active ON agent_workflows(root_thread_id)
      WHERE state IN ('running','cancelling','recovering');
    CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_scope ON agent_workflows(json_extract(policy_json,'$.scopeSnapshotId'));
    CREATE TABLE IF NOT EXISTS agent_executions (
        id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL, parent_execution_id TEXT,
        parent_run_id TEXT, tool_call_id TEXT, task_json TEXT, run_id TEXT UNIQUE,
        state TEXT NOT NULL CHECK(state IN ('queued','dispatching','running','cancelling','recovering','succeeded','partial','failed','cancelled','interrupted')),
        generation INTEGER NOT NULL DEFAULT 0, attempt INTEGER NOT NULL DEFAULT 0,
        deadline_ms INTEGER NOT NULL, result_json TEXT,
        UNIQUE(parent_run_id, tool_call_id)
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_execution_single_child ON agent_executions(workflow_id)
      WHERE parent_execution_id IS NOT NULL AND state IN ('dispatching','running','cancelling','recovering');
    CREATE TABLE IF NOT EXISTS agent_workflow_events (
        workflow_id TEXT NOT NULL, seq INTEGER NOT NULL, execution_id TEXT,
        kind TEXT NOT NULL, generation INTEGER NOT NULL, timestamp INTEGER NOT NULL,
        PRIMARY KEY(workflow_id,seq)
    );")
}

/// Host-generated context registry. This is a frozen scheduling contract, not a
/// tool lease: concrete path/version and current authorization checks are required
/// by the execution adapter even after a node is admitted here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub context: BTreeMap<String, ContextRef>,
    pub capabilities: BTreeSet<Capability>,
    pub scope_snapshot_id: String,
    pub scope_generation: i64,
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub root_thread_id: String,
    pub root_run_id: String,
    pub state: String,
    pub generation: i64,
    pub deadline_ms: i64,
    pub final_result_ref: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub id: String,
    pub workflow_id: String,
    pub parent_execution_id: Option<String>,
    pub state: String,
    pub generation: i64,
    pub attempt: i64,
    pub deadline_ms: i64,
    pub package: Option<TaskPackage>,
}

pub struct WorkflowStore<'a> {
    conn: &'a mut Connection,
}
impl<'a> WorkflowStore<'a> {
    pub fn new(conn: &'a mut Connection) -> Self {
        Self { conn }
    }

    pub fn get(&self, id: &str) -> Result<Workflow> {
        workflow(self.conn, id)?.ok_or_else(|| invalid("协作任务不存在"))
    }

    pub fn begin(
        &mut self,
        root_run_id: &str,
        client_request_id: &str,
        policy: &Policy,
        now: i64,
    ) -> Result<Workflow> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = Self::begin_in_claim(&tx, root_run_id, client_request_id, policy, now)?;
        tx.commit()?;
        Ok(result)
    }

    pub(crate) fn begin_in_claim(
        conn: &Connection,
        root_run_id: &str,
        client_request_id: &str,
        policy: &Policy,
        now: i64,
    ) -> Result<Workflow> {
        if conn.is_autocommit() {
            return Err(invalid("协作创建必须绑定发送事务"));
        }
        if now < 0
            || policy.scope_generation < 0
            || policy.scope_snapshot_id.is_empty()
            || policy.provider_id.is_empty()
            || policy.model_id.is_empty()
            || client_request_id.is_empty()
            || policy.context.len() > 20
        {
            return Err(invalid("协作身份、模型或范围无效"));
        }
        let encoded = serde_json::to_string(policy)?;
        if encoded.len() > 32 * 1024 {
            return Err(invalid("协作范围超过限制"));
        }
        let existing: Option<(String, String, String)> = conn
            .query_row(
                "SELECT id,root_run_id,policy_json FROM agent_workflows WHERE client_request_id=?1",
                [client_request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((id, run, saved)) = existing {
            if run != root_run_id || saved != encoded {
                return Err(invalid("协作请求身份冲突"));
            }
            return workflow(conn, &id)?.ok_or_else(|| invalid("协作记录缺失"));
        }
        check_scope(conn, policy, now)?;
        let thread: Option<String> = conn.query_row("SELECT thread_id FROM agent_runs WHERE id=?1 AND json_extract(status,'$') IN ('queued','running','waiting_approval')", [root_run_id], |row| row.get(0)).optional()?;
        let thread = thread.ok_or_else(|| invalid("父运行不可委托"))?;
        super::sessions::require_user_thread(conn, &thread).map_err(Error::Invalid)?;
        let id = format!("workflow-{}", uuid::Uuid::new_v4());
        let root = format!("execution-{}", uuid::Uuid::new_v4());
        let deadline = now
            .checked_add(WORKFLOW_MS)
            .ok_or_else(|| invalid("协作时钟溢出"))?
            .min(conn.query_row(
                "SELECT deadline_ms FROM agent_scope_snapshots WHERE id=?1",
                [&policy.scope_snapshot_id],
                |row| row.get::<_, i64>(0),
            )?);
        conn.execute("INSERT INTO agent_workflows(id,root_thread_id,root_run_id,client_request_id,policy_json,state,created_at,updated_at,deadline_ms) VALUES (?1,?2,?3,?4,?5,'running',?6,?6,?7)", params![id,thread,root_run_id,client_request_id,encoded,now,deadline])?;
        conn.execute("INSERT INTO agent_executions(id,workflow_id,run_id,state,deadline_ms) VALUES (?1,?2,?3,'running',?4)", params![root,id,root_run_id,deadline])?;
        event(conn, &id, Some(&root), "workflow_started", 0, now)?;
        let result = workflow(conn, &id)?.unwrap();
        Ok(result)
    }

    /// Parent identity comes from the authenticated tool channel, never tool JSON.
    pub fn delegate(
        &mut self,
        parent_run: &str,
        tool_call: &str,
        request: &Delegation,
        now: i64,
    ) -> Result<Execution> {
        if tool_call.is_empty() || tool_call.len() > 256 {
            return Err(invalid("委托工具调用身份无效"));
        }
        let request_json = serde_json::to_value(request)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM agent_executions WHERE parent_run_id=?1 AND tool_call_id=?2",
                params![parent_run, tool_call],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            let result = execution(&tx, &id)?;
            if serde_json::to_value(
                &result
                    .package
                    .as_ref()
                    .ok_or_else(|| invalid("子任务缺少任务包"))?
                    .delegation,
            )? != request_json
            {
                return Err(invalid("重复委托的参数不一致"));
            }
            return Ok(result);
        }
        let parent: Option<(String,String,String, i64, i64, i64)> = tx.query_row(
            "SELECT e.id,w.id,w.policy_json,w.deadline_ms,w.created_at,w.updated_at FROM agent_executions e JOIN agent_workflows w ON w.id=e.workflow_id JOIN agent_runs r ON r.id=e.run_id WHERE e.run_id=?1 AND e.parent_execution_id IS NULL AND e.state='running' AND w.state='running' AND json_extract(r.status,'$') IN ('queued','running','waiting_approval')",
            [parent_run], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
        let (parent, id, policy, deadline, created, updated) =
            parent.ok_or_else(|| invalid("父节点已停止或不允许再次委托"))?;
        if now < created || now < updated {
            return Err(invalid("时钟回退，已暂停协作派发"));
        }
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM agent_executions WHERE workflow_id=?1 AND parent_execution_id IS NOT NULL", [&id], |row| row.get(0))?;
        if count >= MAX_CHILDREN as i64 {
            return Err(invalid("本轮最多委托四个子任务"));
        }
        let policy: Policy = serde_json::from_str(&policy)?;
        check_scope(&tx, &policy, now)?;
        let package = request
            .authorize(0, &policy.context, &policy.capabilities, deadline, now)
            .map_err(Error::Invalid)?;
        let child = format!("execution-{}", uuid::Uuid::new_v4());
        tx.execute("INSERT INTO agent_executions(id,workflow_id,parent_execution_id,parent_run_id,tool_call_id,task_json,state,deadline_ms) VALUES (?1,?2,?3,?4,?5,?6,'queued',?7)",params![child,id,parent,parent_run,tool_call,serde_json::to_string(&package)?,package.deadline_ms])?;
        tx.execute(
            "UPDATE agent_workflows SET updated_at=?2 WHERE id=?1",
            params![id, now],
        )?;
        event(&tx, &id, Some(&child), "execution_queued", 0, now)?;
        let has_sessions: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='agent_internal_threads' AND type='table')", [], |r|r.get(0))?;
        if has_sessions {
            super::sessions::create_internal(&tx, &child, now).map_err(Error::Invalid)?;
        }
        let result = execution(&tx, &child)?;
        tx.commit()?;
        Ok(result)
    }

    /// Reserve scheduling intent only. The caller must atomically bind an internal
    /// Thread/Run and a global native lease before any actual dispatch.
    pub fn reserve(&mut self, id: &str, generation: i64, now: i64) -> Result<Execution> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let node = execution(&tx, id)?;
        check_workflow_scope(&tx, &node.workflow_id, now)?;
        let changed = tx.execute("UPDATE agent_executions SET state='dispatching',generation=generation+1,attempt=attempt+1 WHERE id=?1 AND generation=?2 AND state='queued' AND parent_execution_id IS NOT NULL AND deadline_ms>?3 AND EXISTS(SELECT 1 FROM agent_workflows w JOIN agent_runs r ON r.id=w.root_run_id WHERE w.id=agent_executions.workflow_id AND w.state='running' AND w.deadline_ms>?3 AND w.updated_at<=?3 AND json_extract(r.status,'$') IN ('queued','running','waiting_approval'))",params![id,generation,now])?;
        if changed != 1 {
            return Err(invalid("节点已取消、到期或状态发生变化"));
        }
        tx.execute(
            "UPDATE agent_workflows SET updated_at=?2 WHERE id=?1",
            params![node.workflow_id, now],
        )?;
        let result = execution(&tx, id)?;
        event(
            &tx,
            &result.workflow_id,
            Some(id),
            "execution_reserved",
            result.generation,
            now,
        )?;
        tx.commit()?;
        Ok(result)
    }

    /// All roots and children consume the same durable native capacity. No
    /// process may start before this transaction returns its unique Run identity.
    pub(crate) fn claim_child(
        &mut self,
        id: &str,
        generation: i64,
        engine: crate::agent::registry::EngineId,
        engine_version: &str,
        now: i64,
    ) -> Result<(Execution, String, String)> {
        if engine_version.is_empty() || engine_version.len() > 128 {
            return Err(invalid("子执行器版本证据缺失"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let node = execution(&tx, id)?;
        check_workflow_scope(&tx, &node.workflow_id, now)?;
        let occupied = crate::agent::submission::occupied_native_slots(&tx)?;
        if occupied >= 2 {
            return Err(invalid("执行名额已满，父任务等待时仍占用名额"));
        }
        let (thread, project, provider, model): (String,Option<String>,String,String) = tx.query_row(
            "SELECT i.thread_id,t.project_id,json_extract(w.policy_json,'$.providerId'),json_extract(w.policy_json,'$.modelId')
             FROM agent_internal_threads i JOIN agent_threads t ON t.id=i.thread_id
             JOIN agent_executions e ON e.id=i.execution_id JOIN agent_workflows w ON w.id=e.workflow_id
             WHERE e.id=?1 AND t.latest_run_id IS NULL AND t.external_session_id IS NULL",
             [id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let run = format!("run-{}", uuid::Uuid::new_v4());
        let changed=tx.execute("UPDATE agent_executions SET state='dispatching',generation=generation+1,attempt=attempt+1,run_id=?4
            WHERE id=?1 AND generation=?2 AND state='queued' AND parent_execution_id IS NOT NULL AND deadline_ms>?3
            AND EXISTS(SELECT 1 FROM agent_workflows w JOIN agent_runs r ON r.id=w.root_run_id
                       WHERE w.id=agent_executions.workflow_id AND w.state='running' AND w.deadline_ms>?3 AND w.updated_at<=?3
                       AND lower(trim(r.status,'\"')) IN ('queued','running','waiting_approval'))",params![id,generation,now,run])?;
        if changed != 1 {
            return Err(invalid("子任务已停止、到期或派发状态发生变化"));
        }
        tx.execute("INSERT INTO agent_runs(id,thread_id,project_id,status,provider,model,prompt_version,max_model_calls,current_model_calls,engine,engine_version,created_at,updated_at)
            VALUES(?1,?2,?3,'\"queued\"',?4,?5,'readonly-collaboration-v1',8,0,?6,?7,?8,?8)",params![run,thread,project,provider,model,engine.as_str(),engine_version,now])?;
        tx.execute("UPDATE agent_threads SET engine=?2,latest_run_id=?3,status='\"running\"',updated_at=?4 WHERE id=?1",params![thread,engine.as_str(),run,now])?;
        tx.execute(
            "UPDATE agent_workflows SET updated_at=?2 WHERE id=?1",
            params![node.workflow_id, now],
        )?;
        let result = execution(&tx, id)?;
        event(
            &tx,
            &node.workflow_id,
            Some(id),
            "execution_claimed",
            result.generation,
            now,
        )?;
        tx.commit()?;
        Ok((result, thread, run))
    }

    /// Native acknowledgement and its durable Run must agree before results can
    /// be accepted. This method does not itself start or retry an executor.
    pub fn acknowledge_child(
        &mut self,
        id: &str,
        generation: i64,
        run: &str,
        now: i64,
    ) -> Result<Execution> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let node = execution(&tx, id)?;
        check_workflow_scope(&tx, &node.workflow_id, now)?;
        let changed=tx.execute("UPDATE agent_executions SET state='running'
            WHERE id=?1 AND generation=?2 AND run_id=?3 AND state='dispatching' AND deadline_ms>?4
            AND EXISTS(SELECT 1 FROM agent_runs WHERE id=?3 AND lower(trim(status,'\"'))='running')
            AND EXISTS(SELECT 1 FROM agent_workflows WHERE id=agent_executions.workflow_id AND state='running' AND deadline_ms>?4 AND updated_at<=?4)",params![id,generation,run,now])?;
        if changed != 1 {
            return Err(invalid("子运行确认已失效或身份不一致"));
        }
        event(
            &tx,
            &node.workflow_id,
            Some(id),
            "execution_started",
            generation,
            now,
        )?;
        let result = execution(&tx, id)?;
        tx.commit()?;
        Ok(result)
    }

    /// Receive only the exact admitted attempt; cancellation and recovery revoke it.
    pub fn complete_child(
        &mut self,
        parent_run: &str,
        id: &str,
        generation: i64,
        status: ResultStatus,
        result_json: &str,
        now: i64,
    ) -> Result<Execution> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let node = execution(&tx, id)?;
        check_workflow_scope(&tx, &node.workflow_id, now)?;
        let package = node
            .package
            .as_ref()
            .ok_or_else(|| invalid("只能接收子节点结果"))?;
        let result = ChildResult::parse(result_json, package).map_err(Error::Invalid)?;
        if status == ResultStatus::Succeeded && !result.unresolved.is_empty() {
            return Err(invalid("仍有未解决项的子任务不能标记全部成功"));
        }
        let status = match status {
            ResultStatus::Succeeded => "succeeded",
            ResultStatus::Partial => "partial",
            ResultStatus::Failed => "failed",
            ResultStatus::Cancelled => "cancelled",
        };
        let native: Option<(String, Option<String>)> = tx.query_row(
            "SELECT e.run_id,lower(trim(r.status,'\"')) FROM agent_executions e LEFT JOIN agent_runs r ON r.id=e.run_id WHERE e.id=?1 AND e.run_id IS NOT NULL", [id], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((_, native_status)) = native {
            let required = match status {
                "succeeded" | "partial" => "completed",
                other => other,
            };
            if native_status.as_deref() != Some(required) {
                return Err(invalid("原生运行尚未确认对应终态，不能接收子任务结果"));
            }
        }
        let changed = tx.execute("UPDATE agent_executions SET state=?4,result_json=?5,generation=generation+1 WHERE id=?1 AND parent_run_id=?2 AND generation=?3 AND state='running' AND deadline_ms>?6 AND EXISTS(SELECT 1 FROM agent_workflows w WHERE w.id=agent_executions.workflow_id AND w.state='running' AND w.deadline_ms>?6 AND w.updated_at<=?6)",params![id,parent_run,generation,status,serde_json::to_string(&result)?,now])?;
        if changed != 1 {
            return Err(invalid("结果已过期、已取消或不属于当前执行尝试"));
        }
        tx.execute(
            "UPDATE agent_workflows SET updated_at=?2 WHERE id=?1",
            params![node.workflow_id, now],
        )?;
        let result = execution(&tx, id)?;
        event(
            &tx,
            &result.workflow_id,
            Some(id),
            "execution_completed",
            result.generation,
            now,
        )?;
        tx.commit()?;
        Ok(result)
    }

    /// Finalize only after the authoritative root Run and every child have ended.
    /// This does not infer native completion from silence or missing handles.
    pub fn finish_root(
        &mut self,
        root_run: &str,
        final_ref: Option<&str>,
        now: i64,
    ) -> Result<Workflow> {
        if final_ref.is_some_and(|value| value.is_empty() || value.len() > 256) {
            return Err(invalid("最终成果引用无效"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id: String = tx.query_row(
            "SELECT id FROM agent_workflows WHERE root_run_id=?1",
            [root_run],
            |row| row.get(0),
        )?;
        let current = workflow(&tx, &id)?.ok_or_else(|| invalid("协作任务不存在"))?;
        if current.state != "running" {
            return Err(invalid("协作任务已停止或等待核实，拒绝旧收尾回执"));
        }
        check_workflow_scope(&tx, &id, now)?;
        let (updated, status): (i64, String) = tx.query_row(
            "SELECT w.updated_at,json_extract(r.status,'$') FROM agent_workflows w JOIN agent_runs r ON r.id=w.root_run_id WHERE w.id=?1", [&id], |row| Ok((row.get(0)?,row.get(1)?)),
        )?;
        if now < updated || now >= current.deadline_ms {
            return Err(invalid("收尾时间无效或本轮预算已到期"));
        }
        let active: i64 = tx.query_row("SELECT COUNT(*) FROM agent_executions WHERE workflow_id=?1 AND parent_execution_id IS NOT NULL AND state IN ('queued','dispatching','running','cancelling','recovering')", [&id], |row|row.get(0))?;
        if active != 0 {
            return Err(invalid("仍有子任务未确认结束，不能释放根任务"));
        }
        let incomplete: i64 = tx.query_row("SELECT COUNT(*) FROM agent_executions WHERE workflow_id=?1 AND parent_execution_id IS NOT NULL AND state!='succeeded'", [&id], |row|row.get(0))?;
        let root_state = match status.as_str() {
            "completed" if final_ref.is_some() => "succeeded",
            "failed" => "failed",
            "cancelled" => "cancelled",
            "interrupted" => "interrupted",
            _ => return Err(invalid("根运行尚未结束或缺少最终成果引用")),
        };
        let state = if root_state == "succeeded" && incomplete > 0 {
            "partial"
        } else {
            root_state
        };
        tx.execute("UPDATE agent_executions SET state=?2,generation=generation+1 WHERE workflow_id=?1 AND parent_execution_id IS NULL AND state='running'",params![id,root_state])?;
        tx.execute("UPDATE agent_workflows SET state=?2,generation=generation+1,updated_at=?3,final_result_ref=?4 WHERE id=?1",params![id,state,now,final_ref])?;
        revoke_workflow_scope(&tx, &id)?;
        let result = workflow(&tx, &id)?.unwrap();
        event(&tx, &id, None, "workflow_completed", result.generation, now)?;
        tx.commit()?;
        Ok(result)
    }

    /// Expiry revokes permission; it never asserts that a native process stopped.
    /// The supervisor must cancel/reconcile the returned workflows afterwards.
    pub fn expire(&mut self, now: i64) -> Result<Vec<Workflow>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = tx.prepare("SELECT w.id FROM agent_workflows w JOIN agent_scope_snapshots s ON s.id=json_extract(w.policy_json,'$.scopeSnapshotId') WHERE w.state='running' AND (w.deadline_ms<=?1 OR s.deadline_ms<=?1 OR s.revoked=1 OR s.scope_generation!=json_extract(w.policy_json,'$.scopeGeneration') OR EXISTS(SELECT 1 FROM agent_executions e WHERE e.workflow_id=w.id AND e.state IN ('queued','dispatching','running') AND e.deadline_ms<=?1))")?
            .query_map([now], |row| row.get::<_,String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut results = Vec::new();
        for id in ids {
            revoke_workflow_scope(&tx, &id)?;
            tx.execute("UPDATE agent_workflows SET state='cancelling',generation=generation+1,updated_at=MAX(updated_at,?2) WHERE id=?1",params![id,now])?;
            tx.execute("UPDATE agent_executions SET state=CASE WHEN state='queued' THEN 'cancelled' ELSE 'cancelling' END,generation=generation+1 WHERE workflow_id=?1 AND state IN ('queued','dispatching','running')",[&id])?;
            let result = workflow(&tx, &id)?.unwrap();
            event(
                &tx,
                &id,
                None,
                "workflow_scope_expired",
                result.generation,
                now,
            )?;
            results.push(result);
        }
        tx.commit()?;
        Ok(results)
    }

    /// Only the owning adapter may call after proving its native process/session
    /// stopped. Missing local handles do not constitute that proof.
    pub fn confirm_stopped(&mut self, id: &str, generation: i64, now: i64) -> Result<Execution> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE agent_executions SET state=CASE WHEN state='cancelling' OR EXISTS(SELECT 1 FROM agent_runs WHERE id=agent_executions.run_id AND lower(trim(status,'\"'))='cancelled') THEN 'cancelled' ELSE 'interrupted' END,generation=generation+1 WHERE id=?1 AND generation=?2 AND state IN ('cancelling','recovering')",params![id,generation])?;
        if changed != 1 {
            return Err(invalid("停止回执不属于当前执行尝试"));
        }
        let result = execution(&tx, id)?;
        event(
            &tx,
            &result.workflow_id,
            Some(id),
            "execution_stopped",
            result.generation,
            now,
        )?;
        let active: i64 = tx.query_row("SELECT COUNT(*) FROM agent_executions WHERE workflow_id=?1 AND state IN ('queued','dispatching','running','cancelling','recovering')",[&result.workflow_id],|row|row.get(0))?;
        if active == 0 {
            tx.execute("UPDATE agent_workflows SET state=CASE WHEN state='cancelling' OR EXISTS(SELECT 1 FROM agent_runs WHERE id=agent_workflows.root_run_id AND lower(trim(status,'\"'))='cancelled') THEN 'cancelled' ELSE 'interrupted' END,generation=generation+1,updated_at=MAX(updated_at,?2) WHERE id=?1 AND state IN ('cancelling','recovering')",params![result.workflow_id,now])?;
            let workflow = workflow(&tx, &result.workflow_id)?.unwrap();
            event(
                &tx,
                &workflow.id,
                None,
                "workflow_stopped",
                workflow.generation,
                now,
            )?;
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn cancel(&mut self, id: &str, now: i64) -> Result<Workflow> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE agent_workflows SET state='cancelling',generation=generation+1,updated_at=MAX(updated_at,?2) WHERE id=?1 AND state IN ('running','recovering')",params![id,now])?;
        if changed != 0 {
            revoke_workflow_scope(&tx, id)?;
            tx.execute("UPDATE agent_executions SET state=CASE WHEN state='queued' THEN 'cancelled' ELSE 'cancelling' END,generation=generation+1 WHERE workflow_id=?1 AND state IN ('queued','dispatching','running','recovering')",[id])?;
            let result = workflow(&tx, id)?.unwrap();
            event(&tx, id, None, "workflow_cancelling", result.generation, now)?;
        }
        let result = workflow(&tx, id)?.ok_or_else(|| invalid("协作任务不存在"))?;
        tx.commit()?;
        Ok(result)
    }

    /// Process exit is not proof of native stop. Preserve locks and invalidate old
    /// generations; no queued or unknown node is automatically replayed.
    pub fn recover_after_exit(&mut self, now: i64) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = tx
            .prepare("SELECT id FROM agent_workflows WHERE state IN ('running','cancelling')")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in &ids {
            revoke_workflow_scope(&tx, id)?;
            tx.execute("UPDATE agent_workflows SET state='recovering',generation=generation+1,updated_at=MAX(updated_at,?2) WHERE id=?1",params![id,now])?;
            tx.execute("UPDATE agent_executions SET state=CASE WHEN state='queued' THEN 'interrupted' ELSE 'recovering' END,generation=generation+1 WHERE workflow_id=?1 AND state IN ('queued','dispatching','running','cancelling')",[id])?;
            let result = workflow(&tx, id)?.unwrap();
            event(&tx, id, None, "recovery_required", result.generation, now)?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    /// One unknown native process blocks only its own workflow. Other roots keep
    /// their state; this is not the global Host-exit recovery operation.
    pub fn mark_unknown(&mut self, id: &str, now: i64) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE agent_workflows SET state='recovering',generation=generation+1,updated_at=MAX(updated_at,?2) WHERE id=?1 AND state IN ('running','cancelling')",params![id,now])?;
        if changed != 0 {
            revoke_workflow_scope(&tx, id)?;
            tx.execute("UPDATE agent_executions SET state=CASE WHEN state='queued' THEN 'interrupted' ELSE 'recovering' END,generation=generation+1 WHERE workflow_id=?1 AND state IN ('queued','dispatching','running','cancelling')",[id])?;
            let current = workflow(&tx, id)?.ok_or_else(|| invalid("协作记录缺失"))?;
            event(
                &tx,
                id,
                None,
                "native_state_unknown",
                current.generation,
                now,
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn children(&self, parent_run: &str) -> Result<Vec<Execution>> {
        let ids = self
            .conn
            .prepare("SELECT id FROM agent_executions WHERE parent_run_id=?1 ORDER BY rowid")?
            .query_map([parent_run], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.iter().map(|id| execution(self.conn, id)).collect()
    }
}

fn check_scope(conn: &Connection, policy: &Policy, now: i64) -> Result<()> {
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_scope_snapshots WHERE id=?1 AND scope_generation=?2 AND revoked=0 AND created_at<=?3 AND deadline_ms>?3)", params![policy.scope_snapshot_id,policy.scope_generation,now], |row|row.get(0))?;
    if !valid {
        return Err(invalid("协作范围已撤销、到期或发生变化"));
    }
    super::evidence::verify(
        conn,
        &policy.scope_snapshot_id,
        &policy.context.values().cloned().collect::<Vec<_>>(),
        now,
    )
    .map_err(Error::Invalid)?;
    Ok(())
}
fn check_workflow_scope(conn: &Connection, id: &str, now: i64) -> Result<()> {
    let raw: String = conn.query_row(
        "SELECT policy_json FROM agent_workflows WHERE id=?1",
        [id],
        |row| row.get(0),
    )?;
    check_scope(conn, &serde_json::from_str(&raw)?, now)
}
fn revoke_workflow_scope(conn: &Connection, id: &str) -> Result<()> {
    let scope: String = conn.query_row(
        "SELECT json_extract(policy_json,'$.scopeSnapshotId') FROM agent_workflows WHERE id=?1",
        [id],
        |row| row.get(0),
    )?;
    super::scope::revoke(conn, &scope).map_err(Error::Invalid)?;
    Ok(())
}

fn workflow(conn: &Connection, id: &str) -> Result<Option<Workflow>> {
    Ok(conn.query_row("SELECT id,root_thread_id,root_run_id,state,generation,deadline_ms,final_result_ref FROM agent_workflows WHERE id=?1",[id],|row|Ok(Workflow { id:row.get(0)?,root_thread_id:row.get(1)?,root_run_id:row.get(2)?,state:row.get(3)?,generation:row.get(4)?,deadline_ms:row.get(5)?,final_result_ref:row.get(6)? })).optional()?)
}
fn execution(conn: &Connection, id: &str) -> Result<Execution> {
    let (mut result,package): (Execution,Option<String>) = conn.query_row("SELECT id,workflow_id,parent_execution_id,state,generation,attempt,deadline_ms,task_json FROM agent_executions WHERE id=?1",[id],|row|Ok((Execution { id:row.get(0)?,workflow_id:row.get(1)?,parent_execution_id:row.get(2)?,state:row.get(3)?,generation:row.get(4)?,attempt:row.get(5)?,deadline_ms:row.get(6)?,package:None },row.get(7)?)))?;
    result.package = package
        .map(|json| serde_json::from_str(&json))
        .transpose()?;
    Ok(result)
}
fn event(
    conn: &Connection,
    workflow: &str,
    execution: Option<&str>,
    kind: &str,
    generation: i64,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO agent_workflow_events(workflow_id,seq,execution_id,kind,generation,timestamp) SELECT ?1,COALESCE(MAX(seq),0)+1,?2,?3,?4,?5 FROM agent_workflow_events WHERE workflow_id=?1",params![workflow,execution,kind,generation,now])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_runs(id TEXT PRIMARY KEY,thread_id TEXT,status TEXT); INSERT INTO agent_runs VALUES ('root','thread','\"running\"'),('other','other-thread','\"running\"'),('done','done-thread','\"completed\"');").unwrap();
        create_schema(&conn).unwrap();
        create_schema(&conn).unwrap();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let parent = crate::agent::pi::tools::Scope {
            read_lease: None,
            readonly: None,
            workspace: root.clone(),
            read_paths: vec![],
            workcopy: None,
            protected: vec![],
            mode: "plan".into(),
        };
        let scope = parent
            .restrict_readonly(std::slice::from_ref(&root), false)
            .unwrap();
        conn.execute("INSERT INTO agent_scope_snapshots(id,scope_generation,scope_json,created_at,deadline_ms) VALUES ('scope-root',1,?1,0,1000000)",[serde_json::to_string(&scope).unwrap()]).unwrap();
        let evidence = super::super::evidence::FrozenEvidence::capture_file(
            &parent,
            "ref-1",
            &root.join("Cargo.toml"),
        )
        .unwrap();
        super::super::evidence::persist(&conn, "scope-root", &evidence, 1).unwrap();
        conn
    }
    fn policy() -> Policy {
        let reference = ContextRef {
            id: "ref-1".into(),
            capability: Capability::FileRead,
            version: {
                use sha2::{Digest, Sha256};
                format!(
                    "sha256:{:x}",
                    Sha256::digest(
                        std::fs::read(
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
                        )
                        .unwrap()
                    )
                )
            },
        };
        Policy {
            context: BTreeMap::from([("ref-1".into(), reference)]),
            capabilities: BTreeSet::from([Capability::FileRead]),
            scope_snapshot_id: "scope-root".into(),
            scope_generation: 1,
            provider_id: "provider".into(),
            model_id: "model".into(),
        }
    }
    fn request() -> Delegation {
        Delegation::parse(&json!({"profileId":"reviewer","objective":"核对来源","contextRefIds":["ref-1"],"requiredCapabilities":[],"outputSchemaId":"evidence-review.v1"}).to_string()).unwrap()
    }
    #[test]
    fn scope_deadline_caps_parent_and_children_and_expiry_waits_for_stop() {
        let mut conn = setup();
        conn.execute("INSERT INTO agent_scope_snapshots SELECT 'short',scope_generation,scope_json,created_at,100,revoked FROM agent_scope_snapshots WHERE id='scope-root'", []).unwrap();
        conn.execute("INSERT INTO agent_scope_evidence SELECT 'short',ref_id,binding_json FROM agent_scope_evidence WHERE scope_id='scope-root'", []).unwrap();
        let mut policy = policy();
        policy.scope_snapshot_id = "short".into();
        let mut store = WorkflowStore::new(&mut conn);
        let workflow = store.begin("root", "request", &policy, 1).unwrap();
        assert_eq!(workflow.deadline_ms, 100);
        let active = store.delegate("root", "active", &request(), 2).unwrap();
        let queued = store.delegate("root", "queued", &request(), 2).unwrap();
        assert_eq!(active.deadline_ms, 100);
        store.reserve(&active.id, 0, 3).unwrap();
        assert!(store.expire(99).unwrap().is_empty());
        let expired = store.expire(100).unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, "cancelling");
        assert!(store.expire(101).unwrap().is_empty());
        assert!(store.reserve(&queued.id, 0, 101).is_err());
        let nodes = store.children("root").unwrap();
        assert_eq!(nodes[0].state, "cancelling");
        assert_eq!(nodes[1].state, "cancelled");
        store
            .confirm_stopped(&nodes[0].id, nodes[0].generation, 102)
            .unwrap();
        assert_eq!(store.get(&workflow.id).unwrap().state, "cancelling");
        assert_eq!(
            conn.query_row(
                "SELECT revoked FROM agent_scope_snapshots WHERE id='short'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn child_deadline_and_external_revocation_stop_the_workflow() {
        for revoked in [false, true] {
            let mut conn = setup();
            let (workflow, child) = {
                let mut store = WorkflowStore::new(&mut conn);
                let workflow = store.begin("root", "request", &policy(), 1).unwrap();
                let child = store.delegate("root", "call", &request(), 2).unwrap();
                (workflow, child)
            };
            let now = if revoked {
                super::super::scope::revoke(&conn, "scope-root").unwrap();
                3
            } else {
                child.deadline_ms
            };
            let mut store = WorkflowStore::new(&mut conn);
            assert!(now < workflow.deadline_ms);
            assert_eq!(store.expire(now).unwrap()[0].state, "cancelling");
            assert!(store.finish_root("root", Some("answer"), now).is_err());
        }
    }

    #[test]
    fn finalization_requires_authoritative_root_and_terminal_children() {
        let mut conn = setup();
        let mut store = WorkflowStore::new(&mut conn);
        let workflow = store.begin("root", "request", &policy(), 1).unwrap();
        assert!(store.finish_root("root", Some("answer"), 2).is_err());
        let child = store.delegate("root", "call", &request(), 3).unwrap();
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='root'",
            [],
        )
        .unwrap();
        let mut store = WorkflowStore::new(&mut conn);
        assert!(store.finish_root("root", Some("answer"), 4).is_err());
        // An adapter-confirmed failed child must downgrade the aggregate result.
        conn.execute(
            "UPDATE agent_executions SET state='failed' WHERE id=?1",
            [&child.id],
        )
        .unwrap();
        let mut store = WorkflowStore::new(&mut conn);
        assert!(store.finish_root("root", None, 5).is_err());
        assert!(store.finish_root("root", Some("answer"), 2).is_err());
        let result = store.finish_root("root", Some("answer"), 6).unwrap();
        assert_eq!(result.state, "partial");
        assert_eq!(result.final_result_ref.as_deref(), Some("answer"));
        assert!(store.finish_root("root", Some("overwritten"), 7).is_err());
        assert_eq!(
            store.get(&workflow.id).unwrap().final_result_ref.as_deref(),
            Some("answer")
        );
        assert_eq!(
            conn.query_row(
                "SELECT revoked FROM agent_scope_snapshots WHERE id='scope-root'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn root_terminal_status_is_not_invented_by_model_results() {
        for (run_status, expected) in [
            ("completed", "succeeded"),
            ("failed", "failed"),
            ("cancelled", "cancelled"),
            ("interrupted", "interrupted"),
        ] {
            let mut conn = setup();
            WorkflowStore::new(&mut conn)
                .begin("root", "request", &policy(), 1)
                .unwrap();
            conn.execute(
                "UPDATE agent_runs SET status=?1 WHERE id='root'",
                [serde_json::to_string(run_status).unwrap()],
            )
            .unwrap();
            let result = WorkflowStore::new(&mut conn)
                .finish_root("root", Some("answer"), 2)
                .unwrap();
            assert_eq!(result.state, expected);
        }
    }

    #[test]
    fn replay_is_stable_conflicts_fail_and_active_root_is_unique() {
        let mut conn = setup();
        let mut store = WorkflowStore::new(&mut conn);
        let workflow = store.begin("root", "request", &policy(), 1).unwrap();
        assert_eq!(
            store.begin("root", "request", &policy(), 20).unwrap().id,
            workflow.id
        );
        assert_eq!(
            store.get(&workflow.id).unwrap().deadline_ms,
            WORKFLOW_MS + 1
        );
        assert!(store.begin("other", "request", &policy(), 20).is_err());
        let mut changed = policy();
        changed.model_id = "changed".into();
        assert!(store.begin("root", "request", &changed, 20).is_err());
        assert!(store.begin("root", "new-request", &policy(), 20).is_err());
        assert!(store.begin("done", "done-request", &policy(), 20).is_err());
        let child = store.delegate("root", "call-1", &request(), 2).unwrap();
        assert_eq!(
            store.delegate("root", "call-1", &request(), 30).unwrap().id,
            child.id
        );
        let mut changed = request();
        changed.objective = "another".into();
        assert!(store.delegate("root", "call-1", &changed, 30).is_err());
        assert!(store.delegate("other", "call-2", &request(), 30).is_err());
        assert_eq!(store.children("other").unwrap().len(), 0);
    }
    #[test]
    fn child_quota_deadline_single_active_and_cancellation_are_durable() {
        let mut conn = setup();
        let mut store = WorkflowStore::new(&mut conn);
        let workflow = store.begin("root", "request", &policy(), 1).unwrap();
        let mut nodes = Vec::new();
        for i in 0..MAX_CHILDREN {
            nodes.push(
                store
                    .delegate("root", &format!("call-{i}"), &request(), 2)
                    .unwrap(),
            );
        }
        assert!(store.delegate("root", "too-many", &request(), 2).is_err());
        let first = store.reserve(&nodes[0].id, 0, 3).unwrap();
        assert_eq!(first.attempt, 1);
        assert!(store.reserve(&nodes[1].id, 0, 3).is_err());
        assert!(store.reserve(&nodes[0].id, 0, 3).is_err());
        let cancelled = store.cancel(&workflow.id, 4).unwrap();
        assert_eq!(cancelled.state, "cancelling");
        assert_eq!(
            store.cancel(&workflow.id, 5).unwrap().generation,
            cancelled.generation
        );
        let children = store.children("root").unwrap();
        assert_eq!(children[0].state, "cancelling");
        assert!(children[1..].iter().all(|node| node.state == "cancelled"));
        assert!(store.reserve(&nodes[1].id, 1, 6).is_err());
        assert!(store.delegate("root", "late", &request(), 6).is_err());
    }
    #[test]
    fn host_exit_never_replays_unknown_or_resets_budget() {
        let mut conn = setup();
        let id;
        {
            let mut store = WorkflowStore::new(&mut conn);
            id = store.begin("root", "request", &policy(), 1).unwrap().id;
            let child = store.delegate("root", "call-1", &request(), 2).unwrap();
            store.delegate("root", "call-2", &request(), 2).unwrap();
            store.reserve(&child.id, 0, 3).unwrap();
        }
        let mut reopened = WorkflowStore::new(&mut conn);
        assert_eq!(reopened.recover_after_exit(4).unwrap(), 1);
        assert_eq!(reopened.recover_after_exit(5).unwrap(), 0);
        assert_eq!(reopened.get(&id).unwrap().state, "recovering");
        assert_eq!(reopened.get(&id).unwrap().deadline_ms, WORKFLOW_MS + 1);
        let children = reopened.children("root").unwrap();
        assert_eq!(children[0].state, "recovering");
        assert_eq!(children[1].state, "interrupted");
        assert!(children
            .iter()
            .all(|node| reopened.reserve(&node.id, node.generation, 6).is_err()));
        assert!(reopened.delegate("root", "late", &request(), 6).is_err());
    }
    #[test]
    fn expiry_and_clock_rollback_block_new_work_and_events_remain_ordered() {
        let mut conn = setup();
        let mut store = WorkflowStore::new(&mut conn);
        let workflow = store.begin("root", "request", &policy(), 100).unwrap();
        assert!(store.delegate("root", "early", &request(), 99).is_err());
        let child = store.delegate("root", "call-1", &request(), 101).unwrap();
        assert!(store.reserve(&child.id, 0, child.deadline_ms).is_err());
        assert!(store
            .delegate("root", "late", &request(), workflow.deadline_ms)
            .is_err());
        store.cancel(&workflow.id, 200).unwrap();
        let sequences: Vec<i64> = conn
            .prepare("SELECT seq FROM agent_workflow_events WHERE workflow_id=?1 ORDER BY seq")
            .unwrap()
            .query_map([workflow.id], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(sequences, vec![1, 2, 3]);
    }

    #[test]
    fn results_are_owner_and_attempt_bound_and_cancellation_retains_root_lock() {
        let mut conn = setup();
        let (workflow, child) = {
            let mut store = WorkflowStore::new(&mut conn);
            let workflow = store.begin("root", "request", &policy(), 1).unwrap();
            let child = store.delegate("root", "call", &request(), 2).unwrap();
            (workflow, store.reserve(&child.id, 0, 3).unwrap())
        };
        // Synthetic adapter acknowledgement. Production acknowledgement/internal
        // Thread binding is intentionally not enabled by this storage slice.
        conn.execute(
            "UPDATE agent_executions SET state='running' WHERE id=?1",
            [&child.id],
        )
        .unwrap();
        let body = json!({"summary":"已核对","findings":["有来源"],"evidenceRefs":["ref-1"],"unresolved":[]}).to_string();
        let mut store = WorkflowStore::new(&mut conn);
        assert!(store
            .complete_child(
                "other",
                &child.id,
                child.generation,
                ResultStatus::Succeeded,
                &body,
                4
            )
            .is_err());
        assert!(store
            .complete_child("root", &child.id, 0, ResultStatus::Succeeded, &body, 4)
            .is_err());
        store.cancel(&workflow.id, 5).unwrap();
        assert!(store
            .complete_child(
                "root",
                &child.id,
                child.generation,
                ResultStatus::Succeeded,
                &body,
                6
            )
            .is_err());
        let child = store.children("root").unwrap().remove(0);
        assert!(store
            .confirm_stopped(&child.id, child.generation - 1, 7)
            .is_err());
        store
            .confirm_stopped(&child.id, child.generation, 7)
            .unwrap();
        assert_eq!(store.get(&workflow.id).unwrap().state, "cancelling");
        let (root_id, generation): (String, i64) = conn.query_row("SELECT id,generation FROM agent_executions WHERE workflow_id=?1 AND parent_execution_id IS NULL", [&workflow.id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        let mut store = WorkflowStore::new(&mut conn);
        store.confirm_stopped(&root_id, generation, 8).unwrap();
        assert_eq!(store.get(&workflow.id).unwrap().state, "cancelled");
    }

    #[test]
    fn duplicate_result_does_not_overwrite_a_completed_attempt() {
        let mut conn = setup();
        let child = {
            let mut store = WorkflowStore::new(&mut conn);
            store.begin("root", "request", &policy(), 1).unwrap();
            let child = store.delegate("root", "call", &request(), 2).unwrap();
            store.reserve(&child.id, 0, 3).unwrap()
        };
        conn.execute(
            "UPDATE agent_executions SET state='running' WHERE id=?1",
            [&child.id],
        )
        .unwrap();
        let body = json!({"summary":"已核对","findings":["有来源"],"evidenceRefs":["ref-1"],"unresolved":[]}).to_string();
        let mut store = WorkflowStore::new(&mut conn);
        assert_eq!(
            store
                .complete_child(
                    "root",
                    &child.id,
                    child.generation,
                    ResultStatus::Succeeded,
                    &body,
                    4
                )
                .unwrap()
                .state,
            "succeeded"
        );
        assert!(store
            .complete_child(
                "root",
                &child.id,
                child.generation,
                ResultStatus::Failed,
                &body,
                5
            )
            .is_err());
        assert_eq!(store.children("root").unwrap()[0].state, "succeeded");
    }
    #[test]
    fn finished_parent_cannot_create_or_dispatch_more_children() {
        let mut conn = setup();
        let child = {
            let mut store = WorkflowStore::new(&mut conn);
            store.begin("root", "request", &policy(), 1).unwrap();
            store.delegate("root", "before", &request(), 2).unwrap()
        };
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='root'",
            [],
        )
        .unwrap();
        let mut store = WorkflowStore::new(&mut conn);
        assert!(store.delegate("root", "after", &request(), 3).is_err());
        assert!(store.reserve(&child.id, 0, 3).is_err());
        assert_eq!(store.children("root").unwrap()[0].state, "queued");
    }

    #[test]
    fn changed_input_cannot_be_accepted_as_current_child_success() {
        let mut conn = setup();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("result-evidence-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, "original evidence").unwrap();
        let raw: String = conn
            .query_row(
                "SELECT scope_json FROM agent_scope_snapshots WHERE id='scope-root'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let scope = serde_json::from_str(&raw).unwrap();
        let evidence =
            super::super::evidence::FrozenEvidence::capture_file(&scope, "ref-2", &path).unwrap();
        super::super::evidence::persist(&conn, "scope-root", &evidence, 1).unwrap();
        let mut policy = policy();
        policy.context = BTreeMap::from([("ref-2".into(), evidence.reference())]);
        let mut request = request();
        request.context_ref_ids = vec!["ref-2".into()];
        let child = {
            let mut store = WorkflowStore::new(&mut conn);
            store.begin("root", "request", &policy, 1).unwrap();
            let child = store.delegate("root", "call", &request, 2).unwrap();
            store.reserve(&child.id, 0, 3).unwrap()
        };
        // Only the storage contract is exercised here; this is not native execution.
        conn.execute(
            "UPDATE agent_executions SET state='running' WHERE id=?1",
            [&child.id],
        )
        .unwrap();
        let body = json!({"summary":"verified","findings":["finding"],"evidenceRefs":["ref-2"],"unresolved":[]}).to_string();
        std::fs::write(&path, "updated evidence").unwrap();
        let mut store = WorkflowStore::new(&mut conn);
        assert!(store
            .complete_child(
                "root",
                &child.id,
                child.generation,
                ResultStatus::Succeeded,
                &body,
                4
            )
            .is_err());
        assert_eq!(store.children("root").unwrap()[0].state, "running");
        std::fs::write(&path, "original evidence").unwrap();
        assert_eq!(
            store
                .complete_child(
                    "root",
                    &child.id,
                    child.generation,
                    ResultStatus::Succeeded,
                    &body,
                    5
                )
                .unwrap()
                .state,
            "succeeded"
        );
        std::fs::remove_file(path).unwrap();
    }
}
