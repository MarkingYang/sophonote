//! Durable cancellation precedes process signals. Reconciliation only consumes
//! authoritative terminal Run records; losing a local handle is never success.
use super::store::WorkflowStore;
use rusqlite::{Connection, OptionalExtension};

pub fn installed(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_workflows')",
        [],
        |r| r.get(0),
    )
}

pub fn cancellation_requested(conn: &Connection, run: &str, now: i64) -> rusqlite::Result<bool> {
    if !installed(conn)? {
        return Ok(false);
    }
    conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_executions e JOIN agent_workflows w ON w.id=e.workflow_id
        JOIN agent_scope_snapshots s ON s.id=json_extract(w.policy_json,'$.scopeSnapshotId')
        WHERE e.run_id=?1 AND (e.state IN ('cancelling','recovering','cancelled','interrupted')
          OR w.state!='running' OR w.deadline_ms<=?2 OR e.deadline_ms<=?2 OR s.revoked=1
          OR s.scope_generation<>json_extract(w.policy_json,'$.scopeGeneration')))", rusqlite::params![run,now],|r|r.get(0))
}

pub fn cancel_for_run(conn: &mut Connection, run: &str, now: i64) -> Result<Vec<String>, String> {
    if !installed(conn).map_err(|e| e.to_string())? {
        return Ok(vec![run.into()]);
    }
    let workflow: Option<String> = conn
        .query_row(
            "SELECT workflow_id FROM agent_executions WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(id) = workflow else {
        return Ok(vec![run.into()]);
    };
    WorkflowStore::new(conn)
        .cancel(&id, now)
        .map_err(|e| e.to_string())?;
    pending_runs(conn, &id)
}

fn pending_runs(conn: &Connection, id: &str) -> Result<Vec<String>, String> {
    conn.prepare("SELECT run_id FROM agent_executions WHERE workflow_id=?1 AND run_id IS NOT NULL AND state IN ('dispatching','running','cancelling','recovering')")
        .and_then(|mut s|s.query_map([id],|r|r.get::<_,String>(0))?.collect())
        .map_err(|e|e.to_string())
}

/// Called by the owning native lifecycle after recording Run status, and by an
/// explicit reconciliation request. Never dispatches or replays a model input.
pub fn reconcile(conn: &mut Connection, run: &str, now: i64) -> Result<Vec<String>, String> {
    if !installed(conn).map_err(|e| e.to_string())? {
        return Ok(vec![]);
    }
    let workflow: Option<String> = conn
        .query_row(
            "SELECT workflow_id FROM agent_executions WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(id) = workflow else {
        return Ok(vec![]);
    };
    let root = WorkflowStore::new(conn)
        .get(&id)
        .map_err(|e| e.to_string())?;
    if !matches!(root.state.as_str(), "running" | "cancelling" | "recovering") {
        return Ok(vec![]);
    }
    let unknown:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_executions e JOIN agent_runs r ON r.id=e.run_id WHERE e.workflow_id=?1 AND e.state IN ('dispatching','running','cancelling') AND lower(trim(r.status,'\"'))='interrupted')",[&id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if unknown {
        WorkflowStore::new(conn)
            .mark_unknown(&id, now)
            .map_err(|e| e.to_string())?;
        return pending_runs(conn, &id);
    }
    let root_status: String = conn
        .query_row(
            "SELECT lower(trim(status,'\"')) FROM agent_runs WHERE id=?1",
            [&root.root_run_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    // Run failures are Host facts, unlike a model's claimed result status.
    conn.execute("UPDATE agent_executions SET state=(SELECT lower(trim(status,'\"')) FROM agent_runs WHERE id=agent_executions.run_id),generation=generation+1
        WHERE workflow_id=?1 AND parent_execution_id IS NOT NULL AND state IN ('dispatching','running')
        AND EXISTS(SELECT 1 FROM agent_runs WHERE id=agent_executions.run_id AND lower(trim(status,'\"')) IN ('failed','cancelled'))",[&id]).map_err(|e|e.to_string())?;
    if root.state == "running"
        && matches!(root_status.as_str(), "completed" | "failed" | "cancelled")
    {
        // A root cannot leave unobserved work behind. Successful children must
        // already have a validated result, not merely a completed native Run.
        let active:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_executions WHERE workflow_id=?1 AND parent_execution_id IS NOT NULL AND state IN ('queued','dispatching','running','cancelling','recovering'))",[&id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !active {
            let final_ref = format!("run:{}", root.root_run_id);
            if WorkflowStore::new(conn)
                .finish_root(
                    &root.root_run_id,
                    (root_status == "completed").then_some(final_ref.as_str()),
                    now,
                )
                .is_ok()
            {
                return Ok(vec![]);
            }
        }
        WorkflowStore::new(conn)
            .cancel(&id, now)
            .map_err(|e| e.to_string())?;
    }
    // Only confirmed terminal Runs release cancellation slots. Interrupted is
    // deliberately excluded: it may represent an unobserved native process.
    let stopped:Vec<(String,i64)>=conn.prepare("SELECT e.id,e.generation FROM agent_executions e JOIN agent_runs r ON r.id=e.run_id WHERE e.workflow_id=?1 AND e.state IN ('cancelling','recovering') AND lower(trim(r.status,'\"')) IN ('completed','failed','cancelled')")
        .and_then(|mut s|s.query_map([&id],|r|Ok((r.get(0)?,r.get(1)?)))?.collect()).map_err(|e|e.to_string())?;
    for (node, generation) in stopped {
        WorkflowStore::new(conn)
            .confirm_stopped(&node, generation, now)
            .map_err(|e| e.to_string())?;
    }
    let current = WorkflowStore::new(conn)
        .get(&id)
        .map_err(|e| e.to_string())?;
    if matches!(current.state.as_str(), "cancelling" | "recovering") {
        pending_runs(conn, &id)
    } else {
        Ok(vec![])
    }
}

pub(crate) fn signal(runs: Vec<String>) -> bool {
    let mut signalled = false;
    for run in runs {
        signalled |= crate::agent::commands::global_cancel_registry().cancel(&run);
    }
    signalled
}

/// Periodic supervision belongs to an active workflow's driver, not a permanent
/// application-wide poller. Expiry revokes scope before notifying descendants.
pub fn expire(conn: &mut Connection, now: i64) -> Result<Vec<String>, String> {
    let workflows = WorkflowStore::new(conn)
        .expire(now)
        .map_err(|e| e.to_string())?;
    let mut runs = Vec::new();
    for workflow in workflows {
        runs.extend(pending_runs(conn, &workflow.id)?);
    }
    Ok(runs)
}
