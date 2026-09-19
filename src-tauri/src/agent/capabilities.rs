//! Read-only product capability projection. Native configuration remains with its owner.
use super::commands::{HermesCapabilities, HermesToolInfo};
use crate::commands::ApiResponse;
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilities {
    engine: String,
    available: bool,
    version: Option<String>,
    error: Option<String>,
    managed_categories: Vec<&'static str>,
    tools: Vec<HermesToolInfo>,
    hermes: Option<HermesCapabilities>,
}

fn host_snapshot(engine: &str, status: Value) -> EngineCapabilities {
    // These are the same definitions exposed by the Rust bridge, not CLI-native tools.
    let tools = super::claude::tools_server::definitions()
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["name"] != "bash" || status["commands"] == true)
        .map(|tool| HermesToolInfo {
            name: tool["name"].as_str().unwrap_or_default().into(),
            description: tool["description"].as_str().unwrap_or_default().into(),
        })
        .collect();
    EngineCapabilities {
        engine: engine.into(),
        available: status["available"] == true,
        version: status["version"]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(str::to_owned),
        error: status["error"].as_str().map(str::to_owned),
        managed_categories: vec![],
        tools,
        hermes: None,
    }
}

#[tauri::command]
pub async fn agent_capabilities(app: AppHandle, engine: String) -> ApiResponse<EngineCapabilities> {
    match tokio::time::timeout(std::time::Duration::from_secs(25), load(app, engine)).await {
        Ok(response) => response,
        Err(_) => ApiResponse::err("能力目录读取超时，请重试".into()),
    }
}

async fn load(app: AppHandle, engine: String) -> ApiResponse<EngineCapabilities> {
    if engine == "hermes" {
        let response = super::commands::agent_hermes_capabilities().await;
        return match response.data {
            Some(snapshot) => ApiResponse::ok(EngineCapabilities {
                engine,
                available: true,
                version: None,
                error: None,
                managed_categories: vec!["skills", "tools", "mcp", "hub"],
                tools: snapshot.tools.clone(),
                hermes: Some(snapshot),
            }),
            None => ApiResponse::err(
                response
                    .error
                    .unwrap_or_else(|| "无法读取 Hermes 能力".into()),
            ),
        };
    }
    let response = match engine.as_str() {
        "pi" => super::pi::agent_pi_status(app).await,
        "claude_code" => super::claude::agent_claude_status().await,
        "opencode" => super::opencode::agent_opencode_status(app).await,
        _ => return ApiResponse::err("未知智能体引擎".into()),
    };
    match response.data {
        Some(status) => ApiResponse::ok(host_snapshot(&engine, status)),
        None => ApiResponse::err(response.error.unwrap_or_else(|| "无法读取引擎能力".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn host_bridge_does_not_claim_native_management_or_readiness() {
        for engine in ["pi", "claude_code", "opencode"] {
            let snapshot = host_snapshot(
                engine,
                json!({"available":false,"error":"未安装","commands":false}),
            );
            assert!(!snapshot.available);
            assert_eq!(snapshot.error.as_deref(), Some("未安装"));
            assert!(snapshot.managed_categories.is_empty());
            assert!(snapshot.hermes.is_none());
            assert_eq!(
                snapshot
                    .tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>(),
                ["read", "ls", "write", "edit"]
            );
            let serialized = serde_json::to_value(snapshot).unwrap();
            assert!(serialized.get("managedCategories").is_some());
            assert!(serialized.get("managed_categories").is_none());
        }
    }

    #[test]
    fn displayed_tools_match_host_and_pi_policy() {
        let snapshot = host_snapshot(
            "pi",
            json!({"available":true,"commands":true,"version":"test"}),
        );
        let definitions = super::super::claude::tools_server::definitions();
        assert_eq!(snapshot.tools.len(), definitions.as_array().unwrap().len());
        let policy = include_str!("../../../scripts/assets/pi-policy.ts");
        for tool in snapshot.tools {
            assert!(policy.contains(&format!("name: '{}'", tool.name)));
        }
    }
}
