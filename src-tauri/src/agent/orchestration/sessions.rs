//! Host-owned internal sessions. Ordinary thread APIs must not expose or mutate
//! them; an execution owns exactly one thread and never borrows a root Session.
use rusqlite::{params, Connection, OptionalExtension};

pub fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_internal_threads (
            thread_id TEXT PRIMARY KEY, root_thread_id TEXT NOT NULL,
            execution_id TEXT NOT NULL UNIQUE,
            CHECK(thread_id<>root_thread_id)
        );
        CREATE INDEX IF NOT EXISTS idx_internal_thread_root ON agent_internal_threads(root_thread_id);
        CREATE TRIGGER IF NOT EXISTS internal_thread_owner_immutable
        BEFORE UPDATE ON agent_internal_threads
        BEGIN SELECT RAISE(ABORT,'Internal session ownership is immutable'); END;
        CREATE TRIGGER IF NOT EXISTS internal_thread_owner_valid
        BEFORE INSERT ON agent_internal_threads
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_executions e JOIN agent_workflows w ON w.id=e.workflow_id
            JOIN agent_threads root ON root.id=w.root_thread_id
            JOIN agent_threads child ON child.id=NEW.thread_id
            WHERE e.id=NEW.execution_id AND e.parent_execution_id IS NOT NULL
            AND w.root_thread_id=NEW.root_thread_id
            AND child.archived_at IS NOT NULL AND child.closed_at IS NOT NULL
            AND child.project_id IS root.project_id AND child.external_session_id IS NULL
            AND child.latest_run_id IS NULL
            AND NOT EXISTS(SELECT 1 FROM agent_internal_threads WHERE thread_id=root.id)
        )
        BEGIN SELECT RAISE(ABORT,'Invalid internal session ownership'); END;
        CREATE TRIGGER IF NOT EXISTS internal_session_binding_unique
        BEFORE UPDATE OF external_session_id,engine ON agent_threads
        WHEN EXISTS(SELECT 1 FROM agent_internal_threads WHERE thread_id=OLD.id)
          AND ((OLD.external_session_id IS NOT NULL AND (NEW.external_session_id IS NOT OLD.external_session_id OR NEW.engine<>OLD.engine))
               OR (NEW.external_session_id IS NOT NULL AND EXISTS(SELECT 1 FROM agent_threads t WHERE t.id<>OLD.id AND t.external_session_id=NEW.external_session_id)))
        BEGIN SELECT RAISE(ABORT,'Internal native sessions cannot be shared or rebound'); END;
        CREATE TRIGGER IF NOT EXISTS internal_thread_private
        BEFORE UPDATE OF archived_at,closed_at,pinned_at,collection_id ON agent_threads
        WHEN EXISTS(SELECT 1 FROM agent_internal_threads WHERE thread_id=OLD.id)
        AND (NEW.archived_at IS NOT OLD.archived_at OR NEW.closed_at IS NOT OLD.closed_at
             OR NEW.pinned_at IS NOT NULL OR NEW.collection_id IS NOT NULL)
        BEGIN SELECT RAISE(ABORT,'Internal sessions are not ordinary conversations'); END;
        CREATE TRIGGER IF NOT EXISTS internal_thread_project
        BEFORE UPDATE OF project_id ON agent_threads
        WHEN EXISTS(SELECT 1 FROM agent_internal_threads i JOIN agent_threads root ON root.id=i.root_thread_id
                    WHERE i.thread_id=OLD.id AND NEW.project_id IS NOT root.project_id)
        BEGIN SELECT RAISE(ABORT,'Internal session project follows its root'); END;
        CREATE TRIGGER IF NOT EXISTS internal_thread_delete
        BEFORE DELETE ON agent_threads
        WHEN EXISTS(SELECT 1 FROM agent_internal_threads i JOIN agent_threads root ON root.id=i.root_thread_id
                    WHERE i.thread_id=OLD.id)
        BEGIN SELECT RAISE(ABORT,'Internal sessions must be deleted with their root'); END;
        CREATE TRIGGER IF NOT EXISTS internal_thread_owner_delete
        BEFORE DELETE ON agent_internal_threads
        WHEN EXISTS(SELECT 1 FROM agent_threads WHERE id=OLD.thread_id)
        BEGIN SELECT RAISE(ABORT,'Cannot detach an internal session'); END;
        CREATE TRIGGER IF NOT EXISTS internal_run_requires_execution
        BEFORE INSERT ON agent_runs
        WHEN EXISTS(SELECT 1 FROM agent_internal_threads WHERE thread_id=NEW.thread_id)
          AND NOT EXISTS(SELECT 1 FROM agent_internal_threads i JOIN agent_executions e ON e.id=i.execution_id
                         JOIN agent_workflows w ON w.id=e.workflow_id
                         WHERE i.thread_id=NEW.thread_id AND e.run_id=NEW.id AND e.state='dispatching'
                         AND w.state='running' AND NEW.provider=json_extract(w.policy_json,'$.providerId')
                         AND NEW.model=json_extract(w.policy_json,'$.modelId')
                         AND NEW.max_model_calls BETWEEN 1 AND 8)
        BEGIN SELECT RAISE(ABORT,'Internal runs require atomic execution admission'); END;
        CREATE TRIGGER IF NOT EXISTS workflow_root_new_run
        BEFORE INSERT ON agent_runs
        WHEN EXISTS(SELECT 1 FROM agent_workflows WHERE root_thread_id=NEW.thread_id
                    AND root_run_id<>NEW.id AND state IN ('running','cancelling','recovering'))
        BEGIN SELECT RAISE(ABORT,'Workflow still owns the root conversation'); END;
        CREATE TRIGGER IF NOT EXISTS workflow_run_delete
        BEFORE DELETE ON agent_runs
        WHEN EXISTS(SELECT 1 FROM agent_executions e JOIN agent_workflows w ON w.id=e.workflow_id
                    JOIN agent_threads t ON t.id=w.root_thread_id WHERE e.run_id=OLD.id)
        BEGIN SELECT RAISE(ABORT,'Workflow runs must be cleaned up with their root conversation'); END;
        CREATE TRIGGER IF NOT EXISTS workflow_root_mutation
        BEFORE UPDATE OF project_id,archived_at,closed_at ON agent_threads
        WHEN (NEW.project_id IS NOT OLD.project_id OR NEW.archived_at IS NOT OLD.archived_at
              OR NEW.closed_at IS NOT OLD.closed_at)
          AND EXISTS(SELECT 1 FROM agent_workflows WHERE root_thread_id=OLD.id
                     AND state IN ('running','cancelling','recovering'))
        BEGIN SELECT RAISE(ABORT,'Stop and reconcile the whole workflow first'); END;
        CREATE TRIGGER IF NOT EXISTS workflow_root_delete
        BEFORE DELETE ON agent_threads
        WHEN EXISTS(SELECT 1 FROM agent_workflows WHERE root_thread_id=OLD.id
                    AND state IN ('running','cancelling','recovering'))
        BEGIN SELECT RAISE(ABORT,'Stop and reconcile the whole workflow first'); END;
        CREATE TRIGGER IF NOT EXISTS workflow_root_move_children
        AFTER UPDATE OF project_id ON agent_threads
        WHEN NEW.project_id IS NOT OLD.project_id
        BEGIN
            UPDATE agent_threads SET project_id=NEW.project_id WHERE id IN
                (SELECT thread_id FROM agent_internal_threads WHERE root_thread_id=NEW.id);
            UPDATE agent_runs SET project_id=NEW.project_id WHERE thread_id IN
                (SELECT thread_id FROM agent_internal_threads WHERE root_thread_id=NEW.id);
        END;
        CREATE TRIGGER IF NOT EXISTS workflow_root_delete_children
        AFTER DELETE ON agent_threads
        BEGIN
            DELETE FROM agent_run_events WHERE thread_id IN
                (SELECT thread_id FROM agent_internal_threads WHERE root_thread_id=OLD.id);
            DELETE FROM agent_tool_calls WHERE run_id IN
                (SELECT r.id FROM agent_runs r JOIN agent_internal_threads i ON i.thread_id=r.thread_id WHERE i.root_thread_id=OLD.id);
            DELETE FROM agent_approvals WHERE run_id IN
                (SELECT r.id FROM agent_runs r JOIN agent_internal_threads i ON i.thread_id=r.thread_id WHERE i.root_thread_id=OLD.id);
            DELETE FROM agent_messages WHERE thread_id IN
                (SELECT thread_id FROM agent_internal_threads WHERE root_thread_id=OLD.id);
            DELETE FROM agent_runs WHERE thread_id IN
                (SELECT thread_id FROM agent_internal_threads WHERE root_thread_id=OLD.id);
            DELETE FROM agent_threads WHERE id IN
                (SELECT thread_id FROM agent_internal_threads WHERE root_thread_id=OLD.id);
            DELETE FROM agent_internal_threads WHERE root_thread_id=OLD.id;
            DELETE FROM agent_scope_evidence WHERE scope_id IN
                (SELECT json_extract(policy_json,'$.scopeSnapshotId') FROM agent_workflows WHERE root_thread_id=OLD.id);
            DELETE FROM agent_scope_snapshots WHERE id IN
                (SELECT json_extract(policy_json,'$.scopeSnapshotId') FROM agent_workflows WHERE root_thread_id=OLD.id);
            DELETE FROM agent_workflow_events WHERE workflow_id IN
                (SELECT id FROM agent_workflows WHERE root_thread_id=OLD.id);
            DELETE FROM agent_executions WHERE workflow_id IN
                (SELECT id FROM agent_workflows WHERE root_thread_id=OLD.id);
            DELETE FROM agent_workflows WHERE root_thread_id=OLD.id;
        END;",
    )
}

/// Old databases and isolated legacy contract fixtures have no internal sessions.
/// A SQL error is propagated, never interpreted as an ordinary user thread.
pub fn is_internal(conn: &Connection, thread: &str) -> rusqlite::Result<bool> {
    let installed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_internal_threads')",
        [], |r| r.get(0),
    )?;
    if !installed {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_internal_threads WHERE thread_id=?1)",
        [thread],
        |r| r.get(0),
    )
}

pub fn require_user_thread(conn: &Connection, thread: &str) -> Result<(), String> {
    if is_internal(conn, thread).map_err(|_| "无法核对会话归属")? {
        return Err("子任务只能在所属任务中查看，不能作为普通会话操作".into());
    }
    Ok(())
}

pub fn require_user_run(conn: &Connection, run: &str) -> Result<(), String> {
    let thread: Option<String> = conn
        .query_row("SELECT thread_id FROM agent_runs WHERE id=?1", [run], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|_| "无法核对运行归属")?;
    if let Some(thread) = thread {
        require_user_thread(conn, &thread)?;
    }
    Ok(())
}

pub fn root_busy(conn: &Connection, thread: &str) -> rusqlite::Result<bool> {
    let installed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_workflows')",
        [],
        |r| r.get(0),
    )?;
    if !installed {
        return Ok(false);
    }
    conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_workflows WHERE root_thread_id=?1 AND state IN ('running','cancelling','recovering'))", [thread], |r|r.get(0))
}

/// Called only inside the execution's admission transaction, before native work.
pub(super) fn create_internal(
    conn: &Connection,
    execution: &str,
    now: i64,
) -> Result<String, String> {
    if conn.is_autocommit() {
        return Err("内部会话必须与节点在同一事务创建".into());
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT thread_id FROM agent_internal_threads WHERE execution_id=?1",
            [execution],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(thread) = existing {
        return Ok(thread);
    }
    let (root, project): (String,Option<String>) = conn.query_row(
        "SELECT w.root_thread_id,t.project_id FROM agent_executions e JOIN agent_workflows w ON w.id=e.workflow_id JOIN agent_threads t ON t.id=w.root_thread_id
         WHERE e.id=?1 AND e.parent_execution_id IS NOT NULL AND e.state='queued' AND w.state='running'",
        [execution], |r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"子任务不可创建会话")?;
    let id = format!("internal-{}", uuid::Uuid::new_v4());
    conn.execute("INSERT INTO agent_threads(id,title,status,project_id,created_at,updated_at,closed_at,archived_at) VALUES(?1,'只读子任务','\"completed\"',?2,?3,?3,?3,?3)",params![id,project,now]).map_err(|e|e.to_string())?;
    conn.execute("INSERT INTO agent_internal_threads(thread_id,root_thread_id,execution_id) VALUES(?1,?2,?3)",params![id,root,execution]).map_err(|e|e.to_string())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        orchestration::{
            contracts::*,
            store::{Policy, WorkflowStore},
        },
        store::RunStore,
        types::ThreadListScope,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn fixture() -> (Connection, String, String) {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::create_schema(&conn).unwrap();
        crate::db::create_schema(&conn).unwrap();
        conn.execute_batch("INSERT INTO agent_threads(id,title,status,created_at,updated_at,external_session_id) VALUES('root-thread','用户会话','\"running\"',0,0,'native-root');
            INSERT INTO agent_runs(id,thread_id,status,provider,model,created_at,updated_at) VALUES('root-run','root-thread','\"running\"','p','m',0,0);").unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap();
        let file = root.join("Cargo.toml");
        let scope = crate::agent::pi::tools::Scope {
            read_lease: None,
            readonly: None,
            workspace: root,
            read_paths: vec![],
            workcopy: None,
            protected: vec![],
            mode: "plan".into(),
        };
        let scope_id = super::super::scope::persist_readonly(
            &conn,
            &scope,
            std::slice::from_ref(&file),
            false,
            0,
            600_000,
            0,
        )
        .unwrap();
        let evidence =
            super::super::evidence::FrozenEvidence::capture_file(&scope, "ref-1", &file).unwrap();
        let reference = evidence.reference();
        super::super::evidence::persist(&conn, &scope_id, &evidence, 0).unwrap();
        let policy = Policy {
            context: BTreeMap::from([("ref-1".into(), reference)]),
            capabilities: BTreeSet::from([Capability::FileRead]),
            scope_snapshot_id: scope_id,
            scope_generation: 0,
            provider_id: "p".into(),
            model_id: "m".into(),
        };
        let mut store = WorkflowStore::new(&mut conn);
        let workflow = store.begin("root-run", "request-root", &policy, 1).unwrap();
        let child = store
            .delegate(
                "root-run",
                "call-1",
                &Delegation {
                    profile_id: Profile::Reviewer,
                    objective: "核对证据".into(),
                    context_ref_ids: vec!["ref-1".into()],
                    required_capabilities: BTreeSet::new(),
                    output_schema_id: "evidence-review.v1".into(),
                },
                2,
            )
            .unwrap();
        (conn, workflow.id, child.id)
    }
    fn thread(conn: &Connection, child: &str) -> String {
        conn.query_row(
            "SELECT thread_id FROM agent_internal_threads WHERE execution_id=?1",
            [child],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn internal_session_is_private_and_cannot_be_used_as_a_user_conversation() {
        let (conn, _, child) = fixture();
        let id = thread(&conn, &child);
        assert!(require_user_thread(&conn, &id).is_err());
        assert!(require_user_thread(&conn, "root-thread").is_ok());
        let store = RunStore::new(conn);
        assert_eq!(
            store
                .list_threads(None, ThreadListScope::Active)
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .list_threads(None, ThreadListScope::History)
            .unwrap()
            .is_empty());
        assert!(store.close_thread(&id, 3).is_err());
        assert!(store.reopen_thread(&id, 3).is_err());
        assert!(store.archive_thread(&id, 3).is_err());
        assert!(store.delete_thread(&id).is_err());
        assert!(store.update_thread_title(&id, "pretend user", 3).is_err());
        assert_eq!(store.gc_expired_threads(1, 1_000_000_000).unwrap(), 0);
    }

    #[test]
    fn database_guards_ownership_scope_and_native_session_binding() {
        let (conn, _, child) = fixture();
        let id = thread(&conn, &child);
        for sql in [
            "UPDATE agent_threads SET archived_at=NULL WHERE id=?1",
            "UPDATE agent_threads SET project_id='other' WHERE id=?1",
            "UPDATE agent_threads SET external_session_id='native-root' WHERE id=?1",
            "DELETE FROM agent_threads WHERE id=?1",
            "DELETE FROM agent_internal_threads WHERE thread_id=?1",
            "UPDATE agent_internal_threads SET execution_id='forged' WHERE thread_id=?1",
        ] {
            assert!(conn.execute(sql, [&id]).is_err(), "{sql}");
        }
        conn.execute(
            "UPDATE agent_threads SET external_session_id='native-child' WHERE id=?1",
            [&id],
        )
        .unwrap();
        assert!(conn
            .execute(
                "UPDATE agent_threads SET external_session_id='replacement' WHERE id=?1",
                [&id]
            )
            .is_err());
        assert!(conn.execute("INSERT INTO agent_runs(id,thread_id,created_at,updated_at) VALUES('forged',?1,0,0)",[&id]).is_err());
    }

    #[test]
    fn active_workflow_locks_root_even_after_planning_run_ends() {
        let (conn, _, _) = fixture();
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='root-run'",
            [],
        )
        .unwrap();
        assert!(root_busy(&conn, "root-thread").unwrap());
        for sql in ["UPDATE agent_threads SET project_id='p2' WHERE id='root-thread'","UPDATE agent_threads SET archived_at=3 WHERE id='root-thread'","DELETE FROM agent_threads WHERE id='root-thread'","INSERT INTO agent_runs(id,thread_id,created_at,updated_at) VALUES('new-run','root-thread',3,3)"] {assert!(conn.execute(sql,[]).is_err(),"{sql}");}
    }

    #[test]
    fn root_delete_cleans_owned_child_rows_without_touching_other_threads() {
        let (mut conn, w, child) = fixture();
        let id = thread(&conn, &child);
        let (_, _, run) = WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                3,
            )
            .unwrap();
        conn.execute("INSERT INTO agent_messages(id,thread_id,run_id,role,content,created_at) VALUES('msg',?1,?2,'assistant','child result',4)",params![id,run]).unwrap();
        conn.execute(
            "UPDATE agent_workflows SET state='cancelled' WHERE id=?1",
            [w],
        )
        .unwrap();
        let store = RunStore::new(conn);
        store.create_thread("unrelated", "keep", None, 4).unwrap();
        store.delete_thread("root-thread").unwrap();
        assert!(store.get_thread(&id).unwrap().is_none());
        assert!(store.get_messages(&id).unwrap().is_empty());
        assert!(store.list_runs_by_thread(&id).unwrap().is_empty());
        assert!(store.get_thread("unrelated").unwrap().is_some());
    }

    #[test]
    fn child_claim_is_atomic_deduplicated_and_shares_native_capacity() {
        let (mut conn, _, child) = fixture();
        conn.execute("INSERT INTO agent_runs(id,thread_id,status,created_at,updated_at) VALUES('busy','other','\"running\"',0,0)",[]).unwrap();
        assert!(WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                3
            )
            .is_err());
        let attempt: i64 = conn
            .query_row(
                "SELECT attempt FROM agent_executions WHERE id=?1",
                [&child],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempt, 0);
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='busy'",
            [],
        )
        .unwrap();
        let (node, id, run) = WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                4,
            )
            .unwrap();
        assert_eq!(node.attempt, 1);
        assert_eq!(
            crate::agent::submission::occupied_native_slots(&conn).unwrap(),
            2
        );
        assert!(require_user_run(&conn, &run).is_err());
        assert!(WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                4
            )
            .is_err());
        assert!(WorkflowStore::new(&mut conn)
            .acknowledge_child(&child, node.generation, &run, 5)
            .is_err());
        conn.execute(
            "UPDATE agent_runs SET status='\"running\"' WHERE id=?1",
            [&run],
        )
        .unwrap();
        WorkflowStore::new(&mut conn)
            .acknowledge_child(&child, node.generation, &run, 5)
            .unwrap();
        assert_eq!(thread(&conn, &child), id);
        WorkflowStore::new(&mut conn).recover_after_exit(6).unwrap();
        conn.execute("UPDATE agent_runs SET status='\"interrupted\"'", [])
            .unwrap();
        assert_eq!(
            crate::agent::submission::occupied_native_slots(&conn).unwrap(),
            2,
            "unverified descendants keep their slots"
        );
    }

    #[test]
    fn cancelled_node_never_creates_a_native_run() {
        let (mut conn, w, child) = fixture();
        WorkflowStore::new(&mut conn).cancel(&w, 3).unwrap();
        assert!(WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                4
            )
            .is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn whole_tree_cancel_revokes_tools_before_signals_and_releases_only_confirmed_runs() {
        let (mut conn, w, child) = fixture();
        let (_, _, run) = WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                3,
            )
            .unwrap();
        let runs = super::super::supervisor::cancel_for_run(&mut conn, "root-run", 4).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(runs.contains(&run));
        assert!(super::super::supervisor::cancellation_requested(&conn, &run, 4).unwrap());
        let revoked: i64 = conn
            .query_row("SELECT revoked FROM agent_scope_snapshots", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(revoked, 1);
        conn.execute(
            "UPDATE agent_runs SET status='\"cancelled\"' WHERE id=?1",
            [&run],
        )
        .unwrap();
        let remaining = super::super::supervisor::reconcile(&mut conn, &run, 5).unwrap();
        assert_eq!(remaining, vec!["root-run"]);
        assert_eq!(
            WorkflowStore::new(&mut conn).get(&w).unwrap().state,
            "cancelling"
        );
        conn.execute(
            "UPDATE agent_runs SET status='\"interrupted\"' WHERE id='root-run'",
            [],
        )
        .unwrap();
        super::super::supervisor::reconcile(&mut conn, "root-run", 6).unwrap();
        assert!(root_busy(&conn, "root-thread").unwrap());
        conn.execute(
            "UPDATE agent_runs SET status='\"cancelled\"' WHERE id='root-run'",
            [],
        )
        .unwrap();
        assert!(
            super::super::supervisor::reconcile(&mut conn, "root-run", 7)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            WorkflowStore::new(&mut conn).get(&w).unwrap().state,
            "cancelled"
        );
        assert_eq!(
            crate::agent::submission::occupied_native_slots(&conn).unwrap(),
            0
        );
    }

    #[test]
    fn finished_parent_cancels_unobserved_children_instead_of_abandoning_them() {
        let (mut conn, w, child) = fixture();
        let (_, _, run) = WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                3,
            )
            .unwrap();
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='root-run'",
            [],
        )
        .unwrap();
        assert_eq!(
            super::super::supervisor::reconcile(&mut conn, "root-run", 4).unwrap(),
            vec![run.clone()]
        );
        assert!(root_busy(&conn, "root-thread").unwrap());
        conn.execute(
            "UPDATE agent_runs SET status='\"cancelled\"' WHERE id=?1",
            [&run],
        )
        .unwrap();
        assert!(super::super::supervisor::reconcile(&mut conn, &run, 5)
            .unwrap()
            .is_empty());
        assert_eq!(
            WorkflowStore::new(&mut conn).get(&w).unwrap().state,
            "cancelled"
        );
    }

    #[test]
    fn whole_success_requires_validated_child_result_and_authoritative_root_completion() {
        let (mut conn, w, child) = fixture();
        let (node, _, run) = WorkflowStore::new(&mut conn)
            .claim_child(
                &child,
                0,
                crate::agent::registry::EngineId::Pi,
                "fixture",
                3,
            )
            .unwrap();
        conn.execute(
            "UPDATE agent_runs SET status='\"running\"' WHERE id=?1",
            [&run],
        )
        .unwrap();
        WorkflowStore::new(&mut conn)
            .acknowledge_child(&child, node.generation, &run, 4)
            .unwrap();
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id=?1",
            [&run],
        )
        .unwrap();
        WorkflowStore::new(&mut conn)
            .complete_child(
                "root-run",
                &child,
                node.generation,
                ResultStatus::Succeeded,
                r#"{"summary":"核对完成","findings":[],"evidenceRefs":["ref-1"],"unresolved":[]}"#,
                5,
            )
            .unwrap();
        assert!(super::super::supervisor::reconcile(&mut conn, &run, 6)
            .unwrap()
            .is_empty());
        assert!(root_busy(&conn, "root-thread").unwrap());
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='root-run'",
            [],
        )
        .unwrap();
        assert!(
            super::super::supervisor::reconcile(&mut conn, "root-run", 7)
                .unwrap()
                .is_empty()
        );
        let result = WorkflowStore::new(&mut conn).get(&w).unwrap();
        assert_eq!(result.state, "succeeded");
        assert_eq!(result.final_result_ref.as_deref(), Some("run:root-run"));
        assert!(!root_busy(&conn, "root-thread").unwrap());
    }
}
