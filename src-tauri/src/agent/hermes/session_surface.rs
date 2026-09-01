//! Hermes Desktop 基础会话控制：上下文占用、本轮 YOLO、slash `/undo`。
//!
//! SophoNote 只透传 Gateway 正式 RPC，不维护第二份审批或历史真相源。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};

use crate::agent::engine::EngineError;
use crate::agent::hermes::gateway_client::{HermesGatewayConnection, HermesGatewayEndpoint};
use crate::agent::store::RunStore;
use crate::agent::types::RunStatus;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HermesSessionSurface {
    pub yolo: bool,
    pub context_used: Option<u64>,
    pub context_max: Option<u64>,
    pub context_percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HermesSlashSurfaceResult {
    pub kind: String,
    pub message: String,
    pub notice: Option<String>,
    pub trimmed_run_ids: Vec<String>,
}

pub async fn load_session_surface(
    db_path: &Path,
    thread_id: &str,
) -> Result<HermesSessionSurface, String> {
    let mut gateway = connect_gateway().await?;
    let stored = stored_session_id(db_path, thread_id)?;
    let resumed = gateway
        .call(
            "session.resume",
            json!({
                "session_id": stored,
                "source": "sophonote",
                "omit_messages": true
            }),
        )
        .await
        .map_err(engine_err)?;
    let runtime_id = resumed_session_id(&resumed)?;
    let info = resumed.get("info").cloned().unwrap_or(Value::Null);
    let mut surface = parse_session_surface(&info);
    if surface.context_percent.is_none() {
        if let Ok(usage) = gateway
            .call("session.usage", json!({"session_id": runtime_id}))
            .await
        {
            overlay_usage(&mut surface, &usage);
        }
    }
    Ok(surface)
}

pub async fn set_session_yolo(
    db_path: &Path,
    thread_id: &str,
    enabled: bool,
) -> Result<bool, String> {
    let mut gateway = connect_gateway().await?;
    let runtime_id = resume_thread_session(&mut gateway, db_path, thread_id).await?;
    let result = gateway
        .call(
            "config.set",
            json!({
                "session_id": runtime_id,
                "key": "yolo",
                "value": if enabled { "on" } else { "off" },
                "scope": "session"
            }),
        )
        .await
        .map_err(engine_err)?;
    Ok(parse_yolo_flag(
        result.get("value").and_then(Value::as_str).unwrap_or(""),
    ))
}

pub async fn exec_session_slash(
    db_path: &Path,
    thread_id: &str,
    command: &str,
) -> Result<HermesSlashSurfaceResult, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("Hermes slash command 不能为空".into());
    }
    let mut gateway = connect_gateway().await?;
    let runtime_id = resume_thread_session(&mut gateway, db_path, thread_id).await?;
    let dispatched = gateway
        .call(
            "slash.exec",
            json!({
                "session_id": runtime_id,
                "command": command.trim_start_matches('/')
            }),
        )
        .await
        .map_err(engine_err)?;
    let mut result = parse_slash_surface(&dispatched)?;
    if result.kind == "prefill" {
        if let Some(count) = undo_trim_count(command, result.notice.as_deref()) {
            result.trimmed_run_ids = trim_recent_thread_runs(db_path, thread_id, count)?;
        }
    }
    Ok(result)
}

async fn connect_gateway() -> Result<HermesGatewayConnection, String> {
    let endpoint =
        HermesGatewayEndpoint::from_env().ok_or_else(|| "Hermes Agent 未连接".to_string())?;
    HermesGatewayConnection::connect(&endpoint)
        .await
        .map_err(engine_err)
}

async fn resume_thread_session(
    gateway: &mut HermesGatewayConnection,
    db_path: &Path,
    thread_id: &str,
) -> Result<String, String> {
    let stored = stored_session_id(db_path, thread_id)?;
    let resumed = gateway
        .call(
            "session.resume",
            json!({
                "session_id": stored,
                "source": "sophonote",
                "omit_messages": true
            }),
        )
        .await
        .map_err(engine_err)?;
    resumed_session_id(&resumed)
}

fn stored_session_id(db_path: &Path, thread_id: &str) -> Result<String, String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err("Thread 标识无效".into());
    }
    let conn = rusqlite::Connection::open(db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
    RunStore::new(conn)
        .external_session_id_for_thread(thread_id)
        .map_err(|e| format!("读取 Hermes Session 映射失败: {e}"))?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "当前会话还没有 Hermes Session，请先发送一轮".into())
}

fn resumed_session_id(resumed: &Value) -> Result<String, String> {
    resumed
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "session.resume 缺少 session_id".into())
}

fn engine_err(error: EngineError) -> String {
    error.to_string()
}

pub fn parse_session_surface(info: &Value) -> HermesSessionSurface {
    let mut surface = HermesSessionSurface {
        yolo: info.get("yolo").and_then(Value::as_bool).unwrap_or(false),
        context_used: None,
        context_max: None,
        context_percent: None,
    };
    if let Some(usage) = info.get("usage") {
        overlay_usage(&mut surface, usage);
    }
    surface
}

fn overlay_usage(surface: &mut HermesSessionSurface, usage: &Value) {
    surface.context_used = json_u64(usage, "context_used").or(surface.context_used);
    surface.context_max = json_u64(usage, "context_max").or(surface.context_max);
    surface.context_percent = json_percent(usage, "context_percent").or(surface.context_percent);
}

pub fn parse_yolo_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

pub fn parse_slash_surface(dispatched: &Value) -> Result<HermesSlashSurfaceResult, String> {
    match dispatched.get("type").and_then(Value::as_str) {
        Some("prefill") => Ok(HermesSlashSurfaceResult {
            kind: "prefill".into(),
            message: dispatched
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            notice: dispatched
                .get("notice")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty()),
            trimmed_run_ids: Vec::new(),
        }),
        Some("skill" | "send") => Ok(HermesSlashSurfaceResult {
            kind: "prompt".into(),
            message: dispatched
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            notice: dispatched
                .get("notice")
                .and_then(Value::as_str)
                .map(str::to_string),
            trimmed_run_ids: Vec::new(),
        }),
        Some("exec" | "plugin") | None => Ok(HermesSlashSurfaceResult {
            kind: "output".into(),
            message: dispatched
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or("(no output)")
                .to_string(),
            notice: None,
            trimmed_run_ids: Vec::new(),
        }),
        Some(kind) => Err(format!(
            "Hermes slash.exec 返回了 SophoNote 尚不能呈现的结果类型：{kind}"
        )),
    }
}

pub fn parse_undo_count(command: &str) -> Option<usize> {
    let rest = command.trim().trim_start_matches('/');
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next()?.to_ascii_lowercase();
    if name != "undo" {
        return None;
    }
    match parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        None => Some(1),
        Some(arg) => arg
            .split_whitespace()
            .next()?
            .parse::<usize>()
            .ok()
            .filter(|count| *count >= 1),
    }
}

pub fn parse_undone_turns(notice: &str) -> Option<usize> {
    let lower = notice.to_ascii_lowercase();
    let after = lower.split("undid").nth(1)?;
    after
        .split_whitespace()
        .next()?
        .parse::<usize>()
        .ok()
        .filter(|count| *count >= 1)
}

fn undo_trim_count(command: &str, notice: Option<&str>) -> Option<usize> {
    notice
        .and_then(parse_undone_turns)
        .or_else(|| parse_undo_count(command))
}

fn trim_recent_thread_runs(
    db_path: &Path,
    thread_id: &str,
    count: usize,
) -> Result<Vec<String>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open(db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
    let store = RunStore::new(conn);
    let runs = store
        .list_runs_by_thread(thread_id)
        .map_err(|e| e.to_string())?;
    if runs.iter().take(count).any(|run| {
        matches!(
            run.status,
            RunStatus::Queued | RunStatus::Running | RunStatus::WaitingApproval
        )
    }) {
        return Err("当前回合尚未结束，请先停止后再 /undo".into());
    }
    let trimmed: Vec<String> = runs.iter().take(count).map(|run| run.id.clone()).collect();
    for run_id in &trimmed {
        store
            .delete_run_cascade(run_id)
            .map_err(|e| e.to_string())?;
    }
    let remaining = store
        .list_runs_by_thread(thread_id)
        .map_err(|e| e.to_string())?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    if let Some(latest) = remaining.first() {
        store
            .set_latest_run_id(thread_id, &latest.id, now_ms)
            .map_err(|e| e.to_string())?;
    }
    Ok(trimmed)
}

fn json_u64(value: &Value, key: &str) -> Option<u64> {
    let field = value.get(key)?;
    field
        .as_u64()
        .or_else(|| field.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| field.as_f64().and_then(|n| (n >= 0.0).then_some(n as u64)))
}

fn json_percent(value: &Value, key: &str) -> Option<u8> {
    json_u64(value, key).map(|n| n.min(100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_session_info_usage_and_yolo() {
        let info = json!({
            "yolo": true,
            "usage": {
                "context_used": 12000,
                "context_max": 128000,
                "context_percent": 9
            }
        });
        assert_eq!(
            parse_session_surface(&info),
            HermesSessionSurface {
                yolo: true,
                context_used: Some(12_000),
                context_max: Some(128_000),
                context_percent: Some(9),
            }
        );
    }

    #[test]
    fn unknown_context_occupancy_stays_hidden() {
        let info = json!({"yolo": false, "usage": {"calls": 2, "total": 900}});
        let surface = parse_session_surface(&info);
        assert!(!surface.yolo);
        assert_eq!(surface.context_percent, None);
    }

    #[test]
    fn prefill_undo_does_not_look_like_a_prompt_dispatch() {
        let result = parse_slash_surface(&json!({
            "type": "prefill",
            "message": "上一轮用户原文",
            "notice": "↶ Undid 1 turn (2 message(s)). Edit and resubmit, or send a new message."
        }))
        .unwrap();
        assert_eq!(result.kind, "prefill");
        assert_eq!(result.message, "上一轮用户原文");
        assert_eq!(
            parse_undone_turns(result.notice.as_deref().unwrap()),
            Some(1)
        );
        assert_eq!(parse_undo_count("/undo 3"), Some(3));
        assert_eq!(parse_undo_count("/undo"), Some(1));
        assert_eq!(parse_undo_count("/yolo"), None);
    }

    #[test]
    fn yolo_config_values_are_binary() {
        assert!(parse_yolo_flag("1"));
        assert!(parse_yolo_flag("on"));
        assert!(!parse_yolo_flag("0"));
        assert!(!parse_yolo_flag("off"));
    }
}
