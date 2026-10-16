//! Root-owned, bounded process summaries. Child transcripts stay private.
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub run_id: String,
    pub thread_id: String,
    pub workflow_id: String,
    pub state: String,
    pub locked: bool,
    pub sequence: i64,
    pub children: Vec<Child>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Child {
    pub execution_id: String,
    pub role: String,
    pub objective: String,
    pub state: String,
    pub summary: Option<String>,
    pub unresolved: Vec<String>,
    pub model_calls: Option<i64>,
}
pub fn read(conn: &Connection, root_run: &str) -> Result<Option<Snapshot>, String> {
    super::sessions::require_user_run(conn, root_run)?;
    if !super::supervisor::installed(conn).map_err(|_| "无法读取协作状态")? {
        return Ok(None);
    }
    let tx = conn
        .unchecked_transaction()
        .map_err(|_| "无法读取协作快照")?;
    let record: Option<(String,String,String,i64)> = tx.query_row(
        "SELECT w.id,w.root_thread_id,w.state,COALESCE((SELECT MAX(seq) FROM agent_workflow_events WHERE workflow_id=w.id),0) FROM agent_workflows w WHERE root_run_id=?1", [root_run],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_| "无法读取协作状态")?;
    let Some((id, thread, state, sequence)) = record else {
        return Ok(None);
    };
    let mut stmt = tx.prepare("SELECT e.id,e.state,e.task_json,e.result_json,r.current_model_calls FROM agent_executions e LEFT JOIN agent_runs r ON r.id=e.run_id WHERE e.workflow_id=?1 AND e.parent_execution_id IS NOT NULL ORDER BY e.rowid LIMIT 4").map_err(|_| "无法读取子任务状态")?;
    let children = stmt
        .query_map([&id], |r| {
            let task: String = r.get(2)?;
            let result: Option<String> = r.get(3)?;
            let task: super::contracts::TaskPackage =
                serde_json::from_str(&task).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let result = result
                .map(|v| serde_json::from_str::<super::contracts::ChildResult>(&v))
                .transpose()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(Child {
                execution_id: r.get(0)?,
                state: r.get(1)?,
                role: match task.delegation.profile_id {
                    super::contracts::Profile::Researcher => "researcher",
                    super::contracts::Profile::Reviewer => "reviewer",
                }
                .into(),
                objective: task.delegation.objective,
                summary: result.as_ref().map(|r| r.summary.clone()),
                unresolved: result.map(|r| r.unresolved).unwrap_or_default(),
                model_calls: r.get(4)?,
            })
        })
        .map_err(|_| "无法读取子任务状态")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "协作记录格式无效")?;
    Ok(Some(Snapshot {
        run_id: root_run.into(),
        thread_id: thread,
        workflow_id: id,
        locked: matches!(state.as_str(), "running" | "cancelling" | "recovering"),
        state,
        sequence,
        children,
    }))
}

#[tauri::command]
pub async fn agent_workflow_snapshot(
    app: tauri::AppHandle,
    run_id: String,
) -> crate::commands::ApiResponse<Option<Snapshot>> {
    let path = crate::db::get_db_path(&app);
    let result = (|| {
        let conn = Connection::open(&path).map_err(|_| "无法读取任务记录".to_string())?;
        let snapshot = read(&conn, &run_id)?;
        if snapshot.as_ref().is_some_and(|s| s.locked)
            && !crate::agent::commands::global_cancel_registry().contains(&run_id)
        {
            // The OS owner lock, not absence of a token, authorizes restart recovery.
            crate::agent::submission::live::ensure_owner(&path)?;
            return read(&conn, &run_id);
        }
        Ok(snapshot)
    })();
    match result {
        Ok(value) => crate::commands::ApiResponse::ok(value),
        Err(error) => crate::commands::ApiResponse::err(error),
    }
}

/// Release admission remains closed until the S2 release gates pass.
#[tauri::command]
pub fn agent_workflow_preview_available() -> bool {
    super::admission::preview_available()
}
