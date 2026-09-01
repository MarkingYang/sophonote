//! 真 Hermes Gateway 附着活测（默认 ignore；需本机 `hermes serve`）。
//!
//! ```bash
//! export SOPHONOTE_HERMES_GATEWAY_URL=ws://127.0.0.1:8642/api/ws
//! export SOPHONOTE_HERMES_GATEWAY_TOKEN='…'
//! cargo test --test hermes_attached_live -- --ignored --nocapture
//! ```

use sophonote_lib::agent::hermes::gateway_client::HermesGatewayConnection;
use sophonote_lib::agent::hermes::{HermesGatewayEndpoint, ENV_GATEWAY_TOKEN, ENV_GATEWAY_URL};
use serde_json::json;

fn live_endpoint() -> Option<HermesGatewayEndpoint> {
    HermesGatewayEndpoint::from_env()
}

#[tokio::test]
#[ignore = "需要本机 Hermes Gateway + SOPHONOTE_HERMES_GATEWAY_URL/TOKEN"]
async fn gateway_exposes_runtime_model_catalog() {
    let endpoint = live_endpoint().unwrap_or_else(|| {
        panic!("请设置 {ENV_GATEWAY_URL} 与 {ENV_GATEWAY_TOKEN} 后再跑 --ignored");
    });
    let mut gateway = HermesGatewayConnection::connect(&endpoint)
        .await
        .expect("connect Hermes Gateway");
    let options = gateway
        .call("model.options", json!({"include_unconfigured": true}))
        .await
        .expect("model.options");
    let providers = options["providers"]
        .as_array()
        .unwrap_or_else(|| panic!("model.options.providers missing: {options}"));
    assert!(!providers.is_empty(), "Hermes model catalog is empty");
    assert!(
        providers.iter().any(|provider| {
            provider["authenticated"].as_bool().unwrap_or(true)
                && provider["models"]
                    .as_array()
                    .is_some_and(|models| !models.is_empty())
        }),
        "Hermes has no authenticated provider with selectable models: {options}"
    );
}

#[tokio::test]
#[ignore = "需要本机 Hermes Gateway + SOPHONOTE_HERMES_GATEWAY_URL/TOKEN"]
async fn gateway_exposes_desktop_capability_catalogs() {
    let endpoint = live_endpoint().expect("Hermes Gateway env");
    let mut gateway = HermesGatewayConnection::connect(&endpoint)
        .await
        .expect("connect Hermes Gateway");
    let commands = gateway
        .call("commands.catalog", json!({}))
        .await
        .expect("commands.catalog");
    let toolsets = gateway
        .call("tools.list", json!({}))
        .await
        .expect("tools.list");
    let tools = gateway
        .call("tools.show", json!({}))
        .await
        .expect("tools.show");
    let skills = gateway
        .call("skills.manage", json!({"action": "list"}))
        .await
        .expect("skills.manage");
    let browser = gateway
        .call("browser.manage", json!({"action": "status"}))
        .await
        .expect("browser.manage");
    assert!(commands["skills"].is_object());
    assert!(toolsets["toolsets"].is_array());
    assert!(tools["sections"].is_array());
    assert!(skills["skills"].is_object());
    assert!(browser["connected"].is_boolean());
}

#[tokio::test]
#[ignore = "需要本机 Hermes Gateway + SOPHONOTE_HERMES_GATEWAY_URL/TOKEN"]
async fn dashboard_exposes_authenticated_mcp_management_surface() {
    let endpoint = live_endpoint().expect("Hermes Gateway env");
    let url = endpoint
        .dashboard_base_url()
        .expect("derive dashboard origin")
        .join("api/mcp/servers")
        .expect("MCP servers URL");
    // loopback 走系统代理会被截获返回 502（同 bundled_runtime 口径），强制直连。
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("no-proxy client")
        .get(url)
        .header("X-Hermes-Session-Token", &endpoint.token)
        .send()
        .await
        .expect("GET /api/mcp/servers");
    assert!(response.status().is_success());
    let body = response
        .json::<serde_json::Value>()
        .await
        .expect("MCP servers JSON");
    assert!(body["servers"].is_array(), "MCP servers missing: {body}");
}

#[tokio::test]
#[ignore = "需要本机 Hermes Gateway + SOPHONOTE_HERMES_GATEWAY_URL/TOKEN"]
async fn gateway_creates_a_sophonote_surface_session() {
    let endpoint = live_endpoint().expect("Hermes Gateway env");
    let mut gateway = HermesGatewayConnection::connect(&endpoint)
        .await
        .expect("connect Hermes Gateway");
    let created = gateway
        .call("session.create", json!({"source": "sophonote", "cols": 96}))
        .await
        .expect("session.create");
    let runtime_session_id = created["session_id"].as_str().expect("runtime session_id");
    let stored_session_id = created["stored_session_id"]
        .as_str()
        .expect("stored_session_id");
    assert!(!runtime_session_id.is_empty());
    assert!(!stored_session_id.is_empty());
    gateway
        .call("session.close", json!({"session_id": runtime_session_id}))
        .await
        .expect("session.close");
}
