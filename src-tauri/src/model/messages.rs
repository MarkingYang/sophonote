// ============================================================
// Track B · 智能体演进（docs/architecture.md Phase 0）
// 统一模型消息类型：Message / ToolCall / Usage 全应用唯一口径。
// ============================================================
use serde::{Deserialize, Serialize};

/// 对话角色（OpenAI-compatible 口径；Anthropic 等差异收口在适配器）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

/// 统一消息体。Phase 0 阶段 content 只有纯文本；
/// Phase 1 接入工具调用后，assistant 消息的 tool_calls 与 tool 消息的 call_id 走 ModelToolCall/ToolResult。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
    /// assistant 消息携带的工具调用（Phase 1 起使用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ModelToolCall>,
    /// tool 消息对应的 tool_call id（Phase 1 起使用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ModelMessage {
    pub fn new(role: ModelRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(ModelRole::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(ModelRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(ModelRole::Assistant, content)
    }
}

/// 模型侧发起的一次工具调用（参数保持原始 JSON 字符串/值，校验归 ToolGateway）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    /// 原始参数 JSON；畸形 JSON 由 ToolGateway 按 §十二「最多让模型修复一次」处理
    pub arguments: serde_json::Value,
}

/// 工具定义（交给模型的 function schema）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// tool_choice（OpenAI-compatible 口径）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    /// 指定工具名
    Tool(String),
}

/// 统一请求体（Phase 0：messages/model/temperature；tools 字段为 Phase 1 预留，序列化时省略空值）
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// 推理模式偏好。`None` 保持供应商默认；`Some(false)` 用于低延迟、只需直接答案的请求。
    /// 具体 wire 参数由各 Gateway 按供应商能力转换，不直接泄漏到上层业务。
    pub thinking: Option<bool>,
    /// 提示词版本号（PromptRegistry 口径），落日志/运行事件做回归对比
    pub prompt_version: String,
    /// 关联的 Agent Run（Phase 1 起由 RunManager 注入；Phase 0 可为空）
    pub run_id: Option<String>,
}

/// 结束原因
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    /// provider 返回了未枚举的原因（原文保留）
    Other(String),
}

/// Token 用量（统一口径，聚合逻辑 Phase 1 归 Rig AgentRun）
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// 统一响应体
#[derive(Debug, Clone)]
pub struct ModelResponse {
    /// 文本内容（多段 content part 在 Phase 1 引入；Phase 0 单段文本）
    pub content: String,
    /// 推理过程（DeepSeek reasoning_content 等，可选）
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ModelToolCall>,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    /// provider 侧请求/响应 id（排障溯源）
    pub provider_request_id: Option<String>,
}

/// 模型调用错误分类（§十二 错误策略的传输层基础）
#[derive(Debug, Clone)]
pub enum ModelError {
    /// 配置缺失/非法（未配置供应商、未填 Key 等）——不重试
    Config(String),
    /// 网络层失败（超时、连接重置）——有限重试
    Network(String),
    /// HTTP 非 2xx；status=429 走退避重试。url 为实际请求端点（排障：看清 401 到底来自哪个地址）
    Http {
        status: u16,
        body: String,
        url: String,
    },
    /// 响应解析失败
    Parse(String),
    /// 被取消（CancellationToken）
    Cancelled,
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::Config(msg) => write!(f, "{}", msg),
            ModelError::Network(msg) => write!(f, "模型请求网络失败: {}", msg),
            ModelError::Http { status, body, url } => {
                let snippet: String = body.chars().take(200).collect();
                write!(f, "模型 API error: {} @ {} - {}", status, url, snippet)
            }
            ModelError::Parse(msg) => write!(f, "模型响应解析失败: {}", msg),
            ModelError::Cancelled => write!(f, "模型调用已取消"),
        }
    }
}

impl std::error::Error for ModelError {}
