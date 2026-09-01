// ============================================================
// Track B · 智能体演进（AG-05 追加）：Rig Adapter 层（Phase 1 Spike）
// 实施基线：docs/architecture.md「两个 Adapter 必须独立测试，
// 不散落在循环里」，并以固定 fixture 独立测试。
//
// 边界：
// - 只做 SophoNote 自有类型 ↔ rig 0.41 消息契约的纯转换，无 IO、无状态；
// - rig 类型只在本文件与 run_controller（AG-06）出现，不外泄到业务层；
// - 硬性限制②：rig 消息不作为业务持久化格式——from_rig_history 仅用于
//   把 AgentRun 累计的会话还原为 SophoNote 自有 ModelMessage 转录。
//
// rig 0.41 契约要点（2026-08-07 对 docs.rs 逐字段核实）：
// - Message 是 enum：System{content} / User{content: OneOrMany<UserContent>}
//   / Assistant{id, content: OneOrMany<AssistantContent>}（不再是 struct）；
// - 工具调用关联：tool_results 回填的 ToolResult.id 必须等于
//   PendingToolCall.tool_call.id，且每个待执行调用都必须被应答；
// - PendingToolCall.preresolved_result 非 None 时驱动方必须原样返回该内容，
//   不执行工具（无效工具调用恢复的跳过语义，RunController 职责，见 AG-06）；
// - Usage 口径为 input_tokens/output_tokens（映射自我们的 prompt/completion）。
// ============================================================

use std::collections::BTreeSet;

use rig_agent::agent::run::ModelTurn;
use rig_agent::completion::{Message, Usage};
use rig_core::completion::message::{
    AssistantContent, Text, ToolCall, ToolFunction, ToolResultContent, UserContent,
};
use rig_core::one_or_many::OneOrMany;

use crate::model::messages::{ModelMessage, ModelResponse, ModelRole, ModelToolCall, TokenUsage};

/// Adapter 转换错误（业务侧输入不符合协议，不做静默兜底）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// tool 角色消息缺少 tool_call_id（OpenAI 协议必填）
    MissingToolCallId { index: usize },
    /// assistant 消息既无文本也无工具调用（无法构造合法的 rig 消息）
    EmptyAssistantTurn { index: usize },
    /// 模型响应既无文本也无工具调用（provider 协议异常）
    EmptyModelResponse,
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToolCallId { index } => {
                write!(f, "第 {} 条 tool 消息缺少 tool_call_id", index)
            }
            Self::EmptyAssistantTurn { index } => {
                write!(f, "第 {} 条 assistant 消息无文本且无工具调用", index)
            }
            Self::EmptyModelResponse => write!(f, "模型响应为空（无文本且无工具调用）"),
        }
    }
}

impl std::error::Error for AdapterError {}

// ----------------------------------------------------------------
// Adapter 1a：SophoNote ModelMessage → rig Message（喂给 AgentRun 的历史）
// ----------------------------------------------------------------

/// 将自有转录转换为 rig 历史。顺序保持；每条 tool 消息对应一个
/// 只含单个 ToolResult 的 User 消息（OpenAI-compatible 口径）。
pub fn to_rig_history(messages: &[ModelMessage]) -> Result<Vec<Message>, AdapterError> {
    let mut out = Vec::with_capacity(messages.len());
    for (index, msg) in messages.iter().enumerate() {
        let rig_msg = match msg.role {
            ModelRole::System => Message::system(msg.content.clone()),
            ModelRole::User => Message::user(msg.content.clone()),
            ModelRole::Assistant => {
                let mut parts: Vec<AssistantContent> = Vec::new();
                if !msg.content.is_empty() {
                    parts.push(AssistantContent::text(msg.content.clone()));
                }
                for tc in &msg.tool_calls {
                    parts.push(AssistantContent::ToolCall(rig_tool_call(tc)));
                }
                if parts.is_empty() {
                    return Err(AdapterError::EmptyAssistantTurn { index });
                }
                let content = OneOrMany::from_iter_optional(parts)
                    .expect("parts 非空时 from_iter_optional 必成功");
                Message::Assistant { id: None, content }
            }
            ModelRole::Tool => {
                let Some(call_id) = msg.tool_call_id.clone() else {
                    return Err(AdapterError::MissingToolCallId { index });
                };
                Message::tool_result(call_id, msg.content.clone())
            }
        };
        out.push(rig_msg);
    }
    Ok(out)
}

fn rig_tool_call(tc: &ModelToolCall) -> ToolCall {
    ToolCall::new(
        tc.id.clone(),
        ToolFunction::new(tc.name.clone(), tc.arguments.clone()),
    )
}

// ----------------------------------------------------------------
// Adapter 1b：rig Message → SophoNote ModelMessage（转录还原，仅重建用）
// ----------------------------------------------------------------

/// 将 rig 会话还原为自有转录（有损：多模态/推理内容降级或忽略，
/// provider 消息 id 不保留）。用于 Run 结束后落自有 agent_messages。
pub fn from_rig_history(messages: &[Message]) -> Vec<ModelMessage> {
    let mut out: Vec<ModelMessage> = Vec::new();
    for msg in messages {
        match msg {
            Message::System { content } => out.push(ModelMessage::system(content.clone())),
            Message::User { content } => {
                // 顺序保真：文本累积进缓冲，遇到 ToolResult 先 flush 再逐条落 tool 消息
                let mut text_buf: Vec<String> = Vec::new();
                for item in content.iter() {
                    match item {
                        UserContent::Text(text) => text_buf.push(text.text.clone()),
                        UserContent::ToolResult(result) => {
                            if !text_buf.is_empty() {
                                out.push(ModelMessage::user(text_buf.join("\n")));
                                text_buf.clear();
                            }
                            let mut tool_msg = ModelMessage::new(
                                ModelRole::Tool,
                                tool_result_text(&result.content),
                            );
                            tool_msg.tool_call_id = Some(result.id.clone());
                            out.push(tool_msg);
                        }
                        // 多模态内容 Phase 1 不进转录（图片/音频/文档）
                        _ => {}
                    }
                }
                if !text_buf.is_empty() {
                    out.push(ModelMessage::user(text_buf.join("\n")));
                }
            }
            Message::Assistant { content, .. } => {
                let mut text_buf: Vec<String> = Vec::new();
                let mut calls: Vec<ModelToolCall> = Vec::new();
                for item in content.iter() {
                    match item {
                        AssistantContent::Text(text) => text_buf.push(text.text.clone()),
                        AssistantContent::ToolCall(tc) => calls.push(from_rig_tool_call(tc)),
                        // Reasoning/Image 不进自有转录（Phase 1）
                        _ => {}
                    }
                }
                let mut assistant = ModelMessage::new(ModelRole::Assistant, text_buf.join("\n"));
                assistant.tool_calls = calls;
                out.push(assistant);
            }
        }
    }
    out
}

/// 工具结果内容 → 纯文本（Json 序列化、Image 占位符）
fn tool_result_text(content: &OneOrMany<ToolResultContent>) -> String {
    let parts: Vec<String> = content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text(text) => text.text.clone(),
            ToolResultContent::Json { value } => value.to_string(),
            ToolResultContent::Image(_) => "[image]".to_string(),
        })
        .collect();
    parts.join("\n")
}

// ----------------------------------------------------------------
// Adapter 2：SophoNote ModelResponse → rig ModelTurn（回填状态机）
// ----------------------------------------------------------------

/// 把自有 Gateway 的响应转换为状态机回填用的 ModelTurn。
/// `executable_tool_names` = 本轮请求实际下发给模型的工具集；
/// `allowed_tool_names` = 当前 ToolChoice 允许的工具集（无限制时与前者相同）。
/// 工具白名单校验由状态机负责（非法调用走 resolve_invalid_tool_call）。
pub fn model_response_to_turn(
    response: &ModelResponse,
    executable_tool_names: BTreeSet<String>,
    allowed_tool_names: BTreeSet<String>,
) -> Result<ModelTurn, AdapterError> {
    let mut parts: Vec<AssistantContent> = Vec::new();
    if !response.content.is_empty() {
        parts.push(AssistantContent::text(response.content.clone()));
    }
    for tc in &response.tool_calls {
        parts.push(AssistantContent::ToolCall(rig_tool_call(tc)));
    }
    let choice = OneOrMany::from_iter_optional(parts).ok_or(AdapterError::EmptyModelResponse)?;
    Ok(ModelTurn::new(
        response.provider_request_id.clone(),
        choice,
        usage_from_token_usage(&response.usage),
        executable_tool_names,
        allowed_tool_names,
    ))
}

/// TokenUsage（prompt/completion 口径）→ rig Usage（input/output 口径）
pub fn usage_from_token_usage(usage: &TokenUsage) -> Usage {
    Usage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        cached_input_tokens: 0,
        cache_creation_input_tokens: 0,
        tool_use_prompt_tokens: 0,
        reasoning_tokens: 0,
    }
}

/// rig Usage → TokenUsage（Run 结束后聚合用量落库用）
pub fn token_usage_from_usage(usage: &Usage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    }
}

// ----------------------------------------------------------------
// Adapter 3：工具调用/工具结果 ↔ rig 契约
// ----------------------------------------------------------------

/// rig ToolCall → 自有 ModelToolCall（交给 ToolGateway 执行）
pub fn from_rig_tool_call(tool_call: &ToolCall) -> ModelToolCall {
    ModelToolCall {
        id: tool_call.id.clone(),
        name: tool_call.function.name.clone(),
        arguments: tool_call.function.arguments.clone(),
    }
}

/// 构造回填状态机的工具结果 UserContent。
/// `call_id` 必须等于 PendingToolCall.tool_call.id（状态机按 id 多集匹配，
/// 漏答或错答都会被 tool_results 拒绝）。model_text 是给模型看的简洁文本。
pub fn rig_tool_result(call_id: impl Into<String>, model_text: impl Into<String>) -> UserContent {
    UserContent::tool_result(
        call_id,
        OneOrMany::one(ToolResultContent::Text(Text::new(model_text))),
    )
}

/// 工具执行失败时的回填内容（工具层错误不进异常路径——状态机需要
/// 「每个调用都有应答」，失败原因作为文本交还给模型自行决策）。
pub fn rig_tool_error(call_id: impl Into<String>, reason: impl Into<String>) -> UserContent {
    rig_tool_result(call_id, format!("工具执行失败: {}", reason.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::messages::{FinishReason, ToolDefinition};

    fn ok_usage() -> TokenUsage {
        TokenUsage {
            prompt_tokens: 11,
            completion_tokens: 7,
            total_tokens: 18,
        }
    }

    fn text_response(content: &str) -> ModelResponse {
        ModelResponse {
            content: content.to_string(),
            reasoning: None,
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: ok_usage(),
            provider_request_id: Some("resp-1".into()),
        }
    }

    fn tool_names(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    // ---- Adapter 1：历史双向转换 ----

    #[test]
    fn roundtrip_plain_text_history() {
        let msgs = vec![
            ModelMessage::system("你是 SophoNote 助手"),
            ModelMessage::user("整理我的项目文档"),
            ModelMessage::new(ModelRole::Assistant, "好的，先检索"),
        ];
        let rig_msgs = to_rig_history(&msgs).expect("转换成功");
        assert_eq!(rig_msgs.len(), 3);
        let back = from_rig_history(&rig_msgs);
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].role, ModelRole::System);
        assert_eq!(back[1].content, "整理我的项目文档");
        assert_eq!(back[2].role, ModelRole::Assistant);
        assert_eq!(back[2].content, "好的，先检索");
    }

    #[test]
    fn assistant_with_tool_calls_maps_to_rig_parts() {
        let mut assistant = ModelMessage::new(ModelRole::Assistant, "");
        assistant.tool_calls.push(ModelToolCall {
            id: "call-1".into(),
            name: "document.search".into(),
            arguments: serde_json::json!({"query": "LangGraph"}),
        });
        let rig_msgs = to_rig_history(&[assistant]).expect("转换成功");
        let Message::Assistant { content, id } = &rig_msgs[0] else {
            panic!("应为 Assistant 消息");
        };
        assert!(id.is_none());
        // 空文本 + 单工具调用：只有一个 ToolCall part（不生成空 Text）
        let parts: Vec<_> = content.iter().collect();
        assert_eq!(parts.len(), 1);
        assert!(matches!(parts[0], AssistantContent::ToolCall(_)));
    }

    #[test]
    fn empty_assistant_turn_is_rejected() {
        let msgs = vec![ModelMessage::new(ModelRole::Assistant, "")];
        let err = to_rig_history(&msgs).unwrap_err();
        assert_eq!(err, AdapterError::EmptyAssistantTurn { index: 0 });
    }

    #[test]
    fn tool_message_requires_call_id() {
        let mut tool = ModelMessage::new(ModelRole::Tool, "结果");
        tool.tool_call_id = None;
        let err = to_rig_history(&[tool]).unwrap_err();
        assert_eq!(err, AdapterError::MissingToolCallId { index: 0 });
    }

    #[test]
    fn tool_message_becomes_user_tool_result() {
        let mut tool = ModelMessage::new(ModelRole::Tool, "搜到 3 条");
        tool.tool_call_id = Some("call-1".into());
        let rig_msgs = to_rig_history(&[tool]).expect("转换成功");
        let Message::User { content } = &rig_msgs[0] else {
            panic!("tool 消息应映射为 User 消息");
        };
        let UserContent::ToolResult(result) = content.iter().next().expect("应有内容") else {
            panic!("应为 ToolResult");
        };
        assert_eq!(result.id, "call-1");
        assert_eq!(tool_result_text(&result.content), "搜到 3 条");
    }

    #[test]
    fn from_rig_history_restores_tool_calls_and_results() {
        // 模拟一轮完整往返：assistant(文本+工具调用) → user(工具结果) → assistant(终答)
        let assistant = Message::Assistant {
            id: Some("msg-1".into()),
            content: OneOrMany::many(vec![
                AssistantContent::text("我来搜索"),
                AssistantContent::ToolCall(ToolCall::new(
                    "call-9".into(),
                    ToolFunction::new("document.search".into(), serde_json::json!({"q": "rig"})),
                )),
            ])
            .expect("两项内容"),
        };
        let tool_result = Message::tool_result("call-9", "命中 2 条");
        let final_answer = Message::assistant("结论：……");

        let back = from_rig_history(&[assistant, tool_result, final_answer]);
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].content, "我来搜索");
        assert_eq!(back[0].tool_calls.len(), 1);
        assert_eq!(back[0].tool_calls[0].id, "call-9");
        assert_eq!(back[0].tool_calls[0].name, "document.search");
        assert_eq!(
            back[0].tool_calls[0].arguments,
            serde_json::json!({"q": "rig"})
        );
        assert_eq!(back[1].role, ModelRole::Tool);
        assert_eq!(back[1].tool_call_id.as_deref(), Some("call-9"));
        assert_eq!(back[1].content, "命中 2 条");
        assert_eq!(back[2].role, ModelRole::Assistant);
        assert_eq!(back[2].content, "结论：……");
    }

    // ---- Adapter 2：ModelResponse → ModelTurn ----

    #[test]
    fn text_only_response_becomes_text_turn() {
        let resp = text_response("最终答案");
        let turn = model_response_to_turn(&resp, tool_names(&["a"]), tool_names(&["a"]))
            .expect("转换成功");
        assert_eq!(turn.message_id.as_deref(), Some("resp-1"));
        let parts: Vec<_> = turn.choice.iter().collect();
        assert_eq!(parts.len(), 1);
        assert!(matches!(parts[0], AssistantContent::Text(_)));
        assert_eq!(turn.usage.input_tokens, 11);
        assert_eq!(turn.usage.output_tokens, 7);
        assert_eq!(turn.usage.total_tokens, 18);
        assert_eq!(turn.executable_tool_names, tool_names(&["a"]));
        assert_eq!(turn.allowed_tool_names, tool_names(&["a"]));
    }

    #[test]
    fn tool_call_response_keeps_text_and_calls() {
        let mut resp = text_response("先查一下");
        resp.finish_reason = FinishReason::ToolCalls;
        resp.tool_calls.push(ModelToolCall {
            id: "call-2".into(),
            name: "document.get".into(),
            arguments: serde_json::json!({"document_id": "d-1"}),
        });
        let turn = model_response_to_turn(
            &resp,
            tool_names(&["document.get", "document.search"]),
            tool_names(&["document.get", "document.search"]),
        )
        .expect("转换成功");
        let parts: Vec<_> = turn.choice.iter().collect();
        assert_eq!(parts.len(), 2);
        assert!(matches!(parts[0], AssistantContent::Text(_)));
        let AssistantContent::ToolCall(tc) = parts[1] else {
            panic!("第二项应为 ToolCall");
        };
        assert_eq!(tc.id, "call-2");
        assert_eq!(tc.function.name, "document.get");
    }

    #[test]
    fn empty_response_is_rejected() {
        let resp = text_response("");
        let err = model_response_to_turn(&resp, BTreeSet::new(), BTreeSet::new()).unwrap_err();
        assert_eq!(err, AdapterError::EmptyModelResponse);
    }

    // ---- Adapter 3：工具调用/结果 ----

    #[test]
    fn rig_tool_result_matches_pending_call_id() {
        let result = rig_tool_result("call-3", "成功创建文档");
        let UserContent::ToolResult(tr) = result else {
            panic!("应为 ToolResult");
        };
        assert_eq!(tr.id, "call-3");
        assert_eq!(tool_result_text(&tr.content), "成功创建文档");
    }

    #[test]
    fn rig_tool_error_carries_reason() {
        let result = rig_tool_error("call-4", "超时");
        let UserContent::ToolResult(tr) = result else {
            panic!("应为 ToolResult");
        };
        assert_eq!(tr.id, "call-4");
        assert_eq!(tool_result_text(&tr.content), "工具执行失败: 超时");
    }

    #[test]
    fn from_rig_tool_call_maps_fields() {
        let tc = ToolCall::new(
            "call-5".into(),
            ToolFunction::new("item.get".into(), serde_json::json!({"id": 42})),
        );
        let ours = from_rig_tool_call(&tc);
        assert_eq!(ours.id, "call-5");
        assert_eq!(ours.name, "item.get");
        assert_eq!(ours.arguments, serde_json::json!({"id": 42}));
    }

    /// 防止 ToolDefinition 结构意外漂移（AG-06 下发工具定义时依赖这三个字段）
    #[test]
    fn tool_definition_shape_is_stable() {
        let def = ToolDefinition {
            name: "document.search".into(),
            description: "搜索文档".into(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        assert_eq!(def.name, "document.search");
    }
}
