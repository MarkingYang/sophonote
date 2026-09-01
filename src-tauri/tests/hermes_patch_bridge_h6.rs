//! H6 / NEXT-023：propose_document_patch 经 sophonote-bridge + Lease（dry-run）。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use sophonote_lib::sophonote_mcp::{
    bridge_patch_registry, issue_lease, BridgeInvokeRequest, LeaseError, LeaseRegistry,
    SophonoteBridge, ModelRoute, BRIDGE_PATCH_TOOL,
};
use sophonote_lib::tools::documents::ProposeDocumentPatchTool;
use sophonote_lib::tools::ToolRegistry;

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sophonote-hermes-h6-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

struct PatchFixture {
    root: PathBuf,
    db_path: PathBuf,
    notes: PathBuf,
    article_id: String,
}

impl PatchFixture {
    fn setup() -> Self {
        let root = temp_dir("fx");
        let notes = root.join("notes");
        fs::create_dir_all(&notes).unwrap();
        let db_path = root.join("sophonote.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        sophonote_lib::db::create_schema(&conn).unwrap();
        conn.execute("INSERT INTO projects (id, name) VALUES ('p1', '项目')", [])
            .unwrap();
        let article_id = "a1".to_string();
        conn.execute(
            "INSERT INTO articles (id, title, content, article_type, version) VALUES (?1, '笔记', '', 'manual', 1)",
            rusqlite::params![article_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id, added_at) VALUES ('p1', ?1, 1)",
            rusqlite::params![article_id],
        )
        .unwrap();
        // agent_approvals.run_id FK：dry-run 建审批前须有 Run 行
        conn.execute(
            "INSERT INTO agent_runs (id, thread_id, status, created_at, updated_at)
             VALUES ('run-h6', 't1', 'running', 1, 1)",
            [],
        )
        .unwrap();
        let body = "---\nid: a1\ntitle: \"笔记\"\n---\n\n你好世界\n";
        fs::write(notes.join("a1.md"), body).unwrap();
        Self {
            root,
            db_path,
            notes,
            article_id,
        }
    }
}

impl Drop for PatchFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn bridge_rejects_patch_without_lease_or_wrong_project() {
    let fx = PatchFixture::setup();
    let bridge = SophonoteBridge::new(LeaseRegistry::new());
    let tools = bridge_patch_registry(fx.db_path.clone(), fx.notes.clone(), "p1", "run-h6");

    let err = bridge
        .invoke_with_tools(
            BridgeInvokeRequest {
                lease_id: "missing".into(),
                tool_name: BRIDGE_PATCH_TOOL.into(),
                arguments: serde_json::json!({}),
                claimed_project_id: None,
                claimed_run_id: None,
            },
            &tools,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, LeaseError::NotFound));

    let lease = issue_lease(
        "run-h6",
        "p1",
        [BRIDGE_PATCH_TOOL],
        ModelRoute::deepseek_default(),
        60_000,
    );
    let id = lease.lease_id.clone();
    bridge.register_lease(lease);
    let err = bridge
        .invoke_with_tools(
            BridgeInvokeRequest {
                lease_id: id,
                tool_name: BRIDGE_PATCH_TOOL.into(),
                arguments: serde_json::json!({}),
                claimed_project_id: Some("p-evil".into()),
                claimed_run_id: None,
            },
            &tools,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, LeaseError::ProjectMismatch { .. }));
}

#[tokio::test]
async fn bridge_patch_dry_run_does_not_mutate_notes() {
    let fx = PatchFixture::setup();
    let note_path = fx.notes.join("a1.md");
    let before = fs::read_to_string(&note_path).unwrap();

    let bridge = SophonoteBridge::new(LeaseRegistry::new());
    let lease = issue_lease(
        "run-h6",
        "p1",
        ["list_project_documents", "read_document", BRIDGE_PATCH_TOOL],
        ModelRoute::deepseek_default(),
        60_000,
    );
    let lease_id = lease.lease_id.clone();
    bridge.register_lease(lease);

    let tools = bridge_patch_registry(fx.db_path.clone(), fx.notes.clone(), "p1", "run-h6");

    let res = bridge
        .invoke_with_tools(
            BridgeInvokeRequest {
                lease_id,
                tool_name: BRIDGE_PATCH_TOOL.into(),
                arguments: serde_json::json!({
                    "articleId": fx.article_id,
                    "baseVersion": 1,
                    "expectedText": "你好世界",
                    "replacementMarkdown": "你好 SophoNote",
                    "idempotencyKey": "h6-patch-1"
                }),
                claimed_project_id: Some("p1".into()),
                claimed_run_id: Some("run-h6".into()),
            },
            &tools,
        )
        .await
        .expect("authorized");

    assert!(res.ok, "error={:?}", res.error);
    assert!(!res.output_text.is_empty() || !res.structured.is_null());
    let after = fs::read_to_string(&note_path).unwrap();
    assert_eq!(before, after, "dry-run 不得改 notes");
}

#[tokio::test]
async fn tool_not_on_lease_cannot_propose_via_bridge() {
    let fx = PatchFixture::setup();
    let bridge = SophonoteBridge::new(LeaseRegistry::new());
    let lease = issue_lease(
        "run-h6",
        "p1",
        ["list_project_documents"], // 无 propose
        ModelRoute::deepseek_default(),
        60_000,
    );
    let id = lease.lease_id.clone();
    bridge.register_lease(lease);
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ProposeDocumentPatchTool::new(
        fx.db_path.clone(),
        fx.notes.clone(),
        "p1".into(),
        "run-h6".into(),
    )));
    let err = bridge
        .invoke_with_tools(
            BridgeInvokeRequest {
                lease_id: id,
                tool_name: BRIDGE_PATCH_TOOL.into(),
                arguments: serde_json::json!({
                    "articleId": "a1",
                    "baseVersion": 1,
                    "expectedText": "你好世界",
                    "replacementMarkdown": "x"
                }),
                claimed_project_id: None,
                claimed_run_id: None,
            },
            &reg,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, LeaseError::ToolNotAllowed(_)));
}
