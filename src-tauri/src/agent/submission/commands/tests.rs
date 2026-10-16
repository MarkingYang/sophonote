use super::*;
use crate::agent::submission::live::LiveSubmission;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/submission-command-tests")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        crate::db::create_schema(&Connection::open(&db).unwrap()).unwrap();
        Self(db)
    }
    fn request() -> AgentRunStartArgs {
        serde_json::from_value(serde_json::json!({"engine":"pi","message":"fixture"})).unwrap()
    }
    fn request_id() -> String {
        let stamp = format!("{:012x}", live::now_ms());
        format!("{}-{}-7000-8000-000000000001", &stamp[..8], &stamp[8..])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

#[test]
fn repeated_public_identity_owns_exactly_one_dispatch_lease() {
    let f = Fixture::new();
    let request = Fixture::request();
    let id = Fixture::request_id();
    let (first, lease) = LiveSubmission::admit(&f.0, &request, "pi", &id).unwrap();
    assert!(lease.is_some());
    let (again, replay) = LiveSubmission::admit(&f.0, &request, "pi", &id).unwrap();
    assert!(replay.is_none());
    assert_eq!(first, again);
    let mut changed = Fixture::request();
    changed.message = "different".into();
    assert!(LiveSubmission::admit(&f.0, &changed, "pi", &id).is_err());
    let mut conn = Connection::open(&f.0).unwrap();
    request_cancel(&mut conn, &first.id).unwrap();
    let (terminal, replay) = LiveSubmission::admit(&f.0, &request, "pi", &id).unwrap();
    assert_eq!(terminal.state, State::Cancelled);
    assert!(replay.is_none());
    let view = snapshot(&mut conn, &first.id).unwrap();
    assert!(view.cancel_requested);
    assert!(view.run_id.is_none());
    let encoded = serde_json::to_value(view).unwrap();
    assert_eq!(encoded["submissionId"], first.id);
    assert_eq!(encoded["clientRequestId"], id);
    for absent in ["intent", "intentHash", "messageBody", "key", "dbPath"] {
        assert!(encoded.get(absent).is_none());
    }
}

#[test]
fn cancellation_before_claim_invalidates_receipt_without_creating_run() {
    let f = Fixture::new();
    let (first, lease) =
        LiveSubmission::admit(&f.0, &Fixture::request(), "pi", &Fixture::request_id()).unwrap();
    let lease = lease.unwrap();
    let mut conn = Connection::open(&f.0).unwrap();
    let cancelled = request_cancel(&mut conn, &first.id).unwrap();
    assert_eq!(cancelled.state, State::Cancelled);
    assert!(cancelled.generation > first.generation);
    let tx = conn.transaction().unwrap();
    assert!(live::check_claim(&tx, "new", Some(&lease.receipt)).is_err());
    assert_eq!(
        tx.query_row("SELECT count(*) FROM agent_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn bind_fixture(conn: &mut Connection, lease: &LiveSubmission) {
    let tx = conn.transaction().unwrap();
    tx.execute("INSERT INTO agent_threads(id,title,status,created_at,updated_at) VALUES ('t','fixture','running',0,0)",[]).unwrap();
    tx.execute("INSERT INTO agent_runs(id,thread_id,status,engine,created_at,updated_at) VALUES ('r','t','queued','pi',0,0)",[]).unwrap();
    live::bind_claim(&tx, Some(&lease.receipt), "r", live::now_ms()).unwrap();
    tx.commit().unwrap();
}

#[test]
fn cancellation_between_claim_and_token_registration_is_not_lost() {
    let f = Fixture::new();
    let (first, lease) =
        LiveSubmission::admit(&f.0, &Fixture::request(), "pi", &Fixture::request_id()).unwrap();
    let lease = lease.unwrap();
    let mut conn = Connection::open(&f.0).unwrap();
    bind_fixture(&mut conn, &lease);
    let after = request_cancel(&mut conn, &first.id).unwrap();
    assert_eq!(after.state, State::Dispatching); // no fabricated cancelled terminal
    let token = tokio_util::sync::CancellationToken::new();
    apply_cancel_intent(&f.0, "r", &token);
    assert!(token.is_cancelled());
    let view = snapshot(&mut conn, &first.id).unwrap();
    assert!(view.cancel_requested);
    let again = request_cancel(&mut conn, &first.id).unwrap();
    assert_eq!(after, again);
    conn.execute("UPDATE agent_runs SET status='cancelled' WHERE id='r'", [])
        .unwrap();
    assert_eq!(
        snapshot(&mut conn, &first.id).unwrap().state,
        State::Cancelled
    );
}

#[test]
fn persisted_cancel_read_failure_prevents_native_start() {
    let f = Fixture::new();
    let token = tokio_util::sync::CancellationToken::new();
    apply_cancel_intent(&f.0, "unbound", &token);
    assert!(!token.is_cancelled());
    Connection::open(&f.0)
        .unwrap()
        .execute(
            "ALTER TABLE agent_submissions RENAME TO missing_submissions",
            [],
        )
        .unwrap();
    apply_cancel_intent(&f.0, "unbound", &token);
    assert!(token.is_cancelled());
}

#[test]
fn manual_api_requires_engine_and_complete_document_context() {
    let mut request = Fixture::request();
    assert_eq!(validate(&request).unwrap(), "pi");
    request.engine = Some("auto".into());
    assert!(validate(&request).is_err());
    request.engine = None;
    assert!(validate(&request).is_err());
    request=serde_json::from_value(serde_json::json!({"engine":"pi","message":"x","focusDocument":{"articleId":"a","markdown":null}})).unwrap();
    assert!(validate(&request).is_err());
    assert!(serde_json::from_value::<StartRequest>(
        serde_json::json!({"clientRequestId":"x","request":{"message":"x"},"routing":"auto"})
    )
    .is_err());
}

#[test]
fn automatic_identity_replays_after_selection_and_cannot_switch_twice() {
    let f = Fixture::new();
    let mut request = Fixture::request();
    request.engine = None;
    let id = Fixture::request_id();
    let (first, lease) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
    let mut lease = lease.unwrap();
    lease
        .select_engine(crate::agent::registry::EngineId::Pi)
        .unwrap();
    assert!(lease
        .select_engine(crate::agent::registry::EngineId::Hermes)
        .is_err());
    let mut conn = Connection::open(&f.0).unwrap();
    bind_fixture(&mut conn, &lease);
    let view = snapshot(&mut conn, &first.id).unwrap();
    assert_eq!(view.requested_engine, "auto");
    assert_eq!(view.selected_engine.as_deref(), Some("pi"));
    let (replayed, second) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
    assert!(second.is_none());
    assert_eq!(replayed.run_id.as_deref(), Some("r"));
}

#[test]
fn automatic_cancel_before_selection_prevents_late_choice() {
    let f = Fixture::new();
    let mut request = Fixture::request();
    request.engine = None;
    let (first, lease) =
        LiveSubmission::admit(&f.0, &request, "auto", &Fixture::request_id()).unwrap();
    let mut lease = lease.unwrap();
    request_cancel(&mut Connection::open(&f.0).unwrap(), &first.id).unwrap();
    assert!(lease
        .select_engine(crate::agent::registry::EngineId::Pi)
        .is_err());
}

#[test]
fn admission_capacity_is_shared_by_manual_and_auto_and_released_on_cancel() {
    let f = Fixture::new();
    let request = Fixture::request();
    let id = Fixture::request_id();
    let (first, _a) = LiveSubmission::admit(&f.0, &request, "pi", &id).unwrap();
    let (_second, _b) = LiveSubmission::admit(
        &f.0,
        &request,
        "auto",
        &id.replace("000000000001", "000000000002"),
    )
    .unwrap();
    let third = id.replace("000000000001", "000000000003");
    let (rejected, lease) = LiveSubmission::admit(&f.0, &request, "auto", &third).unwrap();
    assert!(lease.is_none());
    assert_eq!(rejected.state, State::Failed);
    assert!(rejected.run_id.is_none());
    let mut conn = Connection::open(&f.0).unwrap();
    assert!(snapshot(&mut conn, &rejected.id)
        .unwrap()
        .message
        .unwrap()
        .contains("名额"));
    drop(conn);
    let mut conn = Connection::open(&f.0).unwrap();
    assert_eq!(
        recover_draft(&conn, &third, false)
            .unwrap()
            .unwrap()
            .message,
        request.message
    );
    // Capacity rejection in the legacy manual API remains an explicit error.
    let manual_id = id.replace("000000000001", "000000000004");
    assert!(LiveSubmission::admit(&f.0, &request, "pi", &manual_id)
        .err()
        .unwrap()
        .contains("名额"));
    request_cancel(&mut conn, &first.id).unwrap();
    let (replayed, lease) = LiveSubmission::admit(&f.0, &request, "auto", &third).unwrap();
    assert_eq!(replayed, rejected);
    assert!(lease.is_none()); // freeing a slot must never reanimate an old identity
    let new_id = id.replace("000000000001", "000000000005");
    let (retry, lease) = LiveSubmission::admit(&f.0, &request, "auto", &new_id).unwrap();
    assert_eq!(retry.state, State::Preparing);
    assert!(lease.is_some());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn old_manual_schema_upgrade_preserves_ids_rows_indexes_and_guards() {
    let f = Fixture::new();
    let conn = Connection::open(&f.0).unwrap();
    // Build the previous manual schema in a separate real database.
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='agent_submissions'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let old = Connection::open(f.0.with_extension("old.db")).unwrap();
    old.execute_batch("CREATE TABLE agent_threads(id TEXT PRIMARY KEY); CREATE TABLE agent_runs(id TEXT PRIMARY KEY,thread_id TEXT,status TEXT);").unwrap();
    old.execute_batch(&sql.replace(",'auto'", "")).unwrap();
    old.execute_batch("CREATE INDEX old_submission_idx ON agent_submissions(state); CREATE TRIGGER old_submission_guard BEFORE DELETE ON agent_threads WHEN EXISTS(SELECT 1 FROM agent_submissions WHERE thread_id=OLD.id) BEGIN SELECT RAISE(ABORT,'guard'); END;").unwrap();
    old.execute("INSERT INTO agent_threads VALUES('t')", [])
        .unwrap();
    old.execute("INSERT INTO agent_submissions(id,client_request_id,intent_salt,intent_hash,requested_engine,thread_id,state,created_at,updated_at) VALUES('s','req','salt','hash','pi','t','failed',0,0)",[]).unwrap();
    crate::agent::submission::create_schema(&old).unwrap();
    crate::agent::submission::create_schema(&old).unwrap();
    assert_eq!(
        old.query_row(
            "SELECT client_request_id FROM agent_submissions WHERE id='s'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "req"
    );
    assert!(old.execute("DELETE FROM agent_threads", []).is_err());
    old.execute("UPDATE agent_submissions SET requested_engine='auto'", [])
        .unwrap();
}

#[test]
fn preparation_diagnostics_never_persist_native_secret_text() {
    let f = Fixture::new();
    let (submission, lease) =
        LiveSubmission::admit(&f.0, &Fixture::request(), "auto", &Fixture::request_id()).unwrap();
    lease
        .unwrap()
        .failed_reason("无法读取凭据: secret-token-at-private-url");
    let view = snapshot(&mut Connection::open(&f.0).unwrap(), &submission.id).unwrap();
    assert_eq!(view.state, State::Failed);
    assert!(view.message.as_deref().unwrap().contains("系统授权"));
    assert!(!serde_json::to_string(&view)
        .unwrap()
        .contains("secret-token"));
}

#[test]
fn automatic_no_run_draft_survives_reopen_and_claim_consumes_it() {
    let f = Fixture::new();
    let mut request = Fixture::request();
    request.engine = None;
    request.message = "恢复原始草稿".into();
    let (first, lease) =
        LiveSubmission::admit(&f.0, &request, "auto", &Fixture::request_id()).unwrap();
    let conn = Connection::open(&f.0).unwrap();
    let saved: String = conn
        .query_row(
            "SELECT recovery_json FROM agent_submissions WHERE id=?1",
            [&first.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<AgentRunStartArgs>(&saved)
            .unwrap()
            .message,
        request.message
    );
    drop(conn);
    let mut conn = Connection::open(&f.0).unwrap();
    let mut lease = lease.unwrap();
    lease
        .select_engine(crate::agent::registry::EngineId::Pi)
        .unwrap();
    bind_fixture(&mut conn, &lease);
    let saved: Option<String> = conn
        .query_row(
            "SELECT recovery_json FROM agent_submissions WHERE id=?1",
            [&first.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(saved.is_none());
}

#[test]
fn draft_read_requires_no_run_terminal_and_discard_keeps_identity() {
    let f = Fixture::new();
    let request: AgentRunStartArgs = serde_json::from_value(serde_json::json!({
        "message":"文字原样恢复", "attachments":[{"kind":"file","name":"资料","path":"/fixture/input.txt"}],
        "focusDocument":{"articleId":"article","title":"草稿","baseVersion":12,"markdown":"未保存正文"},
        "selection":{"articleId":"article","title":"选区","baseVersion":11,"selectedMarkdown":"原选区内容","selectedTextHash":"original-hash","beforeContext":"前文","afterContext":"后文"}
    })).unwrap();
    let id = Fixture::request_id();
    let (first, lease) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
    let conn = Connection::open(&f.0).unwrap();
    assert!(recover_draft(&conn, &id, false).unwrap().is_none());
    lease.unwrap().failed();
    let restored = recover_draft(&conn, &id, false).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(restored).unwrap(),
        serde_json::to_value(&request).unwrap()
    );
    assert!(!serde_json::to_string(
        &snapshot(&mut Connection::open(&f.0).unwrap(), &first.id).unwrap()
    )
    .unwrap()
    .contains("未保存正文"));
    recover_draft(&conn, &id, true).unwrap();
    assert!(recover_draft(&conn, &id, false).unwrap().is_none());
    let (_, second) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
    assert!(second.is_none());
    assert!(recover_draft(&conn, &id, false).unwrap().is_none());
}

#[test]
fn cancelled_and_crashed_preparation_keep_draft_but_tombstone_erases_it() {
    for crash in [false, true] {
        let f = Fixture::new();
        let id = Fixture::request_id();
        let (first, lease) = LiveSubmission::admit(&f.0, &Fixture::request(), "auto", &id).unwrap();
        let mut conn = Connection::open(&f.0).unwrap();
        if crash {
            conn.execute(
                "UPDATE agent_submissions SET state='interrupted' WHERE id=?1",
                [&first.id],
            )
            .unwrap();
        } else {
            request_cancel(&mut conn, &first.id).unwrap();
        }
        assert!(recover_draft(&conn, &id, false).unwrap().is_some());
        drop(lease);
        super::super::SubmissionStore::new(&mut conn)
            .tombstone(&first.id, live::now_ms())
            .unwrap();
        assert!(recover_draft(&conn, &id, false).unwrap().is_none());
        let raw: Option<String> = conn
            .query_row(
                "SELECT recovery_json FROM agent_submissions WHERE id=?1",
                [&first.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(raw.is_none());
    }
}

#[test]
fn oversized_recovery_input_is_rejected_before_admission() {
    let f = Fixture::new();
    let mut request = Fixture::request();
    request.message = "x".repeat(4 * 1024 * 1024);
    assert!(LiveSubmission::admit(&f.0, &request, "auto", &Fixture::request_id()).is_err());
    let count: i64 = Connection::open(&f.0)
        .unwrap()
        .query_row("SELECT count(*) FROM agent_submissions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn busy_thread_draft_is_terminal_and_never_reacquires_execution() {
    for legacy_run in [false, true] {
        let f = Fixture::new();
        let mut conn = Connection::open(&f.0).unwrap();
        conn.execute("INSERT INTO agent_threads(id,title,status,created_at,updated_at) VALUES('t','fixture','running',0,0)", []).unwrap();
        let mut request = Fixture::request();
        request.engine = None;
        request.thread_id = Some("t".into());
        let id = Fixture::request_id();
        let busy_id = id.replace("000000000001", "000000000002");
        let active = if legacy_run {
            conn.execute("INSERT INTO agent_runs(id,thread_id,status,engine,created_at,updated_at) VALUES('busy','t','running','pi',0,0)", []).unwrap();
            None
        } else {
            Some(LiveSubmission::admit(&f.0, &request, "auto", &busy_id).unwrap())
        };
        let (failed, lease) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
        assert!(lease.is_none());
        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.thread_id.as_deref(), Some("t"));
        assert!(snapshot(&mut conn, &failed.id)
            .unwrap()
            .message
            .unwrap()
            .contains("此会话"));
        assert_eq!(
            serde_json::to_value(recover_draft(&conn, &id, false).unwrap().unwrap()).unwrap(),
            serde_json::to_value(&request).unwrap()
        );
        if let Some((submission, _lease)) = active {
            request_cancel(&mut conn, &submission.id).unwrap();
        } else {
            conn.execute(
                "UPDATE agent_runs SET status='completed' WHERE id='busy'",
                [],
            )
            .unwrap();
        }
        let (_, replay) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
        assert!(replay.is_none());
        // Neither a stale receipt nor a normal transition can revive the failure.
        let tx = conn.transaction().unwrap();
        let receipt = live::Receipt {
            id: failed.id.clone(),
            generation: failed.generation,
        };
        assert!(live::check_claim(&tx, "t", Some(&receipt)).is_err());
        tx.rollback().unwrap();
        assert!(super::super::SubmissionStore::new(&mut conn)
            .advance(
                &failed.id,
                failed.generation,
                State::Preparing,
                live::now_ms()
            )
            .is_err());
        let mut changed = request.clone();
        changed.message = "changed".into();
        assert!(LiveSubmission::admit(&f.0, &changed, "auto", &id).is_err());
        recover_draft(&conn, &id, true).unwrap();
        let (_, replay) = LiveSubmission::admit(&f.0, &request, "auto", &id).unwrap();
        assert!(replay.is_none());
        assert!(recover_draft(&conn, &id, false).unwrap().is_none());
    }
}

#[test]
fn busy_rejection_does_not_persist_invalid_or_oversized_inputs() {
    let f = Fixture::new();
    let request = Fixture::request();
    let id = Fixture::request_id();
    let (_a, _lease_a) = LiveSubmission::admit(&f.0, &request, "pi", &id).unwrap();
    let (_b, _lease_b) = LiveSubmission::admit(
        &f.0,
        &request,
        "pi",
        &id.replace("000000000001", "000000000002"),
    )
    .unwrap();
    let rejected_id = id.replace("000000000001", "000000000003");
    let mut invalid = request.clone();
    invalid.message = "x".repeat(4 * 1024 * 1024);
    assert!(LiveSubmission::admit(&f.0, &invalid, "auto", &rejected_id).is_err());
    invalid = request;
    invalid.thread_id = Some("missing".into());
    assert!(LiveSubmission::admit(&f.0, &invalid, "auto", &rejected_id).is_err());
    let conn = Connection::open(&f.0).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_submissions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(recover_draft(&conn, &rejected_id, false).unwrap().is_none());
}
