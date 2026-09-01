// ============================================================
// Track B · 智能体演进（docs/architecture.md Phase 0）
// OpenAI-compatible Chat Completions 适配器：provider 差异全部收口在这里。
// 第一阶段只支持 OpenAI-compatible；Anthropic 原生格式以后再加（§四）。
// ============================================================
use std::time::Duration;

use async_trait::async_trait;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use super::gateway::ModelGateway;
use super::messages::{
    FinishReason, ModelError, ModelRequest, ModelResponse, ModelToolCall, TokenUsage,
};

/// 模型调用统一超时（夜间深度解读可达分钟级，与原 scheduler 120s 对齐）
const REQUEST_TIMEOUT_SECS: u64 = 120;

/// 进程共享 HTTP 客户端（连接池复用；reqwest::Client 本身廉价克隆但池子要共享）
static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    // build() 仅在 TLS 后端初始化异常时失败（进程级致命错误，expect 可接受）；
    // 不用 get_or_try_init（宿主 rustc 版本该特性尚未稳定）
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("HTTP 客户端初始化失败")
    })
}

/// OpenAI-compatible 网关实例：配置解析完成后即与 settings 解耦（可测试、可复用）。
/// AG-22：新增 provider_id/default_model 只读快照——Run 元数据必须记录
/// 真实解析出的供应商/模型（审计 P1-2：固定值会让切换供应商后审计失真）。
pub struct OpenAiCompatGateway {
    pub provider_id: String,
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
}

/// settings.ai_config 解析出的供应商快照。
/// `models` 是该供应商允许逐 Run 选择的白名单，始终包含默认 `model`。
/// `requires_key` = false 表示本地/私有化部署等无需 API Key 的端点（Ollama、vLLM 等）。
#[derive(Debug, Clone)]
pub struct ProviderSnapshot {
    pub id: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub models: Vec<String>,
    pub requires_key: bool,
}

fn provider_snapshot_from_value(
    provider_id: &str,
    provider: &serde_json::Value,
) -> Result<ProviderSnapshot, ModelError> {
    let base_url = provider["baseUrl"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    let model = provider["model"].as_str().unwrap_or("").trim().to_string();
    if base_url.is_empty() || model.is_empty() {
        return Err(ModelError::Config(format!(
            "供应商 {} 缺少 baseUrl 或 model 配置",
            provider_id
        )));
    }
    let mut models = vec![model.clone()];
    if let Some(configured) = provider["models"].as_array() {
        for candidate in configured {
            let Some(candidate) = candidate.as_str().map(str::trim) else {
                continue;
            };
            if !candidate.is_empty() && !models.iter().any(|existing| existing == candidate) {
                models.push(candidate.to_string());
            }
        }
    }
    Ok(ProviderSnapshot {
        id: provider_id.to_string(),
        protocol: provider["protocol"]
            .as_str()
            .unwrap_or("openai")
            .to_string(),
        base_url,
        model,
        models,
        requires_key: provider["requiresKey"].as_bool().unwrap_or(true),
    })
}

/// 读取设置中全部有效供应商实例。相同厂商的 `-2`、`-3` 配置保留为独立快照，
/// 由调用方分别检查其 Keychain 凭据，避免鉴权和免鉴权配置串状态。
pub fn configured_provider_snapshots(app: &AppHandle) -> Result<Vec<ProviderSnapshot>, ModelError> {
    let db_path = crate::db::get_db_path(app);
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| ModelError::Config(format!("打开数据库失败: {}", e)))?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ai_config'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(raw) = raw else {
        return resolve_provider(app, None).map(|provider| vec![provider]);
    };
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ModelError::Config(format!("AI 配置解析失败: {}", e)))?;
    let Some(providers) = parsed["providers"].as_object() else {
        return Ok(Vec::new());
    };
    Ok(providers
        .iter()
        .filter_map(|(id, provider)| provider_snapshot_from_value(id, provider).ok())
        .collect())
}

impl ProviderSnapshot {
    /// DEC-019：逐 Run 模型只能来自当前激活供应商的配置清单。
    pub fn supports_model(&self, model: &str) -> bool {
        self.models.iter().any(|configured| configured == model)
    }
}

/// 内置默认供应商（与前端 defaultSettings.aiConfig 对齐）：
/// settings 从未写入 ai_config 时兜底，避免比旧前端行为退化
const DEFAULT_PROVIDER_ID: &str = "deepseek";
const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-v4-pro";

/// 从 SQLite settings 读取 ai_config 并定位供应商（provider_override 为空时取 activeProvider）
pub fn resolve_provider(
    app: &AppHandle,
    provider_override: Option<&str>,
) -> Result<ProviderSnapshot, ModelError> {
    let db_path = crate::db::get_db_path(app);
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| ModelError::Config(format!("打开数据库失败: {}", e)))?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ai_config'",
            [],
            |row| row.get(0),
        )
        .ok();

    // settings 无 ai_config：回退内置默认供应商（Key 仍按 provider id 读取）
    let raw = match raw {
        Some(r) => r,
        None => {
            let id = provider_override
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_PROVIDER_ID);
            return Ok(ProviderSnapshot {
                id: id.to_string(),
                protocol: "openai".to_string(),
                base_url: DEFAULT_BASE_URL.to_string(),
                model: DEFAULT_MODEL.to_string(),
                models: vec![DEFAULT_MODEL.to_string()],
                requires_key: true,
            });
        }
    };

    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ModelError::Config(format!("AI 配置解析失败: {}", e)))?;

    let provider_id = match provider_override {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => parsed["activeProvider"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ModelError::Config("AI 配置缺少 activeProvider".to_string()))?
            .to_string(),
    };

    let provider = &parsed["providers"][provider_id.as_str()];
    if provider.is_null() {
        return Err(ModelError::Config(format!(
            "供应商 {} 未配置，请到 设置 → AI 配置 检查",
            provider_id
        )));
    }
    provider_snapshot_from_value(&provider_id, provider)
}

impl OpenAiCompatGateway {
    /// 从 settings 构造网关：配置 + Key 全部在 Rust 侧解析（Key 永不进前端）。
    /// provider_override：指定供应商 id（设置页连接测试用）；None = activeProvider。
    pub fn from_settings(
        app: &AppHandle,
        provider_override: Option<&str>,
    ) -> Result<Self, ModelError> {
        let snapshot = resolve_provider(app, provider_override)?;
        Self::from_snapshot(app, &snapshot)
    }

    /// AG-22（审计 P1-2 整改①）：从已解析的 ProviderSnapshot 构造网关。
    /// 调用方（agent_run_start）先 resolve_provider 拿到真实 provider/model
    /// 写入 Run 记录，再用同一 snapshot 构造网关——Run 元数据与实际请求同源，
    /// 不再出现「记录是 deepseek-chat-v3.1、实际跑 settings 里别的模型」的失真。
    pub fn from_snapshot(app: &AppHandle, snapshot: &ProviderSnapshot) -> Result<Self, ModelError> {
        if snapshot.protocol != "openai" {
            return Err(ModelError::Config(format!(
                "供应商 {} 协议为 {}：Phase 0 仅支持 openai-compatible，其余协议待后续适配器",
                snapshot.id, snapshot.protocol
            )));
        }
        let api_key =
            crate::commands::get_cached_api_key(app, &snapshot.id).map_err(ModelError::Config)?;
        if api_key.is_empty() && snapshot.requires_key {
            return Err(ModelError::Config(format!(
                "供应商 {} 未填写 API Key，请到 设置 → AI 配置 填写",
                snapshot.id
            )));
        }
        Ok(Self {
            provider_id: snapshot.id.clone(),
            base_url: snapshot.base_url.clone(),
            api_key,
            default_model: snapshot.model.clone(),
        })
    }

    /// 组装 OpenAI-compatible 请求体
    fn build_body(&self, request: &ModelRequest) -> serde_json::Value {
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut v = serde_json::json!({ "role": m.role, "content": m.content });
                if !m.tool_calls.is_empty() {
                    // OpenAI wire format：{id, type: function, function: {name, arguments: "<json string>"}}
                    let calls: Vec<serde_json::Value> = m
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments.to_string(),
                                }
                            })
                        })
                        .collect();
                    v["tool_calls"] = serde_json::json!(calls);
                }
                if let Some(id) = &m.tool_call_id {
                    v["tool_call_id"] = serde_json::json!(id);
                }
                v
            })
            .collect();

        let mut body = serde_json::json!({
            "model": if request.model.is_empty() { &self.default_model } else { &request.model },
            "messages": messages,
            "stream": false,
        });
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::json!(t);
        }
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        // DeepSeek V4 默认开启 thinking；补全等低延迟请求必须显式关闭，否则有限的
        // max_tokens 可能全被 reasoning_content 消耗，最终 content 为空。
        // `thinking` 不是 OpenAI 标准字段，只对官方 DeepSeek 端点下发，避免破坏 Kimi 等兼容服务。
        if let Some(enabled) = request.thinking {
            let is_official_deepseek = reqwest::Url::parse(&self.base_url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .is_some_and(|host| host.eq_ignore_ascii_case("api.deepseek.com"));
            if is_official_deepseek {
                body["thinking"] = serde_json::json!({
                    "type": if enabled { "enabled" } else { "disabled" }
                });
            }
        }
        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
            if let Some(choice) = &request.tool_choice {
                body["tool_choice"] = match choice {
                    super::messages::ToolChoice::Auto => serde_json::json!("auto"),
                    super::messages::ToolChoice::None => serde_json::json!("none"),
                    super::messages::ToolChoice::Required => serde_json::json!("required"),
                    super::messages::ToolChoice::Tool(name) => serde_json::json!({
                        "type": "function",
                        "function": { "name": name }
                    }),
                };
            }
        }
        body
    }

    /// 解析 OpenAI-compatible 响应（choices[0].message；兼容 reasoning_content 与 tool_calls）
    fn parse_response(data: &serde_json::Value) -> Result<ModelResponse, ModelError> {
        let choice = &data["choices"][0];
        if choice.is_null() {
            return Err(ModelError::Parse("响应缺少 choices[0]".to_string()));
        }
        let message = &choice["message"];
        let content = message["content"].as_str().unwrap_or_default().to_string();
        let reasoning = message["reasoning_content"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let mut tool_calls = Vec::new();
        if let Some(arr) = message["tool_calls"].as_array() {
            for tc in arr {
                let id = tc["id"].as_str().unwrap_or_default().to_string();
                let name = tc["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let raw_args = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let arguments: serde_json::Value =
                    serde_json::from_str(raw_args).unwrap_or(serde_json::Value::Null);
                tool_calls.push(ModelToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }

        let finish_reason = match choice["finish_reason"].as_str().unwrap_or("stop") {
            "stop" => FinishReason::Stop,
            "tool_calls" => FinishReason::ToolCalls,
            "length" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            other => FinishReason::Other(other.to_string()),
        };

        let usage = TokenUsage {
            prompt_tokens: data["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            completion_tokens: data["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            total_tokens: data["usage"]["total_tokens"].as_u64().unwrap_or(0),
        };

        let provider_request_id = data["id"].as_str().map(|s| s.to_string());

        Ok(ModelResponse {
            content,
            reasoning,
            tool_calls,
            finish_reason,
            usage,
            provider_request_id,
        })
    }
}

#[async_trait]
impl ModelGateway for OpenAiCompatGateway {
    /// AG-22：对外语义 = 带有限重试的补全（网络瞬态/429 退避重试，
    /// 上限 RetryPolicy::model_default；退避期可被 cancel 立即打断）。
    /// 补全请求无副作用，重试安全；工具执行不在此层、永不重放（§十二）。
    async fn complete(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let policy = crate::model::gateway::RetryPolicy::model_default();
        crate::model::gateway::with_retry(policy, &cancel, || {
            self.complete_once(request.clone(), cancel.clone())
        })
        .await
    }
}

#[cfg(test)]
mod provider_snapshot_tests {
    use super::ProviderSnapshot;

    #[test]
    fn model_whitelist_is_provider_scoped() {
        let snapshot = ProviderSnapshot {
            id: "deepseek".into(),
            protocol: "openai".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-flash".into(),
            models: vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()],
            requires_key: true,
        };
        assert!(snapshot.supports_model("deepseek-v4-flash"));
        assert!(snapshot.supports_model("deepseek-v4-pro"));
        assert!(!snapshot.supports_model("kimi-k3"));
    }
}

impl OpenAiCompatGateway {
    /// 单次补全尝试（原 complete 主体；失败分类见 ModelError）
    async fn complete_once(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let client = http_client();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = self.build_body(&request);
        if self.api_key.is_empty() {
            // 免鉴权端点排障线索：写入 dev.log，不含任何凭据
            eprintln!("[model] provider={} keyless POST {}", self.provider_id, url);
        }

        let request_builder = client.post(&url).header("Content-Type", "application/json");
        // 免鉴权端点（Ollama/私有化）不带 Authorization 头
        let request_builder = if self.api_key.is_empty() {
            request_builder
        } else {
            request_builder.bearer_auth(&self.api_key)
        };
        let send_future = request_builder.json(&body);

        // 取消与发送竞争：cancel 先到即放弃请求
        let res = tokio::select! {
            _ = cancel.cancelled() => return Err(ModelError::Cancelled),
            r = send_future.send() => r,
        };
        let res = res.map_err(|e| ModelError::Network(e.to_string()))?;

        let status = res.status();
        let data: serde_json::Value = res
            .json()
            .await
            .map_err(|e| ModelError::Parse(e.to_string()))?;

        if !status.is_success() {
            return Err(ModelError::Http {
                status: status.as_u16(),
                body: data.to_string(),
                url: url.clone(),
            });
        }
        if cancel.is_cancelled() {
            return Err(ModelError::Cancelled);
        }
        Self::parse_response(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::messages::ModelMessage;

    fn request(thinking: Option<bool>) -> ModelRequest {
        ModelRequest {
            model: "deepseek-v4-flash".to_string(),
            messages: vec![ModelMessage::user("续写")],
            tools: Vec::new(),
            tool_choice: None,
            temperature: Some(0.2),
            max_tokens: Some(128),
            thinking,
            prompt_version: "completion@v1".to_string(),
            run_id: None,
        }
    }

    #[test]
    fn build_body_disables_thinking_for_official_deepseek() {
        let gateway = OpenAiCompatGateway {
            provider_id: "deepseek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key: String::new(),
            default_model: "deepseek-v4-flash".to_string(),
        };

        let body = gateway.build_body(&request(Some(false)));
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn build_body_omits_deepseek_extension_for_other_providers() {
        let gateway = OpenAiCompatGateway {
            provider_id: "kimi".to_string(),
            base_url: "https://api.moonshot.cn/v1".to_string(),
            api_key: String::new(),
            default_model: "kimi-latest".to_string(),
        };

        let body = gateway.build_body(&request(Some(false)));
        assert!(body.get("thinking").is_none());
    }
}
