use super::*;
use serde_json::json;
use std::sync::{Arc, Barrier};

const NOW: i64 = 1_800_000_000_000;

struct TestDirectory(std::path::PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/submission-tests")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request_id(at: i64, discriminator: u8) -> String {
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&at.to_be_bytes()[2..]);
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[15] = discriminator;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::create_schema(&conn).unwrap();
    thread(&conn, "t");
    conn
}
fn thread(conn: &Connection, id: &str) {
    conn.execute("INSERT INTO agent_threads(id,title,status,created_at,updated_at) VALUES (?1,'test','running',0,0)", [id]).unwrap();
}
fn request<'a>(id: &'a str, thread: Option<&'a str>, input: &'a serde_json::Value) -> Request<'a> {
    Request {
        client_request_id: id,
        thread_id: thread,
        engine: "pi",
        intent: input,
    }
}
fn prepare(store: &mut SubmissionStore<'_>, id: &str) -> Submission {
    let a = store.advance(id, 0, State::Preparing, NOW + 1).unwrap();
    store
        .advance(id, a.generation, State::Prepared, NOW + 2)
        .unwrap()
}
fn run(conn: &Connection, id: &str, thread: &str, engine: &str) {
    conn.execute("INSERT INTO agent_runs(id,thread_id,status,provider,model,engine,created_at,updated_at) VALUES (?1,?2,'queued','p','m',?3,0,0)", params![id,thread,engine]).unwrap();
}

#[test]
fn schema_is_idempotent_and_preserves_existing_runs() {
    let conn = database();
    run(&conn, "old", "t", "pi");
    crate::db::create_schema(&conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_runs WHERE id='old'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_submissions", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn replay_binds_full_intent_even_after_expiry_and_creates_no_run() {
    let mut conn = database();
    let id = request_id(NOW, 1);
    let input = json!({"message":"private task","model":"m"});
    let mut store = SubmissionStore::new(&mut conn);
    let first = store.admit(request(&id, Some("t"), &input), NOW).unwrap();
    assert!(first.created);
    let replay = store
        .admit(
            request(&id.to_uppercase(), Some("t"), &input),
            NOW + 2 * DAY_MS,
        )
        .unwrap();
    assert!(!replay.created);
    assert_eq!(first.submission, replay.submission);
    for changed in [
        json!({"message":"different","model":"m"}),
        json!({"message":"private task","model":"other"}),
    ] {
        assert!(matches!(
            store.admit(request(&id, Some("t"), &changed), NOW),
            Err(Error::RequestConflict)
        ));
    }
    assert!(matches!(
        store.admit(request(&id, None, &input), NOW),
        Err(Error::RequestConflict)
    ));
    let mut other = request(&id, Some("t"), &input);
    other.engine = "opencode";
    assert!(matches!(
        store.admit(other, NOW),
        Err(Error::RequestConflict)
    ));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let hash: String = conn
        .query_row("SELECT intent_hash FROM agent_submissions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(hash.len(), 64);
    assert!(!hash.contains("private"));
}

#[test]
fn invalid_expired_and_future_ids_are_rejected_but_boundary_is_accepted() {
    let mut conn = database();
    let input = json!({});
    let mut store = SubmissionStore::new(&mut conn);
    for id in ["bad".into(), uuid::Uuid::new_v4().to_string()] {
        assert!(matches!(
            store.admit(request(&id, None, &input), NOW),
            Err(Error::InvalidRequest)
        ));
    }
    for at in [NOW - DAY_MS - 1, NOW + 300_001] {
        assert!(matches!(
            store.admit(request(&request_id(at, 1), None, &input), NOW),
            Err(Error::RequestExpired)
        ));
    }
    for at in [NOW - DAY_MS, NOW + 300_000] {
        assert!(
            store
                .admit(request(&request_id(at, 2), None, &input), NOW)
                .unwrap()
                .created
        );
    }
    let mut auto = request("invalid", None, &input);
    auto.engine = "auto";
    assert!(matches!(store.admit(auto, NOW), Err(Error::InvalidRequest)));
}

#[test]
fn existing_thread_and_legacy_active_run_are_checked() {
    let mut conn = database();
    let input = json!({});
    let id = request_id(NOW, 1);
    run(&conn, "old", "t", "pi");
    let mut store = SubmissionStore::new(&mut conn);
    assert!(matches!(
        store.admit(request(&id, Some("missing"), &input), NOW),
        Err(Error::ThreadMissing)
    ));
    assert!(matches!(
        store.admit(request(&id, Some("t"), &input), NOW),
        Err(Error::ThreadBusy)
    ));
    assert!(store.admit(request(&id, None, &input), NOW).is_ok());
}

#[test]
fn cancellation_fences_late_results_and_releases_preparation_lock() {
    let mut conn = database();
    let input = json!({});
    let mut store = SubmissionStore::new(&mut conn);
    let a = store
        .admit(request(&request_id(NOW, 1), Some("t"), &input), NOW)
        .unwrap()
        .submission;
    assert!(matches!(
        store.admit(request(&request_id(NOW, 2), Some("t"), &input), NOW),
        Err(Error::ThreadBusy)
    ));
    let waiting = store.advance(&a.id, 0, State::Preparing, NOW + 1).unwrap();
    let waiting = store
        .advance(
            &a.id,
            waiting.generation,
            State::WaitingCredentials,
            NOW + 2,
        )
        .unwrap();
    let cancelled = store.cancel_preparation(&a.id, NOW + 3).unwrap();
    assert_eq!(cancelled.state, State::Cancelled);
    assert!(matches!(
        store.advance(&a.id, waiting.generation, State::Prepared, NOW + 4),
        Err(Error::Stale)
    ));
    assert_eq!(store.cancel_preparation(&a.id, NOW + 5).unwrap(), cancelled);
    assert!(store
        .admit(request(&request_id(NOW, 2), Some("t"), &input), NOW + 5)
        .is_ok());
}

#[test]
fn illegal_transitions_and_stale_receipts_never_dispatch() {
    let mut conn = database();
    let input = json!({});
    let mut store = SubmissionStore::new(&mut conn);
    let a = store
        .admit(request(&request_id(NOW, 1), None, &input), NOW)
        .unwrap()
        .submission;
    for next in [
        State::Prepared,
        State::Dispatching,
        State::Running,
        State::Succeeded,
        State::Cancelled,
    ] {
        assert_eq!(
            store.advance(&a.id, 0, next, NOW).unwrap_err(),
            Error::InvalidTransition
        );
    }
    assert_eq!(
        store.advance(&a.id, 1, State::Preparing, NOW).unwrap_err(),
        Error::Stale
    );
    assert_eq!(
        store
            .advance(&a.id, 0, State::Preparing, NOW - 1)
            .unwrap_err(),
        Error::Stale
    );
    assert_eq!(store.get(&a.id).unwrap().state, State::Accepted);
}

#[test]
fn run_binding_is_atomic_with_run_creation_and_prevents_prepare_cancel_after_commit() {
    let mut conn = database();
    let input = json!({});
    let prepared = {
        let mut store = SubmissionStore::new(&mut conn);
        let a = store
            .admit(request(&request_id(NOW, 1), Some("t"), &input), NOW)
            .unwrap()
            .submission;
        prepare(&mut store, &a.id)
    };
    {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        run(&tx, "rollback", "t", "pi");
        assert_eq!(
            bind_run(&tx, &prepared.id, prepared.generation, "rollback", NOW + 3)
                .unwrap()
                .state,
            State::Dispatching
        );
        // Simulate a later message/audit failure by dropping the transaction.
    }
    assert_eq!(
        SubmissionStore::new(&mut conn).get(&prepared.id).unwrap(),
        prepared
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    run(&tx, "run", "t", "pi");
    let bound = bind_run(&tx, &prepared.id, prepared.generation, "run", NOW + 3).unwrap();
    tx.commit().unwrap();
    let mut store = SubmissionStore::new(&mut conn);
    assert_eq!(
        store.cancel_preparation(&prepared.id, NOW + 4).unwrap_err(),
        Error::RequiresReconciliation
    );
    let running = store
        .advance(&bound.id, bound.generation, State::Running, NOW + 4)
        .unwrap();
    let finished = store
        .advance(&running.id, running.generation, State::Succeeded, NOW + 5)
        .unwrap();
    let replay = store
        .admit(request(&request_id(NOW, 1), Some("t"), &input), NOW + 6)
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.submission, finished);
}

#[test]
fn binding_rejects_wrong_engine_thread_missing_run_and_expired_preparation() {
    let mut conn = database();
    thread(&conn, "other");
    let input = json!({});
    let prepared = {
        let mut store = SubmissionStore::new(&mut conn);
        let a = store
            .admit(request(&request_id(NOW, 1), Some("t"), &input), NOW)
            .unwrap()
            .submission;
        prepare(&mut store, &a.id)
    };
    let tx = conn.transaction().unwrap();
    assert_eq!(
        bind_run(&tx, &prepared.id, prepared.generation, "missing", NOW + 3).unwrap_err(),
        Error::Missing
    );
    run(&tx, "wrong-engine", "t", "claude_code");
    run(&tx, "wrong-thread", "other", "pi");
    run(&tx, "valid", "t", "pi");
    for run in ["wrong-engine", "wrong-thread"] {
        assert_eq!(
            bind_run(&tx, &prepared.id, prepared.generation, run, NOW + 3).unwrap_err(),
            Error::RequestConflict
        );
    }
    assert_eq!(
        bind_run(
            &tx,
            &prepared.id,
            prepared.generation,
            "valid",
            NOW + 30_003
        )
        .unwrap_err(),
        Error::Stale
    );
    assert_eq!(
        bind_run(&tx, &prepared.id, 0, "valid", NOW + 3).unwrap_err(),
        Error::Stale
    );
}

#[test]
fn recovery_interrupts_preparation_but_preserves_uncertain_dispatch_and_its_lock() {
    let mut conn = database();
    thread(&conn, "other");
    let input = json!({});
    let (a, b) = {
        let mut store = SubmissionStore::new(&mut conn);
        let a = store
            .admit(request(&request_id(NOW, 1), Some("t"), &input), NOW)
            .unwrap()
            .submission;
        let b = store
            .admit(request(&request_id(NOW, 2), Some("other"), &input), NOW)
            .unwrap()
            .submission;
        (a, prepare(&mut store, &b.id))
    };
    let tx = conn.transaction().unwrap();
    run(&tx, "run", "other", "pi");
    bind_run(&tx, &b.id, b.generation, "run", NOW + 3).unwrap();
    tx.commit().unwrap();
    let mut store = SubmissionStore::new(&mut conn);
    assert_eq!(store.recover_after_host_exit(NOW + 4).unwrap(), 2);
    assert_eq!(store.get(&a.id).unwrap().state, State::Interrupted);
    assert_eq!(store.get(&b.id).unwrap().state, State::Recovering);
    assert_eq!(store.recover_after_host_exit(NOW + 5).unwrap(), 0);
    assert!(matches!(
        store.admit(request(&request_id(NOW, 3), Some("other"), &input), NOW + 5),
        Err(Error::ThreadBusy)
    ));
    assert_eq!(
        store
            .advance(&a.id, 0, State::Preparing, NOW + 5)
            .unwrap_err(),
        Error::Stale
    );
}

#[test]
fn tombstones_survive_thread_deletion_and_pruning_cannot_reexecute_old_request() {
    let mut conn = database();
    let input = json!({"message":"secret"});
    let id = request_id(NOW, 1);
    let a = {
        let mut store = SubmissionStore::new(&mut conn);
        let a = store
            .admit(request(&id, Some("t"), &input), NOW)
            .unwrap()
            .submission;
        assert_eq!(
            store.tombstone(&a.id, NOW).unwrap_err(),
            Error::RequiresReconciliation
        );
        store.cancel_preparation(&a.id, NOW + 1).unwrap();
        store.tombstone(&a.id, NOW + 2).unwrap();
        a
    };
    conn.execute("DELETE FROM agent_threads WHERE id='t'", [])
        .unwrap();
    let mut store = SubmissionStore::new(&mut conn);
    assert!(store.get(&a.id).unwrap().gone);
    assert!(matches!(
        store.admit(request(&id, Some("t"), &input), NOW + 3),
        Err(Error::RequestGone)
    ));
    assert_eq!(store.prune_tombstones(NOW + RETENTION_MS).unwrap(), 0);
    assert_eq!(store.prune_tombstones(NOW + RETENTION_MS + 3).unwrap(), 1);
    assert!(matches!(
        store.admit(request(&id, Some("t"), &input), NOW + RETENTION_MS + 4),
        Err(Error::RequestExpired)
    ));
}

#[test]
fn concurrent_connections_admit_only_one_preparation_for_a_thread() {
    let dir = TestDirectory::new();
    let path = dir.path().join("submissions.db");
    let conn = Connection::open(&path).unwrap();
    crate::db::create_schema(&conn).unwrap();
    thread(&conn, "t");
    drop(conn);
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (1..=2)
        .map(|n| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                barrier.wait();
                SubmissionStore::new(&mut conn)
                    .admit(request(&request_id(NOW, n), Some("t"), &json!({})), NOW)
                    .map(|r| r.created)
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| matches!(r, Ok(true))).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::ThreadBusy)))
            .count(),
        1
    );
}

#[test]
fn concurrent_identical_new_thread_request_replays_one_identity() {
    let dir = TestDirectory::new();
    let path = dir.path().join("submissions.db");
    let conn = Connection::open(&path).unwrap();
    crate::db::create_schema(&conn).unwrap();
    drop(conn);
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                barrier.wait();
                let a = SubmissionStore::new(&mut conn)
                    .admit(request(&request_id(NOW, 1), None, &json!({})), NOW)
                    .unwrap();
                (a.created, a.submission.id)
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_ne!(results[0].0, results[1].0);
    assert_eq!(results[0].1, results[1].1);
}

#[test]
fn new_thread_binding_rejects_active_or_finished_run_and_keeps_request_identity() {
    let mut conn = database();
    let id = request_id(NOW, 1);
    let input = json!({});
    let prepared = {
        let mut store = SubmissionStore::new(&mut conn);
        let a = store
            .admit(request(&id, None, &input), NOW)
            .unwrap()
            .submission;
        prepare(&mut store, &a.id)
    };
    let tx = conn.transaction().unwrap();
    run(&tx, "new", "t", "pi");
    run(&tx, "overlap", "t", "pi");
    assert_eq!(
        bind_run(&tx, &prepared.id, prepared.generation, "new", NOW + 3).unwrap_err(),
        Error::ThreadBusy
    );
    tx.execute("UPDATE agent_runs SET status='completed'", [])
        .unwrap();
    assert_eq!(
        bind_run(&tx, &prepared.id, prepared.generation, "new", NOW + 3).unwrap_err(),
        Error::InvalidTransition
    );
    tx.execute("UPDATE agent_runs SET status='queued' WHERE id='new'", [])
        .unwrap();
    let bound = bind_run(&tx, &prepared.id, prepared.generation, "new", NOW + 3).unwrap();
    tx.commit().unwrap();
    assert_eq!(bound.thread_id.as_deref(), Some("t"));
    let replay = SubmissionStore::new(&mut conn)
        .admit(request(&id, None, &input), NOW + 4)
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.submission, bound);
}

#[test]
fn reopened_database_preserves_request_and_cancel_fence() {
    let dir = TestDirectory::new();
    let path = dir.path().join("durable.db");
    let input = json!({"message":"recover me"});
    let id = request_id(NOW, 1);
    let before = {
        let mut conn = Connection::open(&path).unwrap();
        crate::db::create_schema(&conn).unwrap();
        let mut store = SubmissionStore::new(&mut conn);
        let a = store
            .admit(request(&id, None, &input), NOW)
            .unwrap()
            .submission;
        store.advance(&a.id, 0, State::Preparing, NOW + 1).unwrap()
    };
    let mut conn = Connection::open(path).unwrap();
    crate::db::create_schema(&conn).unwrap();
    let mut store = SubmissionStore::new(&mut conn);
    assert_eq!(store.get(&before.id).unwrap(), before);
    let cancelled = store.cancel_preparation(&before.id, NOW + 2).unwrap();
    assert_eq!(
        store
            .advance(&before.id, before.generation, State::Prepared, NOW + 3)
            .unwrap_err(),
        Error::Stale
    );
    let replay = store.admit(request(&id, None, &input), NOW + 3).unwrap();
    assert!(!replay.created);
    assert_eq!(replay.submission, cancelled);
}
