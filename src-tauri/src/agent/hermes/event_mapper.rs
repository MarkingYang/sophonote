//! stub / 真 Hermes SSE JSON → SophoNote AgentEventPayload（H4 v2 + 真机联调）。
//!
//! 真 Hermes 0.20：字段名为 `event`（非 stub 的 `type`）；
//! `message.delta` 文本在 `delta`；`run.completed` 正文在 `output`；
//! 审批为 `approval.request`。

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use super::user_facing::{
    looks_like_internal_ops_leak, sanitize_user_facing_delta, sanitize_user_facing_text,
};
use crate::agent::events::AgentEventPayload;

fn event_name(ev: &Value) -> Option<&str> {
    ev.get("event")
        .and_then(|v| v.as_str())
        .or_else(|| ev.get("type").and_then(|v| v.as_str()))
}

/// Lease 是 Run 内部短期能力凭据，Hermes 工具事件可以回显原始 arguments；
/// RunStore、过程卡与审批 UI 都不得保存或展示它。保留 articleId 等业务参数。
fn public_tool_arguments(ev: &Value) -> String {
    let mut arguments = ev
        .get("arguments")
        .or_else(|| ev.get("args"))
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    if let Some(object) = arguments.as_object_mut() {
        object.remove("leaseId");
        object.remove("lease_id");
    }
    arguments.to_string()
}

/// Hermes `/v1/runs` 的旧版工具事件没有 call_id，且字段名使用 `tool`。
/// 映射器跨事件保存同名 FIFO，保证 started/completed 能归到同一张卡；
/// 新版协议带稳定 call_id 时直接沿用。
#[derive(Debug, Default)]
pub struct HermesEventMapper {
    next_tool_id: u64,
    pending_tools: HashMap<String, VecDeque<String>>,
    saw_reasoning_delta: bool,
}

impl HermesEventMapper {
    /// Hermes Gateway `params.type + params.payload` → SophoNote 稳定事件。
    /// Gateway 字段保持原生；仅在 Surface 边界做命名归一，不清洗模型正文、
    /// 不合成假步骤。敏感数据应在结构化工具字段中处理，不能用正则改写回答。
    pub fn map_gateway(&mut self, ty: &str, payload: &Value) -> Option<AgentEventPayload> {
        match ty {
            "tool.start" | "tool.progress" => {
                let call_id = payload
                    .get("tool_id")
                    .or_else(|| payload.get("call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("hermes-tool")
                    .to_string();
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                Some(AgentEventPayload::ToolStarted {
                    call_id,
                    name,
                    arguments_json: payload
                        .get("args_text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            payload
                                .get("args")
                                .cloned()
                                .unwrap_or(Value::Object(Default::default()))
                                .to_string()
                        }),
                })
            }
            "tool.complete" => {
                let call_id = payload
                    .get("tool_id")
                    .or_else(|| payload.get("call_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("hermes-tool")
                    .to_string();
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let result = serde_json::json!({
                    "resultText": payload.get("result_text").and_then(Value::as_str),
                    "summary": payload.get("summary").and_then(Value::as_str),
                    "inlineDiff": payload.get("inline_diff").and_then(Value::as_str),
                    "durationSeconds": payload.get("duration_s").and_then(Value::as_f64),
                    "todos": payload.get("todos").cloned().unwrap_or(Value::Null),
                });
                let error = payload
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                Some(AgentEventPayload::ToolCompleted {
                    call_id,
                    name,
                    ok: error.is_none(),
                    error,
                    preresolved: false,
                    structured: result,
                    ui_artifact: None,
                    truncated: false,
                    provenance: Vec::new(),
                })
            }
            "message.delta" => gateway_text(payload, &["text", "rendered"])
                .map(|text| AgentEventPayload::MessageDelta { text, index: None }),
            "message.interim" => {
                gateway_text(payload, &["text"]).map(|text| AgentEventPayload::MessageInterim {
                    text,
                    already_streamed: payload
                        .get("already_streamed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            }
            "message.complete" => gateway_text(payload, &["text", "rendered"])
                .map(|text| AgentEventPayload::MessageCompleted { text }),
            "reasoning.delta" => {
                self.saw_reasoning_delta = true;
                gateway_text(payload, &["text"])
                    .map(|text| AgentEventPayload::ReasoningDelta { text })
            }
            "reasoning.available" if self.saw_reasoning_delta => None,
            "reasoning.available" => gateway_text(payload, &["text"])
                .map(|text| AgentEventPayload::ReasoningDelta { text }),
            // Hermes Desktop 同样忽略该事件：它是 kawaii spinner 状态，不是推理正文。
            "thinking.delta" => None,
            "status.update" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                (!text.is_empty()).then(|| AgentEventPayload::ReasoningDelta {
                    text: text.to_string(),
                })
            }
            "approval.request" => Some(AgentEventPayload::ApprovalRequired {
                approval_id: "gateway-approval".into(),
                tool_name: payload
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("approval")
                    .to_string(),
                arguments_json: payload
                    .get("command")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()))
                    .to_string(),
                choices: payload
                    .get("choices")
                    .and_then(Value::as_array)
                    .map(|choices| {
                        choices
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["once".into(), "session".into(), "deny".into()]),
            }),
            "clarify.request" => Some(AgentEventPayload::ClarifyRequired {
                request_id: payload
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                question: payload
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                choices: payload
                    .get("choices")
                    .and_then(Value::as_array)
                    .map(|choices| {
                        choices
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            _ => None,
        }
    }

    pub fn map(&mut self, ev: &Value) -> Option<AgentEventPayload> {
        match event_name(ev)? {
            "tool.started" => {
                let name = tool_name(ev)?;
                let call_id = ev
                    .get("call_id")
                    .or_else(|| ev.get("tool_call_id"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        self.next_tool_id += 1;
                        format!("hermes-tool-{}", self.next_tool_id)
                    });
                self.pending_tools
                    .entry(name.clone())
                    .or_default()
                    .push_back(call_id.clone());
                let arguments_json = public_tool_arguments(ev);
                Some(AgentEventPayload::ToolStarted {
                    call_id,
                    name,
                    arguments_json,
                })
            }
            "tool.completed" => {
                let name = tool_name(ev)?;
                let explicit_call_id = ev
                    .get("call_id")
                    .or_else(|| ev.get("tool_call_id"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                let pending = self.pending_tools.entry(name.clone()).or_default();
                let call_id = if let Some(call_id) = explicit_call_id {
                    if let Some(index) = pending.iter().position(|value| value == &call_id) {
                        pending.remove(index);
                    }
                    call_id
                } else {
                    pending.pop_front().unwrap_or_else(|| {
                        self.next_tool_id += 1;
                        format!("hermes-tool-{}", self.next_tool_id)
                    })
                };
                let error_flag = ev.get("error").and_then(Value::as_bool).unwrap_or(false);
                let ok = ev.get("ok").and_then(Value::as_bool).unwrap_or(!error_flag);
                Some(AgentEventPayload::ToolCompleted {
                    call_id,
                    name,
                    ok,
                    error: if ok {
                        None
                    } else {
                        Some(
                            ev.get("error")
                                .and_then(Value::as_str)
                                .or_else(|| ev.get("output_text").and_then(Value::as_str))
                                .unwrap_or("tool failed")
                                .to_string(),
                        )
                    },
                    preresolved: false,
                    structured: Value::Null,
                    ui_artifact: None,
                    truncated: false,
                    provenance: Vec::new(),
                })
            }
            "reasoning.delta" => {
                self.saw_reasoning_delta = true;
                map_stub_event(ev)
            }
            "reasoning.available" if self.saw_reasoning_delta => None,
            _ => map_stub_event(ev),
        }
    }
}

fn gateway_text(payload: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| payload.get(*field).and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn tool_name(ev: &Value) -> Option<String> {
    ev.get("name")
        .or_else(|| ev.get("tool"))
        .or_else(|| ev.get("tool_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 将 stub / 真 Hermes 事件映射为 AgentEventPayload；无法映射则返回 None。
pub fn map_stub_event(ev: &Value) -> Option<AgentEventPayload> {
    let ty = event_name(ev)?;
    match ty {
        "run.started" => {
            let user_message = ev
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(AgentEventPayload::RunStarted {
                user_message,
                max_turns: 1,
                context: None,
                skill: None,
            })
        }
        "tool.started" => {
            let call_id = ev.get("call_id")?.as_str()?.to_string();
            let name = ev.get("name")?.as_str()?.to_string();
            let arguments_json = public_tool_arguments(ev);
            Some(AgentEventPayload::ToolStarted {
                call_id,
                name,
                arguments_json,
            })
        }
        "tool.completed" => {
            let call_id = ev.get("call_id")?.as_str()?.to_string();
            let name = ev.get("name")?.as_str()?.to_string();
            let ok = ev.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(AgentEventPayload::ToolCompleted {
                call_id,
                name,
                ok,
                error: if ok {
                    None
                } else {
                    Some(
                        ev.get("output_text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool failed")
                            .to_string(),
                    )
                },
                preresolved: false,
                structured: Value::Null,
                ui_artifact: None,
                truncated: false,
                provenance: Vec::new(),
            })
        }
        "message.delta" => {
            let raw = ev
                .get("delta")
                .or_else(|| ev.get("text"))
                .and_then(|v| v.as_str())?;
            // 流式增量一旦含内部调用信息整帧丢弃，避免半句泄露
            if looks_like_internal_ops_leak(raw) {
                return None;
            }
            let text = sanitize_user_facing_delta(raw);
            if text.is_empty() {
                return None;
            }
            let index = ev.get("index").and_then(|v| v.as_u64()).map(|n| n as u32);
            Some(AgentEventPayload::MessageDelta { text, index })
        }
        "message.completed" => {
            let raw = ev
                .get("text")
                .or_else(|| ev.get("content"))
                .or_else(|| ev.get("output"))
                .and_then(|v| v.as_str())?;
            let text = sanitize_user_facing_text(raw);
            if text.is_empty() || looks_like_internal_ops_leak(&text) {
                return None;
            }
            Some(AgentEventPayload::MessageCompleted { text })
        }
        "approval.required" | "approval.request" => Some(AgentEventPayload::ApprovalRequired {
            approval_id: ev
                .get("approval_id")
                .or_else(|| ev.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("approval")
                .to_string(),
            tool_name: ev
                .get("tool_name")
                .or_else(|| ev.get("tool"))
                .or_else(|| ev.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("tool")
                .to_string(),
            arguments_json: if ev.get("arguments").is_some() || ev.get("args").is_some() {
                public_tool_arguments(ev)
            } else {
                ev.get("command")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()))
                    .to_string()
            },
            choices: ev
                .get("choices")
                .and_then(Value::as_array)
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }),
        "engine.degraded" => Some(AgentEventPayload::EngineDegraded {
            reason: ev
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("degraded")
                .to_string(),
            reconnecting: ev
                .get("reconnecting")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }),
        "run.completed" => {
            let outcome = ev
                .get("outcome")
                .and_then(|v| v.as_str())
                .unwrap_or("completed")
                .to_string();
            let raw = ev
                .get("final_answer")
                .or_else(|| ev.get("output"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let final_answer = sanitize_user_facing_text(raw);
            let final_answer = if looks_like_internal_ops_leak(&final_answer) {
                String::new()
            } else {
                final_answer
            };
            Some(AgentEventPayload::RunCompleted {
                outcome,
                final_answer,
                model_calls: 0,
            })
        }
        "run.failed" => Some(AgentEventPayload::RunFailed {
            outcome: ev
                .get("outcome")
                .and_then(|v| v.as_str())
                .unwrap_or("failed")
                .to_string(),
            error: ev
                .get("error")
                .or_else(|| ev.get("output"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
        }),
        "run.cancelled" => Some(AgentEventPayload::RunCancelled {
            reason: ev
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("cancelled")
                .to_string(),
        }),
        // 真 Hermes 推理：映射为 ReasoningDelta（UI 可折叠，不进正文）
        "reasoning.available" | "reasoning.delta" | "reasoning" => {
            let raw = ev
                .get("delta")
                .or_else(|| ev.get("text"))
                .or_else(|| ev.get("content"))
                .or_else(|| ev.get("reasoning"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if raw.is_empty() {
                // 仅标记可用、无正文：发空增量占位，前端可显示「思考中」
                Some(AgentEventPayload::ReasoningDelta {
                    text: String::new(),
                })
            } else if looks_like_internal_ops_leak(raw) {
                None
            } else {
                let text = sanitize_user_facing_delta(raw);
                if text.is_empty() {
                    None
                } else {
                    Some(AgentEventPayload::ReasoningDelta { text })
                }
            }
        }
        // 显式 thinking_end（若 Hermes 发出）；否则前端由首条 message_delta 合成
        "reasoning.end" | "reasoning.completed" | "thinking_end" => {
            Some(AgentEventPayload::ReasoningCompleted {})
        }
        // 旧 stub 整包 message：映射为 message_completed（兼容）
        "message" => {
            let raw = ev.get("content").and_then(|v| v.as_str())?;
            let text = sanitize_user_facing_text(raw);
            if text.is_empty() {
                return None;
            }
            Some(AgentEventPayload::MessageCompleted { text })
        }
        _ => None,
    }
}

/// SSE 帧是否为终态事件名（stub `type` 或真机 `event`）
pub fn is_terminal_event_name(name: &str) -> bool {
    matches!(name, "run.completed" | "run.failed" | "run.cancelled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tool_started() {
        let ev = serde_json::json!({
            "type": "tool.started",
            "call_id": "c1",
            "name": "list_project_documents",
            "arguments": {}
        });
        match map_stub_event(&ev) {
            Some(AgentEventPayload::ToolStarted { name, .. }) => {
                assert_eq!(name, "list_project_documents");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tool_events_never_expose_bridge_lease() {
        let ev = serde_json::json!({
            "type": "tool.started",
            "call_id": "c1",
            "name": "read_document",
            "arguments": {
                "leaseId": "lease-secret",
                "articleId": "article-1"
            }
        });
        match map_stub_event(&ev) {
            Some(AgentEventPayload::ToolStarted { arguments_json, .. }) => {
                assert!(!arguments_json.contains("lease"));
                assert!(arguments_json.contains("article-1"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn maps_message_delta() {
        let ev = serde_json::json!({"type": "message.delta", "text": "ab", "index": 0});
        match map_stub_event(&ev) {
            Some(AgentEventPayload::MessageDelta { text, index }) => {
                assert_eq!(text, "ab");
                assert_eq!(index, Some(0));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn maps_message_delta_keeps_newlines() {
        let ev = serde_json::json!({"event": "message.delta", "delta": "## 标题\n\n"});
        match map_stub_event(&ev) {
            Some(AgentEventPayload::MessageDelta { text, .. }) => {
                assert!(text.contains("## 标题"));
                assert!(text.ends_with("\n\n"), "got {text:?}");
            }
            other => panic!("unexpected {other:?}"),
        }
        let nl = serde_json::json!({"event": "message.delta", "delta": "\n"});
        match map_stub_event(&nl) {
            Some(AgentEventPayload::MessageDelta { text, .. }) => assert_eq!(text, "\n"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn maps_message_delta_keeps_punctuation_tokens() {
        for token in ["：", "，", "。", "、", "在", "于"] {
            let ev = serde_json::json!({"event": "message.delta", "delta": token});
            match map_stub_event(&ev) {
                Some(AgentEventPayload::MessageDelta { text, .. }) => {
                    assert_eq!(text, token);
                }
                other => panic!("unexpected {other:?} for {token:?}"),
            }
        }
    }

    #[test]
    fn maps_live_hermes_delta_and_completed() {
        let delta = serde_json::json!({
            "event": "message.delta",
            "run_id": "run_x",
            "delta": "po"
        });
        match map_stub_event(&delta) {
            Some(AgentEventPayload::MessageDelta { text, .. }) => assert_eq!(text, "po"),
            other => panic!("unexpected {other:?}"),
        }
        let done = serde_json::json!({
            "event": "run.completed",
            "run_id": "run_x",
            "output": "pong"
        });
        match map_stub_event(&done) {
            Some(AgentEventPayload::RunCompleted { final_answer, .. }) => {
                assert_eq!(final_answer, "pong");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn maps_reasoning_available() {
        let ev = serde_json::json!({
            "event": "reasoning.available",
            "delta": "先列文档再读"
        });
        match map_stub_event(&ev) {
            Some(AgentEventPayload::ReasoningDelta { text }) => {
                assert_eq!(text, "先列文档再读");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn maps_legacy_hermes_tool_aliases_with_stable_pairing() {
        let mut mapper = HermesEventMapper::default();
        let started = serde_json::json!({
            "event": "tool.started",
            "tool": "read_file",
            "preview": "notes/a.md"
        });
        let completed = serde_json::json!({
            "event": "tool.completed",
            "tool": "read_file",
            "duration": 0.094,
            "error": false
        });
        let call_id = match mapper.map(&started) {
            Some(AgentEventPayload::ToolStarted { call_id, name, .. }) => {
                assert_eq!(name, "read_file");
                call_id
            }
            other => panic!("unexpected {other:?}"),
        };
        match mapper.map(&completed) {
            Some(AgentEventPayload::ToolCompleted {
                call_id: completed_id,
                name,
                ok,
                ..
            }) => {
                assert_eq!(completed_id, call_id);
                assert_eq!(name, "read_file");
                assert!(ok);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn drops_available_echo_after_live_reasoning_delta() {
        let mut mapper = HermesEventMapper::default();
        assert!(matches!(
            mapper.map(&serde_json::json!({
                "event": "reasoning.delta",
                "delta": "正在检查"
            })),
            Some(AgentEventPayload::ReasoningDelta { .. })
        ));
        assert!(mapper
            .map(&serde_json::json!({
                "event": "reasoning.available",
                "text": "正在检查"
            }))
            .is_none());
    }

    #[test]
    fn maps_reasoning_end_to_completed() {
        let ev = serde_json::json!({"event": "reasoning.end"});
        match map_stub_event(&ev) {
            Some(AgentEventPayload::ReasoningCompleted {}) => {}
            other => panic!("unexpected {other:?}"),
        }
        let ev2 = serde_json::json!({"event": "thinking_end"});
        match map_stub_event(&ev2) {
            Some(AgentEventPayload::ReasoningCompleted {}) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn drops_bridge_ops_leak_from_user_facing() {
        let ev = serde_json::json!({
            "event": "message.delta",
            "delta": "SophoNote MCP 桥接在 localhost:56946。直接用 curl 调:"
        });
        assert!(map_stub_event(&ev).is_none());
        let done = serde_json::json!({
            "event": "run.completed",
            "output": "SophoNote MCP 桥接在 localhost:1。正文结论。"
        });
        match map_stub_event(&done) {
            Some(AgentEventPayload::RunCompleted { final_answer, .. }) => {
                assert!(!final_answer.to_ascii_lowercase().contains("mcp"));
                assert!(!final_answer.contains("localhost"));
                assert!(final_answer.contains("正文结论"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn gateway_preserves_hermes_text_verbatim() {
        let mut mapper = HermesEventMapper::default();
        let payload = serde_json::json!({
            "text": "Open /tmp/example/index.html via http://localhost:3000\n\n**完成**"
        });
        match mapper.map_gateway("message.delta", &payload) {
            Some(AgentEventPayload::MessageDelta { text, .. }) => {
                assert_eq!(text, payload["text"].as_str().unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn gateway_maps_interim_boundary() {
        let mut mapper = HermesEventMapper::default();
        match mapper.map_gateway(
            "message.interim",
            &serde_json::json!({"text":"先检查项目。","already_streamed":true}),
        ) {
            Some(AgentEventPayload::MessageInterim {
                text,
                already_streamed,
            }) => {
                assert_eq!(text, "先检查项目。");
                assert!(already_streamed);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
