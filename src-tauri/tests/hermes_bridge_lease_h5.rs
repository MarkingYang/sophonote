//! H5 / NEXT-022：MCP Bridge + SidecarLease + 模型只配一次（DEC-012）。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sophonote_lib::sophonote_mcp::{
    authorize_tool, issue_lease, BridgeInvokeRequest, LeaseError, LeaseRegistry, SophonoteBridge,
    ModelRoute, BRIDGE_MCP_NAME,
};
use sophonote_lib::model::openai_compat::ProviderSnapshot;
use sophonote_lib::skills::{
    export_skills_readonly_cache, LoadedSkill, SkillExecution, SkillManifest, SkillSource,
};

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sophonote-hermes-h5-{tag}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).expect("temp");
    dir
}

#[test]
fn model_route_from_sophonote_settings_snapshot_no_key() {
    let snapshot = ProviderSnapshot {
        id: "deepseek".into(),
        protocol: "openai".into(),
        base_url: "https://api.deepseek.com".into(),
        model: "deepseek-v4-pro".into(),
        models: vec!["deepseek-v4-pro".into(), "deepseek-v4-flash".into()],
        requires_key: false,
    };
    let route = ModelRoute::from_provider_snapshot(&snapshot);
    assert_eq!(route.provider_id, "deepseek");
    assert_eq!(route.model, "deepseek-v4-pro");
    let v = serde_json::to_value(&route).unwrap();
    assert!(v.get("apiKey").is_none());
    assert!(v.get("api_key").is_none());
}

#[test]
fn lease_rejects_expired_and_unauthorized_tool() {
    let reg = LeaseRegistry::new();
    let mut lease = issue_lease(
        "run-a",
        "proj-a",
        ["list_project_documents"],
        ModelRoute::deepseek_default(),
        60_000,
    );
    let id = lease.lease_id.clone();
    lease.expires_at_ms = 1;
    reg.insert(lease);
    assert!(matches!(reg.require_active(&id), Err(LeaseError::Expired)));

    let live = issue_lease(
        "run-b",
        "proj-b",
        ["list_project_documents"],
        ModelRoute::deepseek_default(),
        60_000,
    );
    let err = authorize_tool(&live, "propose_document_patch", None, None).unwrap_err();
    assert!(matches!(err, LeaseError::ToolNotAllowed(_)));
}

#[test]
fn bridge_rejects_spoofed_project_id() {
    let bridge = SophonoteBridge::new(LeaseRegistry::new());
    assert_eq!(bridge.name(), BRIDGE_MCP_NAME);
    let lease = issue_lease(
        "run-1",
        "proj-real",
        ["read_document"],
        ModelRoute::deepseek_default(),
        60_000,
    );
    let lease_id = lease.lease_id.clone();
    bridge.register_lease(lease);

    let err = bridge
        .invoke(BridgeInvokeRequest {
            lease_id: lease_id.clone(),
            tool_name: "read_document".into(),
            arguments: serde_json::json!({}),
            claimed_project_id: Some("proj-evil".into()),
            claimed_run_id: None,
        })
        .unwrap_err();
    assert!(matches!(err, LeaseError::ProjectMismatch { .. }));

    let ok = bridge
        .invoke(BridgeInvokeRequest {
            lease_id,
            tool_name: "read_document".into(),
            arguments: serde_json::json!({}),
            claimed_project_id: Some("proj-real".into()),
            claimed_run_id: Some("run-1".into()),
        })
        .unwrap();
    assert!(ok.ok);
    assert_eq!(ok.project_id, "proj-real");
    assert_eq!(ok.model_provider_id, "deepseek");
}

#[test]
fn skill_readonly_export_is_not_source_of_truth() {
    let dir = temp_dir("skills");
    let manifest = SkillManifest {
        name: "note-summarize".into(),
        version: 2,
        description: "摘要".into(),
        execution: SkillExecution::Agent,
        tools: vec!["read_document".into()],
        max_model_calls: Some(4),
        max_tool_calls: Some(8),
        body: "请摘要".into(),
    };
    let loaded = LoadedSkill {
        manifest: Some(manifest),
        source: SkillSource::User,
        origin: "note-summarize.md".into(),
        problems: Vec::new(),
    };
    let n = export_skills_readonly_cache(&[loaded], &dir).unwrap();
    assert_eq!(n, 1);
    let path = dir.join("skills.json");
    assert!(path.is_file());
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.contains("note-summarize"));
    // 导出缓存可删；真相源不在此目录
    fs::remove_file(&path).unwrap();
    assert!(!path.exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lease_model_route_matches_envelope_injection() {
    // Host 签发 Lease 时绑定与 RunEnvelope 相同的 modelRoute
    let route = ModelRoute::deepseek_default();
    let lease = issue_lease(
        "mb-run",
        "p1",
        ["list_project_documents", "read_document"],
        route.clone(),
        60_000,
    );
    assert_eq!(lease.model_route, route);
    assert!(lease.allowed_tools.contains("list_project_documents"));
    assert!(!lease.allowed_tools.contains("create_document"));
}

#[test]
fn now_ms_sanity_for_ttl() {
    let _ = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
}
