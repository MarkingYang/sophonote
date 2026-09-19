//! Pinned native OpenCode server; shared Host permissions, separate native history.
pub(crate) mod runtime;
pub(crate) mod transport;

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
    let result = tokio::task::spawn_blocking(move || {
        runtime::locate(&app).and_then(|root| runtime::verify(&root))
    })
    .await
    .map_err(|_| "OpenCode 校验任务失败".to_string())
    .and_then(|r| r);
    ApiResponse::ok(
        json!({"available":result.is_ok(), "version":runtime::VERSION,
        "error":result.err(), "commands":cfg!(target_os="macos"), "browser":false, "mcp":false}),
    )
}

#[cfg(test)]
mod tests;
