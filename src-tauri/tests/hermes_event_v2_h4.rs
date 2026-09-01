//! H4 / NEXT-021：AgentEvent v2 + SSE 重连/对账（协议 stub）。
//! 禁止假 completed；用户只装 SophoNote，Hermes 为内嵌 sidecar。
#![cfg(feature = "legacy-hermes-fixture")]

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use sophonote_lib::agent::engine::{AgentEngine, RunEnvelope};
use sophonote_lib::agent::events::EventEmitter;
use sophonote_lib::agent::hermes::{
    consume_with_recovery, file_sha256_hex, HermesReadonlyEngine, HermesRunsClient,
    HermesSidecarConfig, RecoveryOutcome,
};
use sophonote_lib::agent::run_controller::SpikeParams;
use sophonote_lib::agent::store::{RunStore, RunStoreTransport};
use sophonote_lib::model::gateway::ModelGateway;
use sophonote_lib::model::messages::{ModelError, ModelRequest, ModelResponse};
use sophonote_lib::tools::project::{ListProjectDocumentsTool, ReadDocumentTool};
use sophonote_lib::tools::ToolRegistry;

fn stub_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hermes_health_stub"))
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sophonote-hermes-h4-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).expect("temp");
    dir
}

struct UnusedGateway;

#[async_trait]
impl ModelGateway for UnusedGateway {
    async fn complete(
        &self,
        _request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Config("H4 不调用模型".into()))
    }
}

struct ProjectFixture {
    root: PathBuf,
    db_path: PathBuf,
    notes: PathBuf,
}

impl ProjectFixture {
    fn setup() -> Self {
        let root = temp_dir("fx");
        let notes = root.join("notes");
        fs::create_dir_all(&notes).unwrap();
        let db_path = root.join("sophonote.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        sophonote_lib::db::create_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', '项目一')",
            [],
        )
        .unwrap();
        for (id, title) in [("a1", "测试笔记"), ("a2", "第二篇")] {
            conn.execute(
                "INSERT INTO articles (id, title, content, article_type) VALUES (?1, ?2, '', 'manual')",
                rusqlite::params![id, title],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id, added_at) VALUES ('p1', ?1, 1)",
                rusqlite::params![id],
            )
            .unwrap();
            fs::write(
                notes.join(format!("{id}.md")),
                format!("---\nid: {id}\ntitle: \"{title}\"\n---\n\n正文-{id}\n"),
            )
            .unwrap();
        }
        Self {
            root,
            db_path,
            notes,
        }
    }

    fn readonly_registry(&self) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ListProjectDocumentsTool::new(
            self.db_path.clone(),
            "p1".into(),
        )));
        reg.register(Arc::new(ReadDocumentTool::new(
            self.db_path.clone(),
            self.notes.clone(),
            "p1".into(),
        )));
        reg
    }
}

impl Drop for ProjectFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn spike_params(user: &str, run_id: &str) -> SpikeParams {
    SpikeParams {
        system: None,
        history: Vec::new(),
        user: user.into(),
        max_turns: 4,
        temperature: Some(0.0),
        run_id: Some(run_id.into()),
        prompt_version: "hermes-h4@v1".into(),
        run_context: None,
        run_skill: None,
        max_tool_calls: None,
    }
}

#[test]
fn agent_runs_has_external_columns() {
    let dir = temp_dir("db");
    let db = dir.join("t.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    sophonote_lib::db::create_schema(&conn).unwrap();
    let mut stmt = conn.prepare("PRAGMA table_info(agent_runs)").unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    for col in [
        "engine_transport",
        "external_run_id",
        "external_session_id",
        "external_protocol_version",
        "last_external_event_id",
    ] {
        assert!(names.iter().any(|n| n == col), "missing column {col}");
    }
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn sse_resume_after_drop_completes_without_fake_status() {
    let fx = ProjectFixture::setup();
    let note_path = fx.notes.join("a1.md");
    let before = file_sha256_hex(&note_path).unwrap();

    let hermes_home = fx.root.join("hermes-home");
    let config = HermesSidecarConfig::for_binary(stub_bin(), hermes_home)
        .unwrap()
        .with_stub_env("HERMES_STUB_DROP_AFTER", "2");
    let engine = HermesReadonlyEngine::new(config);

    let agent_db = fx.root.join("agent.db");
    {
        let conn = rusqlite::Connection::open(&agent_db).unwrap();
        sophonote_lib::db::create_schema(&conn).unwrap();
        let store = RunStore::new(conn);
        store.create_thread("th1", "H4", Some("p1"), 1).unwrap();
        store
            .create_run(
                "mb-run-h4",
                "th1",
                Some("p1"),
                "hermes-stub",
                "stub",
                Some("hermes-h4@v1"),
                4,
                1,
            )
            .unwrap();
        store
            .update_run_external_meta(
                "mb-run-h4",
                Some("http+sse"),
                None,
                None,
                Some("stub-0.3.0"),
                None,
                1,
            )
            .unwrap();
    }

    let transport = Arc::new(RunStoreTransport::new(
        agent_db.to_string_lossy().to_string(),
    ));
    let emitter = Arc::new(EventEmitter::new("th1", "mb-run-h4", transport));

    let pack = serde_json::json!({
        "articleIdHint": "a1",
        "marker": "CTX_PACK_H4_OK"
    });

    let report = engine
        .run_with_events(RunEnvelope {
            gateway: Arc::new(UnusedGateway),
            registry: Arc::new(fx.readonly_registry()),
            params: spike_params("列出并阅读", "mb-run-h4"),
            cancel: CancellationToken::new(),
            events: Some(emitter),
            observer: None,
            context_pack: Some(pack),
            model_route: Some(sophonote_lib::sophonote_mcp::ModelRoute::deepseek_default()),
            hermes_session_id: None,
            hermes_memory_scope_key: None,
            hermes_input: None,
            hermes_model: None,
            hermes_provider: None,
            hermes_command: None,
            hermes_attachments: Vec::new(),
            hermes_focus_document: None,
            hermes_session_binding: None,
        })
        .await
        .expect("run");

    assert_eq!(report.outcome, "completed");
    assert!(report.final_answer.contains("CTX_PACK_H4_OK"));
    assert_eq!(file_sha256_hex(&note_path).unwrap(), before);

    let conn = rusqlite::Connection::open(&agent_db).unwrap();
    let store = RunStore::new(conn);
    let events = store.all_events_of_run("mb-run-h4").expect("events");
    let joined = events.join("\n");
    assert!(
        joined.contains("message_delta"),
        "expected message_delta in events: {joined}"
    );
    assert!(
        joined.contains("engine_degraded"),
        "expected engine_degraded after drop: {joined}"
    );
    assert!(joined.contains("run_completed"));
    assert!(
        !joined.contains("\"outcome\":\"interrupted\""),
        "successful resume must not interrupt: {joined}"
    );
}

#[tokio::test]
async fn sse_exhausted_while_running_is_interrupted_not_completed() {
    let home = temp_dir("intr");
    let config = HermesSidecarConfig::for_binary(stub_bin(), home.clone())
        .unwrap()
        .with_stub_env("HERMES_STUB_DROP_AFTER", "1")
        .with_stub_env("HERMES_STUB_IDLE_CLOSE", "1");
    let handle = sophonote_lib::agent::hermes::start_sidecar(&config)
        .await
        .expect("start");
    let client = HermesRunsClient::new(handle.base_url.clone(), handle.bearer.clone());
    let created = client
        .create_run("stuck", None, None)
        .await
        .expect("create");

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_cb = seen.clone();
    let mut last_id = None;
    let outcome = consume_with_recovery(&client, &created.run_id, &None, &mut last_id, |ev| {
        let seen_cb = seen_cb.clone();
        async move {
            if let Some(t) = ev.get("type").and_then(|v| v.as_str()) {
                seen_cb.lock().unwrap().push(t.to_string());
            }
            Ok(())
        }
    })
    .await
    .expect("recovery");

    match outcome {
        RecoveryOutcome::Interrupted { reason } => {
            assert!(
                reason.contains("不可恢复") || reason.contains("sse_"),
                "reason={reason}"
            );
        }
        other => panic!("expected Interrupted, got {other:?}"),
    }

    let status = client.get_run(&created.run_id).await.expect("get");
    assert_eq!(status.status, "running");
    assert!(status.output.is_empty());

    handle.shutdown().unwrap();
    let _ = fs::remove_dir_all(home);
}

#[tokio::test]
async fn stream_events_from_parses_sse_id() {
    let home = temp_dir("id");
    let config = HermesSidecarConfig::for_binary(stub_bin(), home.clone())
        .unwrap()
        .with_stub_env("HERMES_STUB_DROP_AFTER", "1");
    let handle = sophonote_lib::agent::hermes::start_sidecar(&config)
        .await
        .expect("start");
    let client = HermesRunsClient::new(handle.base_url.clone(), handle.bearer.clone());
    let created = client.create_run("ids", None, Some("a1")).await.unwrap();

    let ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let ids_cb = ids.clone();
    let result = client
        .stream_events_from(&created.run_id, None, |id, _ev| {
            let ids_cb = ids_cb.clone();
            async move {
                if let Some(i) = id {
                    ids_cb.lock().unwrap().push(i);
                }
                Ok(())
            }
        })
        .await
        .unwrap();

    assert!(matches!(
        result,
        sophonote_lib::agent::hermes::SseStreamResult::ConnectionEnded { .. }
    ));
    let captured = ids.lock().unwrap().clone();
    assert_eq!(captured, vec!["0".to_string()]);

    let _ = client.stop(&created.run_id).await;
    handle.shutdown().unwrap();
    let _ = fs::remove_dir_all(home);
}
