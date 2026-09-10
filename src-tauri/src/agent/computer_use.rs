//! CU-01/05: macOS Host owns embedded control; other platforms use Hermes setup.
#[cfg(target_os = "macos")]
pub mod macos;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(not(target_os = "macos"))]
use std::time::Duration;

use crate::commands::ApiResponse;

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseStatus {
    #[serde(default)]
    pub embedded: bool,
    #[serde(default)]
    pub permission_owner: Option<String>,
    pub installed: bool,
    pub platform: String,
    pub platform_supported: bool,
    pub version: Option<String>,
    pub ready: Option<bool>,
    pub can_grant: bool,
    pub accessibility: Option<bool>,
    pub screen_recording: Option<bool>,
    pub checks: Vec<ComputerUseCheck>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct ComputerUseCheck {
    pub label: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseAction {
    Install,
    Grant,
}

#[cfg(any(not(target_os = "macos"), test))]
impl ComputerUseAction {
    fn name(self) -> &'static str {
        match self {
            Self::Install => "tools-post-setup",
            Self::Grant => "computer-use-grant",
        }
    }
    fn request(self) -> (&'static str, Option<Value>) {
        match self {
            Self::Install => (
                "api/tools/toolsets/computer_use/post-setup",
                Some(json!({"key": "cua_driver"})),
            ),
            Self::Grant => ("api/tools/computer-use/permissions/grant", None),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseActionStatus {
    pub running: bool,
    pub exit_code: Option<i64>,
    pub pid: Option<u64>,
}

fn bounded_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(|s| s.chars().take(1024).collect())
}

pub fn parse_status(value: &Value) -> Result<ComputerUseStatus, String> {
    let installed = value
        .get("installed")
        .and_then(Value::as_bool)
        .ok_or("Hermes 未返回电脑操作诊断，请检查 Runtime 版本")?;
    let checks = value
        .get("checks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(64)
        .map(|row| ComputerUseCheck {
            label: bounded_text(row, "label").unwrap_or_default(),
            status: bounded_text(row, "status").unwrap_or_else(|| "unknown".into()),
            message: bounded_text(row, "message").unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    let platform_supported = value
        .get("platform_supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let error = bounded_text(value, "error").filter(|s| !s.is_empty());
    let ready = match value.get("ready").and_then(Value::as_bool) {
        Some(true)
            if installed
                && platform_supported
                && error.is_none()
                && !checks
                    .iter()
                    .any(|c| matches!(c.status.as_str(), "fail" | "failed" | "error")) =>
        {
            Some(true)
        }
        Some(_) => Some(false),
        None => None,
    };
    Ok(ComputerUseStatus {
        embedded: false,
        permission_owner: None,
        installed,
        platform_supported,
        ready,
        checks,
        error,
        platform: bounded_text(value, "platform").unwrap_or_default(),
        version: bounded_text(value, "version"),
        can_grant: value
            .get("can_grant")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        accessibility: value.get("accessibility").and_then(Value::as_bool),
        screen_recording: value.get("screen_recording").and_then(Value::as_bool),
    })
}

#[cfg(not(target_os = "macos"))]
async fn request(
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    super::commands::hermes_dashboard_request(method, path, body, Duration::from_secs(40)).await
}

pub async fn check_session_ready(
    gateway: &mut super::hermes::gateway_client::HermesGatewayConnection,
    session_id: &str,
) -> Result<(), super::engine::EngineError> {
    use super::engine::EngineError;
    let diagnostic = current_status().await.map_err(EngineError::Setup)?;
    if diagnostic.ready != Some(true) {
        return Err(EngineError::Setup(
            "电脑驱动尚未就绪，请打开「电脑操作」完成安装和系统授权".into(),
        ));
    }
    let global = gateway.call("tools.list", json!({})).await?;
    let session = gateway
        .call("tools.list", json!({"session_id": session_id}))
        .await?;
    if tool_enabled(&global) != Some(true) {
        return Err(EngineError::Setup(
            "电脑工具未启用，请在「电脑操作」中启用".into(),
        ));
    }
    if tool_enabled(&session) != Some(true) {
        // tools.configure(session_id) resets the pinned Gateway's history.
        // Never use it as a tool refresh; preserve the user's conversation.
        return Err(EngineError::Setup(
            "当前会话尚未加载电脑工具，请新建会话后使用；原会话历史会保留".into(),
        ));
    }
    Ok(())
}

fn tool_enabled(value: &Value) -> Option<bool> {
    value
        .get("toolsets")?
        .as_array()?
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some("computer_use"))?
        .get("enabled")?
        .as_bool()
}

async fn current_status() -> Result<ComputerUseStatus, String> {
    #[cfg(target_os = "macos")]
    {
        macos::status().await
    }
    #[cfg(not(target_os = "macos"))]
    {
        request(reqwest::Method::GET, "api/tools/computer-use/status", None)
            .await
            .and_then(|value| parse_status(&value))
    }
}

#[tauri::command]
pub async fn agent_computer_use_status() -> ApiResponse<ComputerUseStatus> {
    match current_status().await {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_computer_use_action_start(
    app: tauri::AppHandle,
    action: ComputerUseAction,
) -> ApiResponse<ComputerUseActionStatus> {
    #[cfg(target_os = "macos")]
    {
        match action {
            ComputerUseAction::Install => {
                ApiResponse::err("电脑组件随 SophoNote 安装，请重新安装完整应用".into())
            }
            ComputerUseAction::Grant => match macos::grant(app).await {
                Ok(value) => ApiResponse::ok(value),
                Err(error) => ApiResponse::err(error),
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        static START_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let Ok(_guard) = START_LOCK.try_lock() else {
            return ApiResponse::err("电脑操作配置正在启动，请稍候".into());
        };
        // Hermes shares its post-setup process slot across toolsets. Do not replace
        // a running installation or lose its process identity through double-clicks.
        for current in [ComputerUseAction::Install, ComputerUseAction::Grant] {
            let path = format!("api/actions/{}/status?lines=1", current.name());
            match request(reqwest::Method::GET, &path, None).await {
                Ok(value) if value.get("running").and_then(Value::as_bool) == Some(false) => {}
                Ok(_) => {
                    return ApiResponse::err(
                        "Hermes 已有工具安装或授权流程，请等待完成后重新检测".into(),
                    )
                }
                Err(error) => return ApiResponse::err(error),
            }
        }
        let (path, body) = action.request();
        match request(reqwest::Method::POST, path, body).await {
            Ok(value)
                if value.get("ok").and_then(Value::as_bool) == Some(true)
                    && value.get("pid").and_then(Value::as_u64).is_some() =>
            {
                ApiResponse::ok(ComputerUseActionStatus {
                    running: true,
                    exit_code: None,
                    pid: value.get("pid").and_then(Value::as_u64),
                })
            }
            Ok(_) => ApiResponse::err("Hermes 未确认操作已启动，请重新检测后重试".into()),
            Err(error) => ApiResponse::err(error),
        }
    }
}

#[tauri::command]
pub async fn agent_computer_use_action_status(
    action: ComputerUseAction,
) -> ApiResponse<ComputerUseActionStatus> {
    #[cfg(target_os = "macos")]
    {
        let _ = action;
        ApiResponse::ok(ComputerUseActionStatus {
            running: false,
            exit_code: Some(0),
            pid: None,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Only these two actions; never proxy arbitrary action names or raw logs.
        let path = format!("api/actions/{}/status?lines=1", action.name());
        match request(reqwest::Method::GET, &path, None).await {
            Ok(value) => match value.get("running").and_then(Value::as_bool) {
                Some(running) => ApiResponse::ok(ComputerUseActionStatus {
                    running,
                    exit_code: value.get("exit_code").and_then(Value::as_i64),
                    pid: value.get("pid").and_then(Value::as_u64),
                }),
                None => ApiResponse::err("Hermes 未返回安装/授权状态".into()),
            },
            Err(error) => ApiResponse::err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_is_not_inferred_from_installation() {
        let status = parse_status(&json!({"installed": true, "platform_supported": true})).unwrap();
        assert_eq!(status.ready, None);
        assert_eq!(status.accessibility, None);
        assert!(parse_status(&json!({"detail": "not found"})).is_err());
    }
    #[test]
    fn failing_checks_override_ready_and_private_data_is_not_exposed() {
        let status = parse_status(
            &json!({"installed": true, "platform_supported": true, "ready": true,
            "checks": [{"label":"driver", "status":"fail", "message":"unavailable"}],
            "source": {"secret": "private"}, "lines": ["private"]}),
        )
        .unwrap();
        assert_eq!(status.ready, Some(false));
        assert!(!serde_json::to_string(&status).unwrap().contains("private"));
    }
    #[test]
    fn actions_are_allowlisted() {
        assert!(serde_json::from_str::<ComputerUseAction>("\"terminal\"").is_err());
        assert_eq!(
            ComputerUseAction::Install.request().1,
            Some(json!({"key":"cua_driver"}))
        );
        assert_eq!(ComputerUseAction::Grant.name(), "computer-use-grant");
    }
    #[test]
    fn missing_or_disabled_session_capability_is_not_ready() {
        assert_eq!(tool_enabled(&json!({})), None);
        assert_eq!(
            tool_enabled(&json!({"toolsets":[{"name":"computer_use", "enabled":false}]})),
            Some(false)
        );
        assert_eq!(
            tool_enabled(&json!({"toolsets":[{"name":"computer_use", "enabled":true}]})),
            Some(true)
        );
    }
}
