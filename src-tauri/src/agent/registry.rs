//! Closed execution registry. Native protocols stay in their existing adapters.
use serde::{Deserialize, Serialize};

use super::commands::{AgentRunStartArgs, AgentRunStartResult};
use super::events::AgentEvent;
use super::routing::RouteDecision;
use crate::commands::ApiResponse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EngineId {
    Hermes,
    Pi,
    ClaudeCode,
    Opencode,
}

impl EngineId {
    pub(crate) const ALL: [Self; 4] = [Self::Hermes, Self::Pi, Self::ClaudeCode, Self::Opencode];

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "hermes" => Ok(Self::Hermes),
            "pi" => Ok(Self::Pi),
            "claude_code" => Ok(Self::ClaudeCode),
            "opencode" => Ok(Self::Opencode),
            _ => Err("未知智能体引擎".into()),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hermes => "hermes",
            Self::Pi => "pi",
            Self::ClaudeCode => "claude_code",
            Self::Opencode => "opencode",
        }
    }

    pub(crate) fn transport(self) -> &'static str {
        match self {
            Self::Hermes => "jsonrpc+websocket",
            Self::Pi => "jsonl-rpc",
            Self::ClaudeCode => "stream-json+mcp",
            Self::Opencode => "http+sse+mcp",
        }
    }
}

/// The match is exhaustive: unknown engine names can never fall through to Pi/Hermes.
pub(crate) async fn start(
    app: tauri::AppHandle,
    request: AgentRunStartArgs,
    channel: tauri::ipc::Channel<AgentEvent>,
    route: RouteDecision,
) -> ApiResponse<AgentRunStartResult> {
    if let Err(error) = super::candidates::validate_selected(&route.candidates, route.engine()) {
        return ApiResponse::err(error);
    }
    let submission = match super::submission::live::LiveSubmission::begin(
        &crate::db::get_db_path(&app),
        &request,
        route.engine().as_str(),
    ) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    dispatch(app, request, channel, route, submission).await
}

pub(crate) async fn dispatch(
    app: tauri::AppHandle,
    request: AgentRunStartArgs,
    channel: tauri::ipc::Channel<AgentEvent>,
    mut route: RouteDecision,
    submission: super::submission::live::LiveSubmission,
) -> ApiResponse<AgentRunStartResult> {
    if let Err(error) = super::candidates::validate_selected(&route.candidates, route.engine()) {
        submission.failed();
        return ApiResponse::err(error);
    }
    route.submission = Some(submission.receipt.clone());
    if super::routing_shadow::enabled() {
        route.shadow = Some(super::routing_shadow::evaluate(&app, &route.candidates).await);
    }
    let result = match route.engine() {
        EngineId::Hermes => super::commands::start_hermes(app, request, channel, route).await,
        EngineId::Pi | EngineId::ClaudeCode | EngineId::Opencode => {
            super::pi::start_engine(app, request, channel, route).await
        }
    };
    if !result.success {
        submission.failed_reason(result.error.as_deref().unwrap_or("准备失败"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_closed_and_ids_round_trip() {
        for name in ["hermes", "pi", "claude_code", "opencode"] {
            let engine = EngineId::parse(name).unwrap();
            assert_eq!(engine.as_str(), name);
            assert_eq!(serde_json::to_value(engine).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<EngineId>(serde_json::json!(name)).unwrap(),
                engine
            );
        }
        for name in ["auto", "rig", "Pi", "", "/bin/sh", "harness"] {
            assert!(EngineId::parse(name).is_err());
            assert!(serde_json::from_value::<EngineId>(serde_json::json!(name)).is_err());
        }
    }
}
