// ============================================================
// Track B · 智能体演进（docs/architecture.md Phase 0）
// 模型网关 Tauri 命令：前端 AI 请求统一入口，Key 不进前端。
// ============================================================
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use super::gateway::ModelGateway;
use super::messages::{FinishReason, ModelMessage, ModelRequest, TokenUsage};
use super::openai_compat::OpenAiCompatGateway;
use crate::commands::ApiResponse;

/// 前端聊天补全请求（camelCase 入参由 Tauri 自动映射）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCompletionArgs {
    pub messages: Vec<ModelMessage>,
    /// 覆盖供应商默认模型（如 deepseek-v4-flash）；缺省用 settings 配置的模型
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// 覆盖 activeProvider（设置页连接测试用）；缺省用 settings.activeProvider
    #[serde(default)]
    pub provider: Option<String>,
    /// 提示词版本号（PromptRegistry 口径），仅日志/观测
    #[serde(default)]
    pub prompt_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCompletionResult {
    pub content: String,
    pub reasoning: Option<String>,
    pub usage: Option<TokenUsage>,
    pub finish_reason: FinishReason,
    pub provider_request_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelCatalog {
    pub models: Vec<String>,
    pub endpoint: String,
}

fn model_catalog_endpoints(base_url: &str) -> Result<Vec<String>, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|error| format!("模型服务地址无效: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("模型服务地址只允许 http/https".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("模型服务地址不能包含用户名或密码".into());
    }
    let mut endpoints = vec![format!("{trimmed}/models")];
    if !parsed.path().trim_end_matches('/').ends_with("/v1") {
        endpoints.push(format!("{trimmed}/v1/models"));
    }
    endpoints.dedup();
    Ok(endpoints)
}

fn parse_model_catalog(value: &serde_json::Value) -> Vec<String> {
    let mut models = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(str::to_string)
        .collect::<Vec<_>>();
    models.sort_unstable();
    models.dedup();
    models.truncate(500);
    models
}

/// 统一聊天补全：全应用唯一 /chat/completions 出口（Go 条件见 docs/architecture.md）
#[tauri::command]
pub async fn ai_chat_completion(
    app: AppHandle,
    request: ChatCompletionArgs,
) -> ApiResponse<ChatCompletionResult> {
    let gateway = match OpenAiCompatGateway::from_settings(&app, request.provider.as_deref()) {
        Ok(g) => g,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let prompt_version = request.prompt_version.clone().unwrap_or_default();
    let model_req = ModelRequest {
        model: request.model.clone().unwrap_or_default(),
        messages: request.messages,
        tools: Vec::new(),
        tool_choice: None,
        temperature: request.temperature,
        max_tokens: None,
        thinking: None,
        prompt_version: prompt_version.clone(),
        run_id: None,
    };
    let effective_model = if model_req.model.is_empty() {
        gateway.default_model.clone()
    } else {
        model_req.model.clone()
    };
    println!(
        "[model] chat completion: model={} prompt_version={} messages={}",
        effective_model,
        if prompt_version.is_empty() {
            "-"
        } else {
            prompt_version.as_str()
        },
        model_req.messages.len()
    );

    match gateway.complete(model_req, CancellationToken::new()).await {
        Ok(resp) => ApiResponse::ok(ChatCompletionResult {
            content: resp.content,
            reasoning: resp.reasoning,
            usage: Some(resp.usage),
            finish_reason: resp.finish_reason,
            provider_request_id: resp.provider_request_id,
        }),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 设置页「测试连接」：Rust 侧自行读取 Key 并发起 ping，前端不再经手 Key
#[tauri::command]
pub async fn ai_test_chat_connection(
    app: AppHandle,
    provider: Option<String>,
) -> ApiResponse<serde_json::Value> {
    let gateway = match OpenAiCompatGateway::from_settings(&app, provider.as_deref()) {
        Ok(g) => g,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let request = ModelRequest {
        model: String::new(),
        messages: vec![ModelMessage::user("ping")],
        tools: Vec::new(),
        tool_choice: None,
        temperature: Some(0.0),
        max_tokens: None,
        thinking: None,
        prompt_version: String::new(),
        run_id: None,
    };
    let started = std::time::Instant::now();
    match gateway.complete(request, CancellationToken::new()).await {
        Ok(_) => ApiResponse::ok(serde_json::json!({
            "latencyMs": started.elapsed().as_millis() as u64
        })),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 从已配置的 OpenAI-compatible Provider 拉取模型目录。
/// WebView 只传 provider id；Base URL 与 Key 均由 Host 重新读取。
#[tauri::command]
pub async fn ai_provider_models(
    app: AppHandle,
    provider: String,
) -> ApiResponse<ProviderModelCatalog> {
    let snapshot = match super::openai_compat::resolve_provider(&app, Some(provider.trim())) {
        Ok(snapshot) => snapshot,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    if snapshot.protocol != "openai" {
        return ApiResponse::err(
            "Anthropic 原生接口暂不提供 /models；请使用预置或手工模型 ID".into(),
        );
    }
    // 免鉴权端点（requiresKey=false，如 Ollama/私有化）允许空 Key；Keychain 异常也不阻断
    let key = match crate::commands::get_cached_api_key(&app, &snapshot.id) {
        Ok(key) => key.trim().to_string(),
        Err(error) if snapshot.requires_key => return ApiResponse::err(error),
        Err(_) => String::new(),
    };
    if key.is_empty() && snapshot.requires_key {
        return ApiResponse::err("请先保存该供应商的 API Key".into());
    }
    let endpoints = match model_catalog_endpoints(&snapshot.base_url) {
        Ok(endpoints) => endpoints,
        Err(error) => return ApiResponse::err(error),
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ApiResponse::err(format!("创建模型目录连接失败: {error}")),
    };
    let mut last_error = String::from("供应商没有返回模型目录");
    for endpoint in endpoints {
        let request = client.get(&endpoint);
        // 免鉴权端点不携带 Authorization 头
        let request = if key.is_empty() {
            request
        } else {
            request.bearer_auth(&key)
        };
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("连接模型目录失败: {error}");
                continue;
            }
        };
        let status = response.status();
        let bytes = match response.bytes().await {
            Ok(bytes) if bytes.len() <= 2 * 1024 * 1024 => bytes,
            Ok(_) => {
                last_error = "模型目录响应超过 2 MB 限制".into();
                continue;
            }
            Err(error) => {
                last_error = format!("读取模型目录失败: {error}");
                continue;
            }
        };
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&bytes);
            let detail = detail.chars().take(180).collect::<String>();
            last_error = format!("模型目录返回 {status}: {detail}");
            continue;
        }
        let value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => value,
            Err(error) => {
                last_error = format!("模型目录不是有效 JSON: {error}");
                continue;
            }
        };
        let models = parse_model_catalog(&value);
        if models.is_empty() {
            last_error = "模型目录中没有可用的模型 ID".into();
            continue;
        }
        return ApiResponse::ok(ProviderModelCatalog { models, endpoint });
    }
    ApiResponse::err(last_error)
}

#[cfg(test)]
mod provider_model_tests {
    use super::*;

    #[test]
    fn catalog_endpoints_preserve_existing_v1_and_add_compat_fallback() {
        assert_eq!(
            model_catalog_endpoints("https://api.moonshot.cn/v1/").unwrap(),
            vec!["https://api.moonshot.cn/v1/models"]
        );
        assert_eq!(
            model_catalog_endpoints("https://api.deepseek.com").unwrap(),
            vec![
                "https://api.deepseek.com/models",
                "https://api.deepseek.com/v1/models"
            ]
        );
        assert!(model_catalog_endpoints("file:///tmp/models").is_err());
        assert!(model_catalog_endpoints("https://user:secret@example.com/v1").is_err());
    }

    #[test]
    fn catalog_parser_deduplicates_sorts_and_ignores_bad_entries() {
        let models = parse_model_catalog(&serde_json::json!({
            "data": [
                {"id": "glm-5"},
                {"id": " qwen3.7-plus "},
                {"id": "glm-5"},
                {"id": ""},
                {"name": "missing-id"}
            ]
        }));
        assert_eq!(models, vec!["glm-5", "qwen3.7-plus"]);
    }
}
