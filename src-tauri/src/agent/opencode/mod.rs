//! Pinned native OpenCode server; shared Host permissions, separate native history.
pub(crate) mod runtime;
pub(crate) mod transport;
pub(crate) mod update;

use crate::commands::ApiResponse;
use serde_json::{json, Value};
use tauri::AppHandle;

#[tauri::command]
pub async fn agent_opencode_models(
    app: AppHandle,
) -> ApiResponse<super::commands::HermesModelOptions> {
    super::pi::model_options(&app, false)
}

#[tauri::command]
pub async fn agent_opencode_status(app: AppHandle) -> ApiResponse<Value> {
    let result = tokio::task::spawn_blocking(move || update::resolve(&app))
        .await
        .map_err(|_| "OpenCode 校验任务失败".to_string())
        .and_then(|r| r);
    let (available, version, error) = match result {
        Ok((runtime, warning)) => (true, runtime.version, warning),
        Err(error) => (false, runtime::VERSION.into(), Some(error)),
    };
    ApiResponse::ok(json!({"available":available, "version":version,
        "error":error, "commands":cfg!(target_os="macos"), "browser":false, "mcp":false}))
}

#[cfg(test)]
mod tests;
