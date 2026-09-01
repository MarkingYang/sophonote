//! H3 / NEXT-020：Hermes 只读 Run（协议 stub + Adapter 内 list/read）。
#![cfg(feature = "legacy-hermes-fixture")]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use sophonote_lib::agent::engine::{AgentEngine, RunEnvelope};
use sophonote_lib::agent::events::EventEmitter;
use sophonote_lib::agent::hermes::{
    file_sha256_hex, HermesReadonlyEngine, HermesRunsClient, HermesSidecarConfig,
};
use sophonote_lib::agent::run_controller::SpikeParams;
use sophonote_lib::agent::store::{RunStore, RunStoreTransport};
use sophonote_lib::model::gateway::ModelGateway;
use sophonote_lib::model::messages::{ModelError, ModelRequest, ModelResponse};
use sophonote_lib::tools::documents::ProposeDocumentPatchTool;
use sophonote_lib::tools::project::{ListProjectDocumentsTool, ReadDocumentTool};
use sophonote_lib::tools::ToolRegistry;

fn stub_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hermes_health_stub"))
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sophonote-hermes-h3-{tag}-{}", uuid::Uuid::new_v4()));
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
        Err(ModelError::Config("H3 readonly engine 不调用模型".into()))
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

fn spike_params(user: &str) -> SpikeParams {
    SpikeParams {
        system: None,
        history: Vec::new(),
        user: user.into(),
        max_turns: 4,
        temperature: Some(0.0),
        run_id: Some("mb-run-h3".into()),
        prompt_version: "hermes-readonly@v1".into(),
        run_context: None,
        run_skill: None,
        max_tool_calls: None,
    }
}

#[tokio::test]
async fn runs_api_create_and_stop() {
    let home = temp_dir("sse");
    let config = HermesSidecarConfig::for_binary(stub_bin(), home.clone()).unwrap();
    let handle = sophonote_lib::agent::hermes::start_sidecar(&config)
        .await
        .expect("start");
    let client = HermesRunsClient::new(handle.base_url.clone(), handle.bearer.clone());
    let created = client
        .create_run("hello", None, Some("a1"))
        .await
        .expect("create");
    assert!(created.run_id.starts_with("run-"));
    let _ = client.stop(&created.run_id).await;
    handle.shutdown().unwrap();
    let _ = fs::remove_dir_all(home);
}

#[tokio::test]
async fn readonly_run_list_read_preserves_notes_and_runstore() {
    let fx = ProjectFixture::setup();
    let note_path = fx.notes.join("a1.md");
    let before = file_sha256_hex(&note_path).unwrap();

    let hermes_home = fx.root.join("hermes-home");
    let config = HermesSidecarConfig::for_binary(stub_bin(), hermes_home).unwrap();
    let engine = HermesReadonlyEngine::new(config);

    let agent_db = fx.root.join("agent.db");
    {
        let conn = rusqlite::Connection::open(&agent_db).unwrap();
        sophonote_lib::db::create_schema(&conn).unwrap();
        let store = RunStore::new(conn);
        store
            .create_thread("th1", "H3", Some("p1"), 1)
            .expect("thread");
        store
            .create_run(
                "mb-run-h3",
                "th1",
                Some("p1"),
                "hermes-stub",
                "stub",
                Some("hermes-readonly@v1"),
                4,
                1,
            )
            .expect("run");
    }

    let transport = Arc::new(RunStoreTransport::new(
        agent_db.to_string_lossy().to_string(),
    ));
    let emitter = Arc::new(EventEmitter::new("th1", "mb-run-h3", transport));

    let pack = serde_json::json!({
        "searchHits": [{"title": "测试笔记", "score": 0.9}],
        "evidenceSummaries": ["[E1] 夹具证据"],
        "articleIdHint": "a1",
        "marker": "CTX_PACK_H3_OK"
    });

    let report = engine
        .run_with_events(RunEnvelope {
            gateway: Arc::new(UnusedGateway),
            registry: Arc::new(fx.readonly_registry()),
            params: spike_params("请列出并阅读项目文档"),
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
        .expect("readonly run");

    assert_eq!(report.outcome, "completed");
    assert!(
        report.final_answer.contains("CTX_PACK_H3_OK"),
        "终态应回显 context_pack: {}",
        report.final_answer
    );
    assert!(
        report.final_answer.contains("list") || report.final_answer.contains("只读"),
        "应体现只读工具路径"
    );

    let after = file_sha256_hex(&note_path).unwrap();
    assert_eq!(before, after, "notes 不得被修改");

    let conn = rusqlite::Connection::open(&agent_db).unwrap();
    let store = RunStore::new(conn);
    let events = store.all_events_of_run("mb-run-h3").expect("events");
    assert!(!events.is_empty(), "RunStore 应有事件");
    let joined = events.join("\n");
    assert!(
        joined.contains("run_started")
            || joined.contains("RunStarted")
            || joined.contains("list_project"),
        "应含 started 或工具事件: {joined}"
    );
    assert!(
        joined.contains("run_completed")
            || joined.contains("completed")
            || joined.contains("list_project_documents"),
        "应含终态或工具名: {joined}"
    );
}

#[tokio::test]
async fn rejects_registry_with_write_tools() {
    let fx = ProjectFixture::setup();
    let hermes_home = fx.root.join("hermes-home2");
    let config = HermesSidecarConfig::for_binary(stub_bin(), hermes_home).unwrap();
    let engine = HermesReadonlyEngine::new(config);

    let mut reg = fx.readonly_registry();
    reg.register(Arc::new(ProposeDocumentPatchTool::new(
        fx.db_path.clone(),
        fx.notes.clone(),
        "p1".into(),
        "r1".into(),
    )));

    let err = engine
        .run_with_events(RunEnvelope {
            gateway: Arc::new(UnusedGateway),
            registry: Arc::new(reg),
            params: spike_params("x"),
            cancel: CancellationToken::new(),
            events: None,
            observer: None,
            context_pack: None,
            model_route: None,
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
        .expect_err("must refuse write tools");
    assert!(
        err.to_string().contains("写工具") || err.to_string().contains("propose"),
        "{err}"
    );
}

#[test]
fn readonly_assert_helper_exported() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ListProjectDocumentsTool::new(
        PathBuf::from("/tmp/x.db"),
        "p".into(),
    )));
    assert!(sophonote_lib::agent::hermes::assert_readonly_registry(&reg).is_ok());
}
