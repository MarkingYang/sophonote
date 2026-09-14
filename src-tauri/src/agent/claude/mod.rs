//! Claude Code's official CLI, driven by Rust with a Run-scoped tool surface.
pub(crate) mod runtime;
mod tools_server;
pub(crate) mod transport;

use super::commands::HermesModelOptions;
use crate::commands::ApiResponse;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// DeepSeek documents both protocols on the same official origin. Never rewrite proxies.
pub(crate) fn provider_snapshot(
    mut provider: crate::model::openai_compat::ProviderSnapshot,
) -> Option<crate::model::openai_compat::ProviderSnapshot> {
    if provider.protocol == "anthropic" {
        return Some(provider);
    }
    if provider.protocol != "openai" {
        return None;
    }
    let url = reqwest::Url::parse(&provider.base_url).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.deepseek.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path().trim_end_matches('/'), "" | "/v1" | "/anthropic")
    {
        return None;
    }
    provider.protocol = "anthropic".into();
    provider.base_url = "https://api.deepseek.com/anthropic".into();
    Some(provider)
}

pub(crate) fn session_id(thread: &str) -> String {
    let digest = Sha256::digest(thread.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// Discover only this isolated home's transcript; never search personal Claude history.
pub(crate) fn session_path(home: &Path, thread: &str) -> PathBuf {
    let filename = format!("{}.jsonl", session_id(thread));
    if let Ok(projects) = std::fs::read_dir(home.join("config/projects")) {
        for project in projects.take(100).flatten() {
            let candidate = project.path().join(&filename);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    home.join(filename)
}

#[tauri::command]
pub async fn agent_claude_models(app: AppHandle) -> ApiResponse<HermesModelOptions> {
    super::pi::model_options(&app, true)
}

#[tauri::command]
pub async fn agent_claude_status() -> ApiResponse<Value> {
    let runtime = runtime::locate().await;
    ApiResponse::ok(json!({
        "available": runtime.is_ok(), "version": runtime.as_ref().map(|r| r.version.as_str()).unwrap_or(""),
        "error": runtime.err(), "commands": cfg!(target_os="macos"), "browser": false, "mcp": false,
    }))
}

#[cfg(test)]
mod tests;
