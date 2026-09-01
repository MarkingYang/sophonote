//! H7 / NEXT-024：黄金任务双跑 Rig vs Hermes（安全边界与终态类别）。
#![cfg(feature = "legacy-hermes-fixture")]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use sophonote_lib::agent::engine::{AgentEngine, RigAgentEngine, RunEnvelope};
use sophonote_lib::agent::hermes::{file_sha256_hex, HermesReadonlyEngine, HermesSidecarConfig};
use sophonote_lib::agent::run_controller::SpikeParams;
use sophonote_lib::sophonote_mcp::{
    bridge_patch_registry, issue_lease, BridgeInvokeRequest, LeaseRegistry, SophonoteBridge,
    ModelRoute, BRIDGE_PATCH_TOOL,
};
use sophonote_lib::model::gateway::ModelGateway;
use sophonote_lib::model::messages::{FinishReason, ModelError, ModelRequest, ModelResponse};
use sophonote_lib::tools::builtin::spike_registry;
use sophonote_lib::tools::project::{ListProjectDocumentsTool, ReadDocumentTool};
use sophonote_lib::tools::ToolRegistry;

fn stub_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hermes_health_stub"))
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sophonote-hermes-h7-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 假网关：直接返回终态文本（无工具），用于 Rig 侧黄金对照。
struct FinalOnlyGateway;

#[async_trait]
impl ModelGateway for FinalOnlyGateway {
    async fn complete(
        &self,
        _request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse {
            content: "黄金任务完成（Rig）".into(),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: Default::default(),
            reasoning: None,
            provider_request_id: None,
        })
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
        conn.execute("INSERT INTO projects (id, name) VALUES ('p1', '项目')", [])
            .unwrap();
        let (id, title) = ("a1", "笔记一");
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
            format!("---\nid: {id}\ntitle: \"{title}\"\n---\n\n正文\n"),
        )
        .unwrap();
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
        prompt_version: "hermes-h7@v1".into(),
        run_context: None,
        run_skill: None,
        max_tool_calls: None,
    }
}

#[tokio::test]
async fn dual_run_both_complete_without_touching_notes() {
    let fx = ProjectFixture::setup();
    let note_path = fx.notes.join("a1.md");
    let before = file_sha256_hex(&note_path).unwrap();

    // --- Hermes 只读路径 ---
    let hermes_home = fx.root.join("hermes-home");
    let config = HermesSidecarConfig::for_binary(stub_bin(), hermes_home).unwrap();
    let hermes = HermesReadonlyEngine::new(config);
    let hermes_report = hermes
        .run_with_events(RunEnvelope {
            gateway: Arc::new(FinalOnlyGateway),
            registry: Arc::new(fx.readonly_registry()),
            params: spike_params("列出并阅读", "h7-hermes"),
            cancel: CancellationToken::new(),
            events: None,
            observer: None,
            context_pack: Some(serde_json::json!({"articleIdHint": "a1", "marker": "H7"})),
            model_route: Some(ModelRoute::deepseek_default()),
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
        .expect("hermes");

    // --- Rig 路径（假网关终态）---
    let rig = RigAgentEngine;
    let rig_report = rig
        .run_with_events(RunEnvelope {
            gateway: Arc::new(FinalOnlyGateway),
            registry: Arc::new(spike_registry()),
            params: spike_params("黄金任务", "h7-rig"),
            cancel: CancellationToken::new(),
            events: None,
            observer: None,
            context_pack: None,
            model_route: Some(ModelRoute::deepseek_default()),
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
        .expect("rig");

    assert_eq!(hermes_report.outcome, "completed");
    assert_eq!(rig_report.outcome, "completed");
    assert_eq!(file_sha256_hex(&note_path).unwrap(), before);
    assert_eq!(hermes.engine_id(), "hermes");
    assert_eq!(rig.engine_id(), "rig");
}

#[tokio::test]
async fn dual_run_write_boundary_bridge_requires_lease() {
    let fx = ProjectFixture::setup();
    let bridge = SophonoteBridge::new(LeaseRegistry::new());
    let tools = bridge_patch_registry(fx.db_path.clone(), fx.notes.clone(), "p1", "h7-run");
    // 无 Lease → 拒绝（Hermes 安全边界）
    let err = bridge
        .invoke_with_tools(
            BridgeInvokeRequest {
                lease_id: "nope".into(),
                tool_name: BRIDGE_PATCH_TOOL.into(),
                arguments: serde_json::json!({}),
                claimed_project_id: None,
                claimed_run_id: None,
            },
            &tools,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("不存在") || err.to_string().contains("lease"));

    // 有 Lease → 可授权（Rig 侧写工具同样需项目绑定；此处对照 Bridge 门禁）
    let lease = issue_lease(
        "h7-run",
        "p1",
        [BRIDGE_PATCH_TOOL],
        ModelRoute::deepseek_default(),
        60_000,
    );
    let id = lease.lease_id.clone();
    bridge.register_lease(lease);
    let auth = bridge.invoke(BridgeInvokeRequest {
        lease_id: id,
        tool_name: BRIDGE_PATCH_TOOL.into(),
        arguments: serde_json::json!({}),
        claimed_project_id: Some("p1".into()),
        claimed_run_id: Some("h7-run".into()),
    });
    assert!(auth.is_ok());
}
