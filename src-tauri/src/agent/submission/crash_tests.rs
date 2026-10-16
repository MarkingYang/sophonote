//! Process-death tests for the real SQLite claim boundary. No native model is run.
use super::*;
use crate::agent::{
    automatic::Fence, commands::AgentRunStartArgs, registry::EngineId, routing, store::RunStore,
    task_profile::TaskProfile, types::RunStatus,
};
use serde_json::json;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "agent::submission::live::crash_tests::crash_worker";
const INPUT: &str = "只读验收：翻译测试材料，不修改文件";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/submission-crash-tests")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn db(&self) -> PathBuf {
        self.0.join("test.db")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn request() -> AgentRunStartArgs {
    serde_json::from_value(json!({"threadId":"t","message":INPUT,"workspacePermissionMode":"plan"}))
        .unwrap()
}
fn stop_here(dir: &Path, submission: &str, request_id: &str) -> ! {
    std::fs::write(
        dir.join("ready.tmp"),
        json!({"submission":submission,"request":request_id}).to_string(),
    )
    .unwrap();
    std::fs::rename(dir.join("ready.tmp"), dir.join("ready.json")).unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
#[ignore = "private worker launched and killed by crash-window tests"]
fn crash_worker() {
    let Ok(dir) = std::env::var("SOPHONOTE_CRASH_FIXTURE") else {
        return;
    };
    let phase = std::env::var("SOPHONOTE_CRASH_PHASE").unwrap();
    let dir = PathBuf::from(dir);
    let db = dir.join("test.db");
    let conn = Connection::open(&db).unwrap();
    crate::db::create_schema(&conn).unwrap();
    let store = RunStore::new(conn);
    let now = now_ms() as u64;
    store.create_thread("t", "进程恢复夹具", None, now).unwrap();
    store
        .with_run_claim("t", "pi", || {
            store.create_run("history", "t", None, "fixture", "model", None, 6, now)?;
            store.set_run_engine("history", "pi", "fixture")?;
            store.save_message(
                "old-message",
                "t",
                "history",
                "assistant",
                "已有历史不可丢失",
                now,
            )
        })
        .unwrap();
    store
        .update_run_status("history", &RunStatus::Completed, now)
        .unwrap();
    // The old native session must survive every recovery window.
    store
        .bind_thread_external_session("t", "native-history", now)
        .unwrap();
    let stamp = format!("{:012x}", now_ms());
    let request_id = format!("{}-{}-7000-8000-000000000001", &stamp[..8], &stamp[8..]);
    if phase == "admission_rejected" {
        let busy_id = request_id.replace("000000000001", "000000000002");
        let (_busy, busy_lease) = LiveSubmission::admit(&db, &request(), "auto", &busy_id).unwrap();
        assert!(busy_lease.is_some());
        let (rejected, lease) =
            LiveSubmission::admit(&db, &request(), "auto", &request_id).unwrap();
        assert_eq!(rejected.state, State::Failed);
        assert!(lease.is_none());
        stop_here(&dir, &rejected.id, &request_id);
    }
    let (submission, lease) = LiveSubmission::admit(&db, &request(), "auto", &request_id).unwrap();
    let mut lease = lease.unwrap();
    lease.select_engine(EngineId::Pi).unwrap();
    if phase == "preparing" {
        stop_here(&dir, &submission.id, &request_id);
    }
    let mut route = routing::resolve_from_store(&store, Some("t"), None).unwrap();
    route.automatic(
        EngineId::Pi, Fence::capture(&Connection::open(&db).unwrap()).unwrap(),
        serde_json::from_value(json!({"policyVersion":"general-v1","recommendedPreparationEngine":"pi","automaticExecution":false,"probes":[],"elapsedMs":0})).unwrap(),
        TaskProfile::TextResponse, EngineId::Pi,
    );
    let now = now_ms() as u64;
    let evidence = routing::CapabilitySnapshot::new(
        EngineId::Pi,
        None,
        routing::PreparationEvidence::Unavailable,
        Some(vec![]),
        now,
    );
    store
        .with_submission_claim("t", "pi", Some(&lease.receipt), || {
            store.create_run("new-run", "t", None, "fixture", "model", None, 6, now)?;
            store.set_run_engine("new-run", "pi", "fixture")?;
            store.save_message("new-message", "t", "new-run", "user", INPUT, now)?;
            store.record_route("new-run", &route, &evidence)?;
            store.bind_submission("new-run", Some(&lease.receipt), now)?;
            if phase == "claim_open" {
                stop_here(&dir, &submission.id, &request_id);
            }
            Ok(())
        })
        .unwrap();
    if phase == "ack_unpersisted" {
        // External evidence stands for a native ack that has not reached Host DB.
        // Recovery must not infer "not executed" from the missing DB ack.
        std::fs::write(dir.join("native-ack"), "fixture-ack").unwrap();
    }
    if phase == "terminal" {
        store
            .save_message(
                "final-message",
                "t",
                "new-run",
                "assistant",
                "验收答案保持不变",
                now + 1,
            )
            .unwrap();
        store
            .update_run_status("new-run", &RunStatus::Completed, now + 1)
            .unwrap();
    }
    stop_here(&dir, &submission.id, &request_id);
}

fn check_crash(phase: &str) {
    let fixture = Fixture::new();
    let mut worker = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("SOPHONOTE_CRASH_FIXTURE", &fixture.0)
            .env("SOPHONOTE_CRASH_PHASE", phase)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let ready = fixture.0.join("ready.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "worker exited before {phase}"
        );
        assert!(Instant::now() < deadline, "worker did not reach {phase}");
        std::thread::sleep(Duration::from_millis(10));
    }
    // A second Host must not recover an owner that is still alive.
    assert!(acquire_owner(&fixture.db()).is_err());
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    let ids: serde_json::Value = serde_json::from_slice(&std::fs::read(ready).unwrap()).unwrap();
    let id = ids["submission"].as_str().unwrap();
    let request_id = ids["request"].as_str().unwrap();
    let owner = acquire_owner(&fixture.db()).unwrap();
    let mut conn = Connection::open(fixture.db()).unwrap();
    let state = SubmissionStore::new(&mut conn).get(id).unwrap();
    let committed = !matches!(phase, "preparing" | "claim_open" | "admission_rejected");
    let expected = if phase == "admission_rejected" {
        State::Failed
    } else if phase == "terminal" {
        State::Succeeded
    } else if committed {
        State::Recovering
    } else {
        State::Interrupted
    };
    assert_eq!(state.state, expected, "{phase}");
    assert_eq!(state.run_id.as_deref(), committed.then_some("new-run"));
    let recovery: Option<String> = conn
        .query_row(
            "SELECT recovery_json FROM agent_submissions WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        recovery.is_some(),
        !committed,
        "{phase}: draft follows atomic claim"
    );
    if let Some(recovery) = recovery {
        let restored: crate::agent::commands::AgentRunStartArgs =
            serde_json::from_str(&recovery).unwrap();
        assert_eq!(restored.message, request().message);
    }

    let count = |table: &str| {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    assert_eq!(count("agent_runs"), if committed { 2 } else { 1 });
    let messages = if phase == "terminal" {
        3
    } else if committed {
        2
    } else {
        1
    };
    assert_eq!(count("agent_messages"), messages);
    assert_eq!(
        fixture.0.join("native-ack").exists(),
        phase == "ack_unpersisted"
    );
    let store = RunStore::new(Connection::open(fixture.db()).unwrap());
    assert_eq!(
        store
            .external_session_id_for_thread("t")
            .unwrap()
            .as_deref(),
        Some("native-history")
    );
    assert_eq!(store.get_thread("t").unwrap().unwrap().engine, "pi");
    let history: String = conn
        .query_row(
            "SELECT content FROM agent_messages WHERE id='old-message'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, "已有历史不可丢失");
    if committed {
        let raw: String = conn
            .query_row(
                "SELECT route_decision_json FROM agent_runs WHERE id='new-run'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let audit: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(audit["decision"]["policyVersion"], "automatic-v3");
        assert_eq!(audit["decision"]["selectedEngine"], "pi");
        assert_eq!(audit["decision"]["source"], "session_affinity");
    }
    drop(owner);
    // Lost reply/restart replays the same submission; it never grants a new lease.
    let (replay, lease) =
        LiveSubmission::admit(&fixture.db(), &request(), "auto", request_id).unwrap();
    assert!(lease.is_none());
    assert_eq!(replay.id, id);
    assert_eq!(replay.state, expected);
    if phase == "terminal" {
        let answer: String = conn
            .query_row(
                "SELECT content FROM agent_messages WHERE id='final-message'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(answer, "验收答案保持不变");
    }
    let next = request_id.replace("000000000001", "000000000003");
    let retry = LiveSubmission::admit(&fixture.db(), &request(), "auto", &next);
    if expected == State::Recovering {
        let (rejected, lease) = retry.unwrap();
        assert_eq!(rejected.state, State::Failed);
        assert!(rejected.run_id.is_none());
        assert!(lease.is_none());
    } else {
        assert!(retry.unwrap().1.is_some());
    }
    assert_eq!(count("agent_runs"), if committed { 2 } else { 1 });
    assert_eq!(count("agent_messages"), messages);
}

#[test]
fn process_death_during_preparation_preserves_history_and_allows_explicit_retry() {
    check_crash("preparing");
}
#[test]
fn process_death_inside_claim_rolls_back_run_message_and_audit_together() {
    check_crash("claim_open");
}
#[test]
fn process_death_after_claim_keeps_unknown_dispatch_locked() {
    check_crash("dispatch_unknown");
}
#[test]
fn process_death_after_external_ack_never_replays_unpersisted_ack() {
    check_crash("ack_unpersisted");
}
#[test]
fn process_death_after_terminal_recovers_same_result_without_another_run() {
    check_crash("terminal");
}

#[test]
fn process_death_after_admission_rejection_keeps_draft_without_restarting_it() {
    check_crash("admission_rejected");
}
