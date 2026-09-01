//! Hermes Client Surface 引擎。
//!
//! 产品路径只连接 `hermes serve` 的 JSON-RPC/WebSocket Gateway。SophoNote
//! 不再通过 `/v1/runs` 重建 Agent 循环，也不向 Hermes 注入 system prompt、
//! 历史、Skill 正文、Memory key 或工具 Lease。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::agent::attachments::{RunAttachmentInput, RunAttachmentKind};
use crate::agent::engine::{
    AgentEngine, EngineError, HermesFocusDocument, HermesSessionBinding, RunEnvelope,
};
use crate::agent::events::{AgentEventPayload, EventEmitter};
use crate::agent::run_controller::SpikeRunReport;
use crate::model::messages::TokenUsage;
use crate::tools::{ProvenanceRef, UiArtifact};

use super::event_mapper::HermesEventMapper;
use super::gateway_client::{
    event_payload, event_session_id, event_type, gateway_env_configured, register_run_control,
    unregister_run_control, GatewayControl, HermesGatewayConnection, HermesGatewayEndpoint,
};
use super::{ENGINE_ID, STUB_PROTOCOL_VERSION};

pub fn attached_env_configured() -> bool {
    gateway_env_configured()
}

#[derive(Clone)]
pub struct AttachedHermesEngine {
    pub endpoint: HermesGatewayEndpoint,
}

/// `session.resume` 后可继续观察的 Hermes 原生回合。
///
/// Gateway 会把 live Session 的 transport 重新绑定到当前 WebSocket；因此这个
/// connection 不能在恢复探针返回后被丢弃，必须交给后台 observer 继续消费事件。
pub struct RecoveredHermesTurn {
    pub gateway: HermesGatewayConnection,
    pub runtime_session_id: String,
    pub stored_session_id: String,
    pub active: bool,
    /// Hermes 已结束、但 SophoNote 在断连期间错过 `message.complete` 时，从原生
    /// Session transcript 恢复出的本轮最终回答。
    pub inactive_final_answer: Option<String>,
}

struct RunControlGuard(String);

impl Drop for RunControlGuard {
    fn drop(&mut self) {
        unregister_run_control(&self.0);
    }
}

const EDITABLE_START: &str = "<!-- SOPHONOTE_EDITABLE_START -->";
const EDITABLE_END: &str = "<!-- SOPHONOTE_EDITABLE_END -->";
const TITLE_START: &str = "<!-- SOPHONOTE_TITLE_START -->";
const TITLE_END: &str = "<!-- SOPHONOTE_TITLE_END -->";
const PROJECT_ACTIONS_START: &str = "<!-- SOPHONOTE_PROJECT_ACTIONS_START -->";
const PROJECT_ACTIONS_END: &str = "<!-- SOPHONOTE_PROJECT_ACTIONS_END -->";
const PROJECT_MANIFEST_START: &str = "<!-- SOPHONOTE_PROJECT_MANIFEST_START -->";
const PROJECT_MANIFEST_END: &str = "<!-- SOPHONOTE_PROJECT_MANIFEST_END -->";

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ProjectDocumentAction {
    CreateDocument {
        client_id: String,
        title: String,
        #[serde(default)]
        content: String,
        #[serde(default)]
        parent_client_id: Option<String>,
        #[serde(default)]
        parent_article_id: Option<String>,
    },
    SetDocumentParent {
        article_id: String,
        #[serde(default)]
        parent_article_id: Option<String>,
    },
}

#[derive(Clone)]
enum WorkingCopyTarget {
    Document(HermesFocusDocument),
    Selection(crate::agent::events::RunContext),
    Project,
}

#[derive(Clone)]
struct HostPatchContext {
    staged_path: std::path::PathBuf,
    target: WorkingCopyTarget,
    binding: HermesSessionBinding,
}

fn host_patch_context_for_attachment(
    binding: Option<&HermesSessionBinding>,
    staged_path: std::path::PathBuf,
    target: WorkingCopyTarget,
) -> Option<HostPatchContext> {
    // SophoNote 是 Hermes 的文档 Surface。只要本轮带有宿主文档工作副本，
    // 就必须在终态比较工作副本，而不能依赖用户是否显式选择某个 Skill。
    // 未发生变化时 emit_host_patch 会直接跳过；发生变化时仍只生成待审阅 Diff，
    // 不会绕过用户确认直接覆盖左侧原文。
    binding.map(|binding| HostPatchContext {
        staged_path,
        target,
        binding: binding.clone(),
    })
}

impl AttachedHermesEngine {
    pub fn new(endpoint: HermesGatewayEndpoint) -> Self {
        Self { endpoint }
    }

    pub fn try_from_env() -> Result<Self, EngineError> {
        HermesGatewayEndpoint::from_env()
            .map(Self::new)
            .ok_or_else(|| {
                EngineError::Unhealthy(format!(
                    "未配置 Hermes Surface Gateway：请设置 {} 与 {}",
                    super::ENV_GATEWAY_URL,
                    super::ENV_GATEWAY_TOKEN
                ))
            })
    }
}

impl AgentEngine for AttachedHermesEngine {
    fn engine_id(&self) -> &'static str {
        ENGINE_ID
    }

    fn engine_version(&self) -> &'static str {
        STUB_PROTOCOL_VERSION
    }

    fn health(&self) -> Result<(), EngineError> {
        if self.endpoint.ws_url.is_empty() || self.endpoint.token.is_empty() {
            return Err(EngineError::Unhealthy("Hermes Gateway 配置为空".into()));
        }
        Ok(())
    }

    async fn run_with_events(&self, envelope: RunEnvelope) -> Result<SpikeRunReport, EngineError> {
        self.health()?;
        let emitter = envelope.events.clone();
        if let Some(emitter) = &emitter {
            let _ = emitter.emit(AgentEventPayload::RunStarted {
                user_message: envelope.params.user.clone(),
                max_turns: envelope.params.max_turns,
                context: envelope.params.run_context.clone(),
                skill: envelope.params.run_skill.clone(),
            });
        }

        let mut gateway = HermesGatewayConnection::connect(&self.endpoint).await?;
        let (runtime_session_id, stored_session_id) = open_session(&mut gateway, &envelope).await?;

        if let Some(binding) = &envelope.hermes_session_binding {
            persist_session_binding(binding, &stored_session_id)?;
        }

        let mut native_refs = Vec::new();
        let mut host_patch_context = None;
        let selection = envelope
            .params
            .run_context
            .as_ref()
            .filter(|context| !context.selected_markdown.is_empty());
        if let Some(selection) = selection {
            let attached =
                attach_selection_context(&mut gateway, &runtime_session_id, selection).await?;
            native_refs.push(attached.ref_text);
            host_patch_context = host_patch_context_for_attachment(
                envelope.hermes_session_binding.as_ref(),
                attached.path,
                WorkingCopyTarget::Selection(selection.clone()),
            );
        } else if let Some(document) = &envelope.hermes_focus_document {
            let project_id = envelope
                .hermes_session_binding
                .as_ref()
                .and_then(|binding| binding.project_id.as_deref());
            let attached =
                attach_focus_document(&mut gateway, &runtime_session_id, document, project_id)
                    .await?;
            native_refs.push(attached.ref_text);
            host_patch_context = host_patch_context_for_attachment(
                envelope.hermes_session_binding.as_ref(),
                attached.path,
                WorkingCopyTarget::Document(document.clone()),
            );
        } else if envelope.hermes_project_context {
            if let Some(binding) = envelope.hermes_session_binding.as_ref() {
                if let Some(project_id) = binding.project_id.as_deref() {
                    let attached = attach_project_context(
                        &mut gateway,
                        &runtime_session_id,
                        binding,
                        project_id,
                    )
                    .await?;
                    native_refs.push(attached.ref_text);
                    host_patch_context = host_patch_context_for_attachment(
                        Some(binding),
                        attached.path,
                        WorkingCopyTarget::Project,
                    );
                }
            }
        }
        for attachment in &envelope.hermes_attachments {
            if let Some(reference) =
                attach_native(&mut gateway, &runtime_session_id, attachment).await?
            {
                native_refs.push(reference);
            }
        }

        let user_text = join_user_input(&envelope.params.user, &native_refs);

        let prompt_text = if let Some(command) = envelope
            .hermes_command
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            match dispatch_native_slash(&mut gateway, &runtime_session_id, command).await? {
                NativeSlashResult::Output(output) => {
                    if let Some(emitter) = &emitter {
                        let _ = emitter.emit(AgentEventPayload::MessageCompleted {
                            text: output.clone(),
                        });
                        let _ = emitter.emit(AgentEventPayload::RunCompleted {
                            outcome: "completed".into(),
                            final_answer: output.clone(),
                            model_calls: 0,
                        });
                    }
                    return Ok(SpikeRunReport {
                        outcome: "completed".into(),
                        final_answer: output,
                        model_calls: 0,
                        tool_executions: Vec::new(),
                        invalid_resolutions: 0,
                        usage: TokenUsage::default(),
                        transcript: Vec::new(),
                        error: None,
                    });
                }
                NativeSlashResult::Prompt(message) => message,
            }
        } else {
            dispatch_native_skill_if_needed(
                &mut gateway,
                &runtime_session_id,
                envelope
                    .params
                    .run_skill
                    .as_ref()
                    .map(|skill| skill.name.as_str()),
                &user_text,
            )
            .await?
        };

        gateway
            .call(
                "prompt.submit",
                json!({
                    "session_id": runtime_session_id,
                    "text": prompt_text,
                    "surface": "sophonote"
                }),
            )
            .await?;

        let report = observe_gateway_turn(
            gateway,
            runtime_session_id,
            stored_session_id,
            emitter,
            envelope.cancel.clone(),
            host_patch_context,
        )
        .await?;

        let _ = envelope.gateway;
        let _ = envelope.registry;
        let _ = envelope.observer;
        let _ = envelope.context_pack;
        let _ = envelope.model_route;
        let _ = envelope.hermes_memory_scope_key;
        let _ = envelope.hermes_input;
        let _ = envelope.hermes_provider.as_deref();
        let _ = envelope.hermes_command.as_deref();
        Ok(report)
    }
}

/// 恢复一个已经绑定到 SophoNote Thread 的 Hermes Session。只做原生 Session
/// reattach，不提交新 prompt；`active=false` 表示 Hermes 已明确没有运行中回合。
pub async fn resume_turn(
    endpoint: &HermesGatewayEndpoint,
    stored_session_id: &str,
    expected_user_message: Option<&str>,
) -> Result<RecoveredHermesTurn, EngineError> {
    let mut gateway = HermesGatewayConnection::connect(endpoint).await?;
    let resumed = gateway
        .call(
            "session.resume",
            json!({
                "session_id": stored_session_id,
                "source": "sophonote",
                "omit_messages": false
            }),
        )
        .await?;
    let runtime_session_id = required_string(&resumed, "session_id", "session.resume")?;
    let running = resumed
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let streaming = resumed.get("status").and_then(Value::as_str) == Some("streaming");
    let active = running || streaming || resumed.get("auto_continue").is_some();
    let inactive_final_answer = (!active)
        .then(|| final_answer_after_user(&resumed, expected_user_message))
        .flatten();
    Ok(RecoveredHermesTurn {
        gateway,
        runtime_session_id,
        stored_session_id: stored_session_id.to_string(),
        active,
        inactive_final_answer,
    })
}

fn final_answer_after_user(resumed: &Value, expected_user_message: Option<&str>) -> Option<String> {
    let expected = expected_user_message?.trim();
    if expected.is_empty() {
        return None;
    }
    let messages = resumed.get("messages")?.as_array()?;
    let user_index = messages.iter().rposition(|message| {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            return false;
        }
        let text = message
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        text == expected || text.starts_with(expected)
    })?;
    let following_turn = &messages[user_index + 1..];
    let turn_end = following_turn
        .iter()
        .position(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .unwrap_or(following_turn.len());
    following_turn[..turn_end]
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .and_then(|message| message.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 消费恢复后同一 Hermes 回合的剩余事件。调用方负责把返回报告写回 RunStore；
/// 连接在终态前断开时返回 Err，但不伪造 terminal，允许下一次恢复继续 reattach。
pub async fn observe_recovered_turn(
    recovered: RecoveredHermesTurn,
    emitter: Arc<EventEmitter>,
    cancel: CancellationToken,
) -> Result<SpikeRunReport, EngineError> {
    observe_gateway_turn(
        recovered.gateway,
        recovered.runtime_session_id,
        recovered.stored_session_id,
        Some(emitter),
        cancel,
        None,
    )
    .await
}

const GATEWAY_DELTA_FLUSH_MS: u64 = 48;
const GATEWAY_DELTA_TARGET_CHARS: usize = 32;

#[derive(Debug)]
enum PendingGatewayDelta {
    Message { text: String, index: Option<u32> },
    Reasoning { text: String },
}

impl PendingGatewayDelta {
    #[allow(clippy::result_large_err)] // 未合并的原事件需原样交回调用方，不能丢字段。
    fn from_payload(payload: AgentEventPayload) -> Result<Self, AgentEventPayload> {
        match payload {
            AgentEventPayload::MessageDelta { text, index } => Ok(Self::Message { text, index }),
            AgentEventPayload::ReasoningDelta { text } => Ok(Self::Reasoning { text }),
            other => Err(other),
        }
    }

    #[allow(clippy::result_large_err)] // 合并失败时调用方继续处理同一个完整事件。
    fn append(&mut self, payload: AgentEventPayload) -> Result<(), AgentEventPayload> {
        match (self, payload) {
            (Self::Message { text, .. }, AgentEventPayload::MessageDelta { text: next, .. }) => {
                text.push_str(&next);
                Ok(())
            }
            (Self::Reasoning { text }, AgentEventPayload::ReasoningDelta { text: next }) => {
                text.push_str(&next);
                Ok(())
            }
            (_, other) => Err(other),
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Message { text, .. } | Self::Reasoning { text } => text,
        }
    }

    fn ready(&self) -> bool {
        let text = self.text();
        let chars = text.chars().count();
        chars >= GATEWAY_DELTA_TARGET_CHARS
            || (chars >= 8
                && text
                    .chars()
                    .last()
                    .is_some_and(|ch| matches!(ch, '\n' | '。' | '！' | '？' | '.' | '!' | '?')))
    }

    fn into_payload(self) -> AgentEventPayload {
        match self {
            Self::Message { text, index } => AgentEventPayload::MessageDelta { text, index },
            Self::Reasoning { text } => AgentEventPayload::ReasoningDelta { text },
        }
    }
}

fn flush_gateway_delta(pending: &mut Option<PendingGatewayDelta>, emitter: Option<&EventEmitter>) {
    let Some(delta) = pending.take() else { return };
    if let Some(emitter) = emitter {
        let _ = emitter.emit(delta.into_payload());
    }
}

async fn observe_gateway_turn(
    mut gateway: HermesGatewayConnection,
    runtime_session_id: String,
    stored_session_id: String,
    emitter: Option<Arc<EventEmitter>>,
    cancel: CancellationToken,
    host_patch_context: Option<HostPatchContext>,
) -> Result<SpikeRunReport, EngineError> {
    let mut mapper = HermesEventMapper::default();
    let mut report = empty_report();
    let mut final_answer = String::new();
    let control_run_id = emitter
        .as_ref()
        .map(|emitter| emitter.run_id().to_string())
        .unwrap_or_else(|| format!("surface-{stored_session_id}"));
    let mut controls = register_run_control(&control_run_id);
    let _control_guard = RunControlGuard(control_run_id);
    let mut pending_delta: Option<PendingGatewayDelta> = None;
    let mut delta_flush =
        tokio::time::interval(std::time::Duration::from_millis(GATEWAY_DELTA_FLUSH_MS));
    delta_flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // interval 的首个 tick 会立即完成；先消费它，避免第一个字符绕过合帧。
    delta_flush.tick().await;

    loop {
        tokio::select! {
            _ = delta_flush.tick(), if pending_delta.is_some() => {
                flush_gateway_delta(&mut pending_delta, emitter.as_deref());
            }
            _ = cancel.cancelled() => {
                flush_gateway_delta(&mut pending_delta, emitter.as_deref());
                gateway.call(
                    "session.interrupt",
                    json!({"session_id": runtime_session_id}),
                ).await?;
                report.outcome = "cancelled".into();
                if let Some(emitter) = &emitter {
                    let _ = emitter.emit(AgentEventPayload::RunCancelled {
                        reason: "用户取消".into(),
                    });
                }
                break;
            }
            control = controls.recv() => {
                let Some(control) = control else { continue; };
                match control {
                    GatewayControl::Approval { choice, all } => {
                        gateway.call("approval.respond", json!({
                            "session_id": runtime_session_id,
                            "choice": choice,
                            "all": all,
                        })).await?;
                    }
                    GatewayControl::Clarify { request_id, answer } => {
                        gateway.call("clarify.respond", json!({
                            "request_id": request_id,
                            "answer": answer,
                        })).await?;
                    }
                }
            }
            frame = gateway.next_event() => {
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        // 断线前收到的最后几个字符同样属于正式事件；先落盘再报告异常，
                        // 避免恢复会话时出现尾部文本缺失。
                        flush_gateway_delta(&mut pending_delta, emitter.as_deref());
                        return Err(EngineError::Unhealthy("Hermes Gateway 在本轮完成前断开".into()));
                    }
                    Err(error) => {
                        flush_gateway_delta(&mut pending_delta, emitter.as_deref());
                        return Err(error);
                    }
                };
                if event_session_id(&frame).is_some_and(|sid| sid != runtime_session_id) {
                    continue;
                }
                let Some(ty) = event_type(&frame) else { continue; };
                let payload = event_payload(&frame);

                let terminal = ty == "message.complete";
                if terminal {
                    let status = payload.get("status").and_then(Value::as_str).unwrap_or("completed");
                    let text = payload
                        .get("text")
                        .or_else(|| payload.get("rendered"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !text.is_empty() {
                        final_answer = text;
                    }
                    report.outcome = if status == "error" { "failed" } else { "completed" }.into();
                    report.error = if status == "error" {
                        Some(payload.get("error").and_then(Value::as_str).unwrap_or("Hermes turn failed").to_string())
                    } else { None };
                }

                if let Some(mapped) = mapper.map_gateway(ty, &payload) {
                    if let AgentEventPayload::MessageCompleted { text } = &mapped {
                        final_answer = text.clone();
                    }
                    match PendingGatewayDelta::from_payload(mapped) {
                        Ok(delta) => {
                            if let Some(current) = pending_delta.as_mut() {
                                if let Err(next_payload) = current.append(delta.into_payload()) {
                                    flush_gateway_delta(&mut pending_delta, emitter.as_deref());
                                    pending_delta = PendingGatewayDelta::from_payload(next_payload).ok();
                                }
                            } else {
                                pending_delta = Some(delta);
                            }
                            if pending_delta.as_ref().is_some_and(PendingGatewayDelta::ready) {
                                flush_gateway_delta(&mut pending_delta, emitter.as_deref());
                            }
                        }
                        Err(non_delta) => {
                            flush_gateway_delta(&mut pending_delta, emitter.as_deref());
                            if let Some(emitter) = &emitter {
                                let _ = emitter.emit(non_delta);
                            }
                        }
                    }
                }

                if terminal && report.outcome == "completed" {
                    if let Some(context) = &host_patch_context {
                        emit_host_patch(context, emitter.as_deref());
                    }
                }

                if terminal {
                    if let Some(emitter) = &emitter {
                        if report.outcome == "failed" {
                            let _ = emitter.emit(AgentEventPayload::RunFailed {
                                outcome: "failed".into(),
                                error: report.error.clone().unwrap_or_else(|| "Hermes turn failed".into()),
                            });
                        } else {
                            let _ = emitter.emit(AgentEventPayload::RunCompleted {
                                outcome: "completed".into(),
                                final_answer: final_answer.clone(),
                                model_calls: 1,
                            });
                        }
                    }
                    break;
                }
            }
        }
    }
    report.final_answer = final_answer;
    report.model_calls = usize::from(report.outcome == "completed" || report.outcome == "failed");
    Ok(report)
}

async fn open_session(
    gateway: &mut HermesGatewayConnection,
    envelope: &RunEnvelope,
) -> Result<(String, String), EngineError> {
    if let Some(stored_session_id) = envelope.hermes_session_id.as_deref() {
        let resumed = gateway
            .call(
                "session.resume",
                json!({
                    "session_id": stored_session_id,
                    "source": "sophonote",
                    "omit_messages": true
                }),
            )
            .await?;
        let runtime = required_string(&resumed, "session_id", "session.resume")?;
        if let Some(root) = envelope.hermes_workspace_root.as_deref() {
            gateway
                .call(
                    "session.cwd.set",
                    json!({"session_id": &runtime, "cwd": root}),
                )
                .await?;
        }
        if let Some(model) = envelope.hermes_model.as_deref() {
            let value = model_assignment(model, envelope.hermes_provider.as_deref());
            gateway
                .call(
                    "config.set",
                    json!({"session_id": runtime, "key": "model", "value": value}),
                )
                .await?;
        }
        return Ok((runtime, stored_session_id.to_string()));
    }

    let mut params = json!({"source":"sophonote","cols":96});
    if let Some(root) = envelope.hermes_workspace_root.as_deref() {
        params["cwd"] = Value::String(root.to_string());
    }
    if let Some(model) = envelope.hermes_model.as_deref() {
        params["model"] = Value::String(model.to_string());
    }
    if let Some(provider) = envelope.hermes_provider.as_deref() {
        params["provider"] = Value::String(provider.to_string());
    }
    let created = gateway.call("session.create", params).await?;
    Ok((
        required_string(&created, "session_id", "session.create")?,
        required_string(&created, "stored_session_id", "session.create")?,
    ))
}

fn model_assignment(model: &str, provider: Option<&str>) -> String {
    provider
        .filter(|value| !value.is_empty())
        .map(|provider| format!("{model} --provider {provider} --session"))
        .unwrap_or_else(|| model.to_string())
}

fn required_string(value: &Value, field: &str, method: &str) -> Result<String, EngineError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| EngineError::Setup(format!("{method} 缺少 {field}")))
}

fn persist_session_binding(
    binding: &HermesSessionBinding,
    stored_session_id: &str,
) -> Result<(), EngineError> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let conn = rusqlite::Connection::open(&binding.db_path)
        .map_err(|error| EngineError::Setup(format!("打开会话数据库失败: {error}")))?;
    let store = crate::agent::store::RunStore::new(conn);
    store
        .bind_thread_external_session(&binding.thread_id, stored_session_id, now_ms)
        .map_err(|error| EngineError::Setup(format!("保存 Hermes Session 映射失败: {error}")))?;
    store
        .update_run_external_meta(
            &binding.run_id,
            Some("jsonrpc+websocket"),
            None,
            Some(stored_session_id),
            Some(STUB_PROTOCOL_VERSION),
            None,
            now_ms,
        )
        .map_err(|error| EngineError::Setup(format!("保存 Hermes Run 对账信息失败: {error}")))
}

async fn attach_native(
    gateway: &mut HermesGatewayConnection,
    session_id: &str,
    attachment: &RunAttachmentInput,
) -> Result<Option<String>, EngineError> {
    match attachment.kind {
        RunAttachmentKind::Image => {
            let content_base64 = if let Some(data_url) = attachment
                .data_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                data_url.to_string()
            } else {
                let path = attachment
                    .path
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        EngineError::Setup(format!("图片附件「{}」缺少路径", attachment.name))
                    })?;
                let bytes = std::fs::read(path).map_err(|error| {
                    EngineError::Setup(format!("读取图片附件失败 {path}: {error}"))
                })?;
                STANDARD.encode(bytes)
            };
            let response = gateway
                .call(
                    "image.attach_bytes",
                    json!({
                        "session_id": session_id,
                        "content_base64": content_base64,
                        "filename": attachment.name,
                    }),
                )
                .await?;
            validate_image_attach_response(&response, &attachment.name)?;
            // 与 Hermes Desktop 一致：图片已经进入 Session attached_images，
            // prompt.submit 只发送用户原文，不再拼接 `[User attached image]`。
            Ok(None)
        }
        RunAttachmentKind::File => {
            let path = attachment
                .path
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    EngineError::Setup(format!("文件附件「{}」缺少路径", attachment.name))
                })?;
            let bytes = std::fs::read(path)
                .map_err(|error| EngineError::Setup(format!("读取附件失败 {path}: {error}")))?;
            let data_url = format!(
                "data:application/octet-stream;base64,{}",
                STANDARD.encode(bytes)
            );
            let response = gateway
                .call(
                    "file.attach",
                    json!({
                        "session_id": session_id,
                        "path": path,
                        "name": attachment.name,
                        "data_url": data_url,
                    }),
                )
                .await?;
            Ok(response
                .get("ref_text")
                .and_then(Value::as_str)
                .map(str::to_string))
        }
        RunAttachmentKind::Folder => {
            let path = attachment
                .path
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    EngineError::Setup(format!("文件夹附件「{}」缺少路径", attachment.name))
                })?;
            Ok(Some(format!("@file:{}", quote_context_ref(path))))
        }
        RunAttachmentKind::Url => Ok(attachment
            .url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)),
    }
}

/// Upload the user's explicit editor selection through Hermes' native file
/// transport. This is user-authored context, not a hidden system prompt: the
/// visible selection chip is persisted in `run_started`, while the Gateway
/// stages the bytes in the Session workspace and returns an `@file:` ref.
struct NativeFileAttachment {
    ref_text: String,
    path: std::path::PathBuf,
}

async fn attach_selection_context(
    gateway: &mut HermesGatewayConnection,
    session_id: &str,
    selection: &crate::agent::events::RunContext,
) -> Result<NativeFileAttachment, EngineError> {
    let markdown = selection_attachment_markdown(selection);
    let data_url = format!(
        "data:text/markdown;charset=utf-8;base64,{}",
        STANDARD.encode(markdown.as_bytes())
    );
    let response = gateway
        .call(
            "file.attach",
            json!({
                "session_id": session_id,
                "name": "sophonote-selection.md",
                "data_url": data_url,
            }),
        )
        .await?;
    native_file_attachment(&response, "file.attach(selection)")
}

/// Upload the current editor draft selected by the visible document chip.
/// Hermes may edit only this Session working copy. SophoNote remains the owner of
/// the real document and converts a valid end-of-turn difference into a Patch.
async fn attach_focus_document(
    gateway: &mut HermesGatewayConnection,
    session_id: &str,
    document: &HermesFocusDocument,
    project_id: Option<&str>,
) -> Result<NativeFileAttachment, EngineError> {
    let markdown = focus_document_attachment_markdown(document, project_id);
    let data_url = format!(
        "data:text/markdown;charset=utf-8;base64,{}",
        STANDARD.encode(markdown.as_bytes())
    );
    let response = gateway
        .call(
            "file.attach",
            json!({
                "session_id": session_id,
                "name": "sophonote-document.md",
                "data_url": data_url,
            }),
        )
        .await?;
    native_file_attachment(&response, "file.attach(document)")
}

/// Upload a project-scoped Host work copy. It deliberately contains only the
/// project identity and document tree manifest—not member document bodies.
/// Hermes can create or reorganize project documents by editing the bounded
/// action array; SophoNote validates and applies those operations at turn end.
async fn attach_project_context(
    gateway: &mut HermesGatewayConnection,
    session_id: &str,
    binding: &HermesSessionBinding,
    project_id: &str,
) -> Result<NativeFileAttachment, EngineError> {
    let markdown = project_attachment_markdown(binding, project_id)?;
    let data_url = format!(
        "data:text/markdown;charset=utf-8;base64,{}",
        STANDARD.encode(markdown.as_bytes())
    );
    let response = gateway
        .call(
            "file.attach",
            json!({
                "session_id": session_id,
                "name": "sophonote-project.md",
                "data_url": data_url,
            }),
        )
        .await?;
    native_file_attachment(&response, "file.attach(project)")
}

fn project_attachment_markdown(
    binding: &HermesSessionBinding,
    project_id: &str,
) -> Result<String, EngineError> {
    let conn = rusqlite::Connection::open(&binding.db_path)
        .map_err(|error| EngineError::Setup(format!("打开项目数据库失败：{error}")))?;
    let (project_name, description): (String, Option<String>) = conn
        .query_row(
            "SELECT name, description FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| EngineError::Setup(format!("读取项目上下文失败：{error}")))?;
    let mut statement = conn
        .prepare(
            "SELECT a.id, a.title, d.parent_id
             FROM project_documents d
             JOIN articles a ON a.id = d.article_id
             WHERE d.project_id = ?1
             ORDER BY a.created_at ASC, a.id ASC",
        )
        .map_err(|error| EngineError::Setup(format!("准备项目文档清单失败：{error}")))?;
    let rows = statement
        .query_map(rusqlite::params![project_id], |row| {
            Ok(json!({
                "articleId": row.get::<_, String>(0)?,
                "title": row.get::<_, String>(1)?,
                "parentArticleId": row.get::<_, Option<String>>(2)?,
            }))
        })
        .map_err(|error| EngineError::Setup(format!("读取项目文档清单失败：{error}")))?;
    let documents = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| EngineError::Setup(format!("解析项目文档清单失败：{error}")))?;
    let project_id = serde_json::to_string(project_id).unwrap_or_else(|_| "\"\"".into());
    let project_name = serde_json::to_string(&project_name).unwrap_or_else(|_| "\"\"".into());
    let description = description.unwrap_or_default();
    let manifest = serde_json::to_string_pretty(&documents)
        .map_err(|error| EngineError::Setup(format!("序列化项目文档清单失败：{error}")))?;
    Ok(format!(
        "---\nsource: sophonote-project\nprojectId: {project_id}\nprojectName: {project_name}\n---\n\n## 项目说明\n\n{description}\n\n{PROJECT_MANIFEST_START}\n{manifest}\n{PROJECT_MANIFEST_END}\n\n{PROJECT_ACTIONS_START}\n[]\n{PROJECT_ACTIONS_END}"
    ))
}

fn native_file_attachment(
    response: &Value,
    method: &str,
) -> Result<NativeFileAttachment, EngineError> {
    Ok(NativeFileAttachment {
        ref_text: required_string(response, "ref_text", method)?,
        path: std::path::PathBuf::from(required_string(response, "path", method)?),
    })
}

fn selection_attachment_markdown(selection: &crate::agent::events::RunContext) -> String {
    let title = serde_json::to_string(&selection.title).unwrap_or_else(|_| "\"\"".into());
    let article_id = serde_json::to_string(&selection.article_id).unwrap_or_else(|_| "\"\"".into());
    let hash =
        serde_json::to_string(&selection.selected_text_hash).unwrap_or_else(|_| "\"\"".into());
    format!(
        "---\nsource: sophonote-selection\narticleId: {article_id}\ntitle: {title}\nbaseVersion: {}\nselectedTextHash: {hash}\n---\n\n{EDITABLE_START}{}{EDITABLE_END}",
        selection.base_version, selection.selected_markdown,
    )
}

fn focus_document_attachment_markdown(
    document: &HermesFocusDocument,
    project_id: Option<&str>,
) -> String {
    let title = serde_json::to_string(&document.title).unwrap_or_else(|_| "\"\"".into());
    let article_id = serde_json::to_string(&document.article_id).unwrap_or_else(|_| "\"\"".into());
    let project_actions = project_id.map_or_else(String::new, |project_id| {
        let project_id = serde_json::to_string(project_id).unwrap_or_else(|_| "\"\"".into());
        format!("\nprojectId: {project_id}\n{PROJECT_ACTIONS_START}\n[]\n{PROJECT_ACTIONS_END}\n")
    });
    format!(
        "---\nsource: sophonote-document\narticleId: {article_id}\ntitle: {title}\nbaseVersion: {}\n---\n{project_actions}\n{TITLE_START}{}{TITLE_END}\n\n{EDITABLE_START}{}{EDITABLE_END}",
        document.base_version, document.title, document.markdown,
    )
}

fn validate_image_attach_response(response: &Value, name: &str) -> Result<(), EngineError> {
    if response.get("attached").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let detail = response
        .get("message")
        .or_else(|| response.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("Hermes 未确认附件已进入当前 Session");
    Err(EngineError::Setup(format!(
        "图片附件「{name}」上传失败：{detail}"
    )))
}

fn editable_title(value: &str) -> Result<&str, String> {
    if value.matches(TITLE_START).count() != 1 || value.matches(TITLE_END).count() != 1 {
        return Err("Hermes 工作副本的标题边界已丢失或重复".to_string());
    }
    let (_, tail) = value
        .split_once(TITLE_START)
        .ok_or_else(|| "Hermes 工作副本缺少标题起始边界".to_string())?;
    let (title, _) = tail
        .split_once(TITLE_END)
        .ok_or_else(|| "Hermes 工作副本缺少标题结束边界".to_string())?;
    Ok(title)
}

fn editable_markdown(value: &str) -> Result<&str, String> {
    if value.matches(EDITABLE_START).count() != 1 || value.matches(EDITABLE_END).count() != 1 {
        return Err("Hermes 工作副本的可编辑边界已丢失或重复".to_string());
    }
    let (_, tail) = value
        .split_once(EDITABLE_START)
        .ok_or_else(|| "Hermes 工作副本缺少起始边界".to_string())?;
    let (body, _) = tail
        .split_once(EDITABLE_END)
        .ok_or_else(|| "Hermes 工作副本缺少结束边界".to_string())?;
    Ok(body)
}

fn emit_host_patch(context: &HostPatchContext, emitter: Option<&EventEmitter>) {
    let Some(emitter) = emitter else {
        return;
    };
    let staged_result = std::fs::read_to_string(&context.staged_path)
        .map_err(|error| format!("无法回读 Hermes 工作副本：{error}"));
    if matches!(context.target, WorkingCopyTarget::Project) {
        match staged_result {
            Ok(staged) => emit_host_project_actions(context, &staged, emitter),
            Err(error) => {
                emit_host_project_action_result(emitter, &context.binding.run_id, Err(error))
            }
        }
        return;
    }
    let call_id = format!("host-patch-{}", context.binding.run_id);
    let (document_id, base_version, original) = match &context.target {
        WorkingCopyTarget::Document(document) => (
            document.article_id.as_str(),
            document.base_version,
            document.markdown.as_str(),
        ),
        WorkingCopyTarget::Selection(selection) => (
            selection.article_id.as_str(),
            selection.base_version,
            selection.selected_markdown.as_str(),
        ),
        WorkingCopyTarget::Project => unreachable!("project work copy handled above"),
    };
    // NEXT-042：标题改名与正文 diff 优先合并为同一审批——正文有差异时，标题提案
    // 随 Patch 审批卡一起走（此前独立 rename 卡被折叠在 <details> 里，用户看不见、
    // 永远不被批准）；仅改名回合（正文无差异）保留独立 rename 卡作为唯一决策入口。
    let host_title = if let (Ok(staged), WorkingCopyTarget::Document(document)) =
        (&staged_result, &context.target)
    {
        let decision = resolve_host_title_decision(context, document, staged);
        if let HostTitleDecision::Invalid { reason } = &decision {
            emit_host_title_failure(emitter, &context.binding.run_id, reason.clone());
        }
        emit_host_project_actions(context, staged, emitter);
        decision
    } else {
        HostTitleDecision::Unchanged
    };
    let body_changed = matches!(
        &staged_result,
        Ok(staged) if editable_markdown(staged).map(|body| body != original).unwrap_or(false)
    );
    if !body_changed {
        if let (HostTitleDecision::Proposed(title), WorkingCopyTarget::Document(document)) =
            (&host_title, &context.target)
        {
            emit_host_rename_card(context, document, title, emitter);
        }
    }
    let result = (|| -> Result<Option<crate::documents::service::PatchPreview>, String> {
        let staged = staged_result?;
        let replacement = editable_markdown(&staged)?;
        if replacement == original {
            return Ok(None);
        }
        let conn = rusqlite::Connection::open(&context.binding.db_path)
            .map_err(|error| format!("打开文档数据库失败：{error}"))?;
        let idempotency_key = format!("hermes-workcopy:{}:{}", context.binding.run_id, document_id);
        // NEXT-042：正文有差异时，标题提案并入同一 Patch 审批（随整块批准落盘）。
        let proposed_title = match (&context.target, &host_title) {
            (WorkingCopyTarget::Document(_), HostTitleDecision::Proposed(title)) => {
                Some(title.as_str())
            }
            _ => None,
        };
        let preview = match &context.target {
            WorkingCopyTarget::Document(document) => {
                crate::documents::service::preview_host_document_patch(
                    &conn,
                    &context.binding.notes_dir,
                    &document.article_id,
                    document.base_version,
                    &document.markdown,
                    replacement,
                    Some(&idempotency_key),
                    Some(&context.binding.run_id),
                    proposed_title,
                )
            }
            WorkingCopyTarget::Selection(selection) => {
                crate::documents::service::preview_scoped_patch(
                    &conn,
                    &context.binding.notes_dir,
                    &crate::documents::service::ScopedPatchRequest {
                        document_id: selection.article_id.clone(),
                        base_version: selection.base_version,
                        scope: crate::documents::service::PatchScope::Selection,
                        anchor: crate::documents::anchor::TextAnchor {
                            selected_text: selection.selected_markdown.clone(),
                            selected_text_hash: selection.selected_text_hash.clone(),
                            before_context: selection.before_context.clone(),
                            after_context: selection.after_context.clone(),
                        },
                        replacement_markdown: replacement.to_string(),
                        idempotency_key: Some(idempotency_key),
                        run_id: Some(context.binding.run_id.clone()),
                        project_gate: None,
                    },
                )
            }
            WorkingCopyTarget::Project => unreachable!("project work copy handled above"),
        }
        .map_err(|error| error.to_string())?;
        Ok(Some(preview))
    })();

    match result {
        Ok(None) => {}
        Ok(Some(preview)) => {
            let _ = emitter.emit(AgentEventPayload::ToolStarted {
                call_id: call_id.clone(),
                name: "sophonote_document_patch".into(),
                arguments_json: serde_json::json!({
                    "documentId": document_id,
                    "baseVersion": base_version,
                    "source": "hermes-session-working-copy"
                })
                .to_string(),
            });
            let structured = serde_json::to_value(&preview).unwrap_or_default();
            let provenance = vec![ProvenanceRef::new("sophonote-document")
                .with_id(document_id)
                .with_title(&preview.title)];
            let artifact = UiArtifact::new(
                "diff",
                structured.clone(),
                match &preview.proposed_title {
                    Some(new_title) => format!(
                        "已把 Hermes 对《{}》的修改送到左侧原文，共 {} 个可审阅变更块；全部批准时标题将改为《{}》。",
                        preview.title,
                        preview.hunks.len(),
                        new_title
                    ),
                    None => format!(
                        "已把 Hermes 对《{}》的修改送到左侧原文，共 {} 个可审阅变更块。",
                        preview.title,
                        preview.hunks.len()
                    ),
                },
                provenance.clone(),
            )
            .ok();
            let _ = emitter.emit(AgentEventPayload::ToolCompleted {
                call_id,
                name: "sophonote_document_patch".into(),
                ok: artifact.is_some(),
                error: artifact
                    .is_none()
                    .then(|| "无法生成安全 Diff 产物".to_string()),
                preresolved: false,
                structured,
                ui_artifact: artifact,
                truncated: false,
                provenance,
            });
        }
        Err(error) => {
            let _ = emitter.emit(AgentEventPayload::ToolStarted {
                call_id: call_id.clone(),
                name: "sophonote_document_patch".into(),
                arguments_json: serde_json::json!({
                    "documentId": document_id,
                    "baseVersion": base_version,
                    "source": "hermes-session-working-copy"
                })
                .to_string(),
            });
            let _ = emitter.emit(AgentEventPayload::ToolCompleted {
                call_id,
                name: "sophonote_document_patch".into(),
                ok: false,
                error: Some(error),
                preresolved: false,
                structured: Value::Null,
                ui_artifact: None,
                truncated: false,
                provenance: vec![],
            });
        }
    }
}

fn project_document_actions(value: &str) -> Result<Vec<ProjectDocumentAction>, String> {
    let start_count = value.matches(PROJECT_ACTIONS_START).count();
    let end_count = value.matches(PROJECT_ACTIONS_END).count();
    if start_count == 0 && end_count == 0 {
        return Ok(Vec::new());
    }
    if start_count != 1 || end_count != 1 {
        return Err("Hermes 工作副本的项目操作边界已丢失或重复".to_string());
    }
    let (_, tail) = value
        .split_once(PROJECT_ACTIONS_START)
        .ok_or_else(|| "Hermes 工作副本缺少项目操作起始边界".to_string())?;
    let (raw, _) = tail
        .split_once(PROJECT_ACTIONS_END)
        .ok_or_else(|| "Hermes 工作副本缺少项目操作结束边界".to_string())?;
    let actions: Vec<ProjectDocumentAction> =
        serde_json::from_str(raw.trim()).map_err(|error| format!("项目操作 JSON 无效：{error}"))?;
    if actions.len() > 64 {
        return Err("单轮最多创建或调整 64 个项目文档节点".to_string());
    }
    Ok(actions)
}

fn emit_host_project_actions(context: &HostPatchContext, staged: &str, emitter: &EventEmitter) {
    let Some(project_id) = context.binding.project_id.as_deref() else {
        return;
    };
    let actions = match project_document_actions(staged) {
        Ok(actions) if actions.is_empty() => return,
        Ok(actions) => actions,
        Err(error) => {
            emit_host_project_action_result(emitter, &context.binding.run_id, Err(error));
            return;
        }
    };
    let result = apply_project_document_actions(&context.binding, project_id, actions);
    emit_host_project_action_result(emitter, &context.binding.run_id, result);
}

fn apply_project_document_actions(
    binding: &HermesSessionBinding,
    project_id: &str,
    actions: Vec<ProjectDocumentAction>,
) -> Result<Value, String> {
    let conn = rusqlite::Connection::open(&binding.db_path)
        .map_err(|error| format!("打开文档数据库失败：{error}"))?;
    let mut created_ids = std::collections::HashMap::<String, String>::new();
    let mut created = Vec::new();
    let mut moved = Vec::new();

    for action in actions {
        match action {
            ProjectDocumentAction::CreateDocument {
                client_id,
                title,
                content,
                parent_client_id,
                parent_article_id,
            } => {
                let client_id = client_id.trim();
                if client_id.is_empty() || client_id.len() > 80 {
                    return Err("create_document.client_id 必须为 1–80 个字符".to_string());
                }
                if created_ids.contains_key(client_id) {
                    return Err(format!("项目操作 client_id 重复：{client_id}"));
                }
                if parent_client_id.is_some() && parent_article_id.is_some() {
                    return Err(format!(
                        "文档 {client_id} 不能同时指定 parent_client_id 与 parent_article_id"
                    ));
                }
                let parent_id = if let Some(parent_client_id) = parent_client_id {
                    Some(
                        created_ids
                            .get(parent_client_id.trim())
                            .cloned()
                            .ok_or_else(|| {
                                format!("父节点尚未创建：{parent_client_id}；请按父到子排序")
                            })?,
                    )
                } else {
                    parent_article_id
                };
                let idempotency_key = format!("hermes-tree:{}:{client_id}", binding.run_id);
                let title = title.trim();
                if title.is_empty()
                    || title.contains('\n')
                    || title.contains('\r')
                    || title.chars().count() > 200
                {
                    return Err(format!(
                        "文档 {client_id} 的标题必须为 1–200 个字符且不能包含换行"
                    ));
                }
                if let Some(parent_id) = parent_id.as_deref() {
                    let parent_count: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1 AND article_id = ?2",
                            rusqlite::params![project_id, parent_id],
                            |row| row.get(0),
                        )
                        .map_err(|error| format!("校验父文档失败：{error}"))?;
                    if parent_count != 1 {
                        return Err(format!("父文档不属于当前项目：{parent_id}"));
                    }
                }
                let idempotent_existing: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM document_operations WHERE idempotency_key = ?1 AND status = 'committed'",
                        rusqlite::params![idempotency_key],
                        |row| row.get(0),
                    )
                    .map_err(|error| format!("校验创建幂等状态失败：{error}"))?;
                if idempotent_existing == 0 {
                    let duplicate: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM project_documents d JOIN articles a ON a.id = d.article_id WHERE d.project_id = ?1 AND a.title = ?2",
                            rusqlite::params![project_id, title],
                            |row| row.get(0),
                        )
                        .map_err(|error| format!("校验项目文档重名失败：{error}"))?;
                    if duplicate > 0 {
                        return Err(format!("项目内已存在同名文档《{title}》"));
                    }
                }
                let document = crate::documents::service::create_document_in_project(
                    &conn,
                    &binding.notes_dir,
                    project_id,
                    title,
                    &content,
                    Some(&idempotency_key),
                )
                .map_err(|error| error.to_string())?;
                if let Some(parent_id) = parent_id.as_deref() {
                    crate::project_tree::set_doc_parent_in_project(
                        &conn,
                        Some(project_id),
                        &document.article_id,
                        Some(parent_id),
                    )?;
                }
                created_ids.insert(client_id.to_string(), document.article_id.clone());
                created.push(json!({
                    "clientId": client_id,
                    "articleId": document.article_id,
                    "title": document.title,
                    "parentArticleId": parent_id,
                }));
            }
            ProjectDocumentAction::SetDocumentParent {
                article_id,
                parent_article_id,
            } => {
                crate::project_tree::set_doc_parent_in_project(
                    &conn,
                    Some(project_id),
                    article_id.trim(),
                    parent_article_id.as_deref(),
                )?;
                moved.push(json!({
                    "articleId": article_id,
                    "parentArticleId": parent_article_id,
                }));
            }
        }
    }
    Ok(json!({
        "projectId": project_id,
        "created": created,
        "moved": moved,
    }))
}

fn emit_host_project_action_result(
    emitter: &EventEmitter,
    run_id: &str,
    result: Result<Value, String>,
) {
    let call_id = format!("host-project-tree-{run_id}");
    let _ = emitter.emit(AgentEventPayload::ToolStarted {
        call_id: call_id.clone(),
        name: "sophonote_project_tree".into(),
        arguments_json: "{\"source\":\"hermes-session-working-copy\"}".into(),
    });
    match result {
        Ok(structured) => {
            let _ = emitter.emit(AgentEventPayload::ToolCompleted {
                call_id,
                name: "sophonote_project_tree".into(),
                ok: true,
                error: None,
                preresolved: false,
                structured,
                ui_artifact: None,
                truncated: false,
                provenance: vec![],
            });
        }
        Err(error) => {
            let _ = emitter.emit(AgentEventPayload::ToolCompleted {
                call_id,
                name: "sophonote_project_tree".into(),
                ok: false,
                error: Some(error),
                preresolved: false,
                structured: Value::Null,
                ui_artifact: None,
                truncated: false,
                provenance: vec![],
            });
        }
    }
}

/// NEXT-042：工作副本标题区的决策结果。此前 `emit_host_title_proposal` 把校验、
/// 落卡、静默返回揉在一起，且校验失败会整体静默（用户看不到改名为何没生效）。
enum HostTitleDecision {
    /// 标题区未编辑（或 trim 后与当前标题一致）——无需提案。
    Unchanged,
    /// 标题区有编辑但不满足改名前置条件——原因必须可见，不得静默吞掉。
    Invalid { reason: String },
    /// 合法且有效的标题提案（已 trim，通过新鲜度与项目内同名校验）。
    Proposed(String),
}

fn resolve_host_title_decision(
    context: &HostPatchContext,
    document: &HermesFocusDocument,
    staged: &str,
) -> HostTitleDecision {
    let proposed = match editable_title(staged) {
        Ok(value) => value.trim(),
        Err(error) => {
            return HostTitleDecision::Invalid { reason: error };
        }
    };
    if proposed == document.title {
        return HostTitleDecision::Unchanged;
    }
    if proposed.is_empty()
        || proposed.contains('\n')
        || proposed.contains('\r')
        || proposed.chars().count() > 200
    {
        return HostTitleDecision::Invalid {
            reason: "标题必须为 1–200 个字符且不能包含换行".to_string(),
        };
    }
    let conn = match rusqlite::Connection::open(&context.binding.db_path) {
        Ok(conn) => conn,
        Err(error) => {
            return HostTitleDecision::Invalid {
                reason: format!("打开文档数据库失败：{error}"),
            };
        }
    };
    let current_title: Result<String, _> = conn.query_row(
        "SELECT title FROM articles WHERE id = ?1",
        rusqlite::params![document.article_id],
        |row| row.get(0),
    );
    if !matches!(current_title.as_deref(), Ok(title) if title == document.title) {
        return HostTitleDecision::Invalid {
            reason: "标题已在本轮期间变化，请刷新后重试".to_string(),
        };
    }
    if let Some(project_id) = context.binding.project_id.as_deref() {
        let duplicate: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_documents d JOIN articles a ON a.id = d.article_id WHERE d.project_id = ?1 AND d.article_id != ?2 AND a.title = ?3",
                rusqlite::params![project_id, document.article_id, proposed],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if duplicate > 0 {
            return HostTitleDecision::Invalid {
                reason: format!("项目内已存在同名文档《{proposed}》"),
            };
        }
    }
    HostTitleDecision::Proposed(proposed.to_string())
}

/// 独立 rename 审批卡（仅改名回合：正文无差异时，Chat 卡是唯一决策入口）。
/// 「应用」走 appStore.updateArticleTitle → db_rename_article 完整链路。
fn emit_host_rename_card(
    context: &HostPatchContext,
    document: &HermesFocusDocument,
    proposed: &str,
    emitter: &EventEmitter,
) {
    let call_id = format!("host-rename-{}", context.binding.run_id);
    let operation_id = format!("rename-{}", uuid::Uuid::new_v4());
    let structured = serde_json::json!({
        "operationId": operation_id,
        "documentId": document.article_id,
        "oldTitle": document.title,
        "newTitle": proposed,
        "wikilinkAffectedCount": 0,
        "status": "pending_approval",
    });
    let provenance = vec![ProvenanceRef::new("sophonote-document")
        .with_id(&document.article_id)
        .with_title(&document.title)];
    let artifact = UiArtifact::new(
        "rename",
        structured.clone(),
        format!("将把《{}》重命名为《{}》。", document.title, proposed),
        provenance.clone(),
    )
    .ok();
    let _ = emitter.emit(AgentEventPayload::ToolStarted {
        call_id: call_id.clone(),
        name: "rename_article".into(),
        arguments_json: serde_json::json!({
            "articleId": document.article_id,
            "newTitle": proposed,
            "source": "hermes-session-working-copy"
        })
        .to_string(),
    });
    let _ = emitter.emit(AgentEventPayload::ToolCompleted {
        call_id,
        name: "rename_article".into(),
        ok: artifact.is_some(),
        error: artifact
            .is_none()
            .then(|| "无法生成标题改名提案".to_string()),
        preresolved: false,
        structured,
        ui_artifact: artifact,
        truncated: false,
        provenance,
    });
}

fn emit_host_title_failure(emitter: &EventEmitter, run_id: &str, error: String) {
    let call_id = format!("host-rename-{run_id}");
    let _ = emitter.emit(AgentEventPayload::ToolStarted {
        call_id: call_id.clone(),
        name: "rename_article".into(),
        arguments_json: "{}".into(),
    });
    let _ = emitter.emit(AgentEventPayload::ToolCompleted {
        call_id,
        name: "rename_article".into(),
        ok: false,
        error: Some(error),
        preresolved: false,
        structured: Value::Null,
        ui_artifact: None,
        truncated: false,
        provenance: vec![],
    });
}

fn quote_context_ref(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        format!("\"{}\"", path.replace('"', "\\\""))
    } else {
        path.to_string()
    }
}

fn join_user_input(message: &str, references: &[String]) -> String {
    let mut parts = Vec::with_capacity(1 + references.len());
    if !message.trim().is_empty() {
        parts.push(message.trim().to_string());
    }
    parts.extend(
        references
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned(),
    );
    parts.join("\n\n")
}

async fn dispatch_native_skill_if_needed(
    gateway: &mut HermesGatewayConnection,
    session_id: &str,
    skill: Option<&str>,
    user_text: &str,
) -> Result<String, EngineError> {
    let Some(skill) = skill.filter(|value| !value.trim().is_empty()) else {
        return Ok(user_text.to_string());
    };
    let dispatched = gateway
        .call(
            "command.dispatch",
            json!({"session_id": session_id, "name": skill, "arg": user_text}),
        )
        .await?;
    match dispatched.get("type").and_then(Value::as_str) {
        Some("skill" | "send") => required_string(&dispatched, "message", "command.dispatch"),
        _ => Err(EngineError::Setup(format!(
            "Hermes Skill /{skill} 不可用或不是可发送 Skill"
        ))),
    }
}

enum NativeSlashResult {
    Output(String),
    Prompt(String),
}

async fn dispatch_native_slash(
    gateway: &mut HermesGatewayConnection,
    session_id: &str,
    command: &str,
) -> Result<NativeSlashResult, EngineError> {
    let command = command.trim().trim_start_matches('/');
    if command.is_empty() {
        return Err(EngineError::Setup("Hermes slash command 不能为空".into()));
    }
    let dispatched = match gateway
        .call(
            "slash.exec",
            json!({"session_id": session_id, "command": command}),
        )
        .await
    {
        Ok(value) => value,
        Err(slash_error) => {
            let mut parts = command.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("");
            let arg = parts.next().unwrap_or("").trim();
            gateway
                .call(
                    "command.dispatch",
                    json!({"session_id": session_id, "name": name, "arg": arg}),
                )
                .await
                .map_err(|_| slash_error)?
        }
    };
    parse_native_slash_result(&dispatched)
}

fn parse_native_slash_result(dispatched: &Value) -> Result<NativeSlashResult, EngineError> {
    match dispatched.get("type").and_then(Value::as_str) {
        Some("prefill") => {
            // `/undo` 返回 prefill：只把撤回说明呈现为可见结果，绝不能再 prompt.submit。
            let notice = dispatched
                .get("notice")
                .and_then(Value::as_str)
                .unwrap_or("已撤回上一轮 Hermes 对话");
            let message = dispatched
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            Ok(NativeSlashResult::Output(
                [notice, message]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ))
        }
        Some("skill" | "send") => {
            required_string(dispatched, "message", "slash.exec").map(NativeSlashResult::Prompt)
        }
        Some("exec" | "plugin") | None => Ok(NativeSlashResult::Output(
            dispatched
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or("(no output)")
                .to_string(),
        )),
        Some(kind) => Err(EngineError::Setup(format!(
            "Hermes slash.exec 返回了 SophoNote 尚不能呈现的结果类型：{kind}"
        ))),
    }
}

fn empty_report() -> SpikeRunReport {
    SpikeRunReport {
        outcome: String::new(),
        final_answer: String::new(),
        model_calls: 0,
        tool_executions: Vec::new(),
        invalid_resolutions: 0,
        usage: TokenUsage::default(),
        transcript: Vec::new(),
        error: None,
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[derive(Default)]
    struct RecordingTransport(std::sync::Mutex<Vec<crate::agent::events::AgentEvent>>);

    impl crate::agent::events::EventTransport for RecordingTransport {
        fn send(&self, event: crate::agent::events::AgentEvent) -> Result<(), String> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[test]
    fn gateway_delta_coalescer_preserves_text_and_boundaries() {
        let mut pending = PendingGatewayDelta::from_payload(AgentEventPayload::MessageDelta {
            text: "Hermes ".into(),
            index: Some(1),
        })
        .unwrap();
        pending
            .append(AgentEventPayload::MessageDelta {
                text: "流式回答。".into(),
                index: Some(2),
            })
            .unwrap();
        assert!(pending.ready());
        assert_eq!(
            pending.into_payload(),
            AgentEventPayload::MessageDelta {
                text: "Hermes 流式回答。".into(),
                index: Some(1),
            }
        );
    }

    #[test]
    fn gateway_delta_coalescer_never_merges_reasoning_into_answer() {
        let mut pending = PendingGatewayDelta::from_payload(AgentEventPayload::ReasoningDelta {
            text: "思考".into(),
        })
        .unwrap();
        let next = pending
            .append(AgentEventPayload::MessageDelta {
                text: "答案".into(),
                index: None,
            })
            .unwrap_err();
        assert!(matches!(next, AgentEventPayload::MessageDelta { .. }));
        assert_eq!(
            pending.into_payload(),
            AgentEventPayload::ReasoningDelta {
                text: "思考".into()
            }
        );
    }

    #[test]
    fn selection_attachment_preserves_selected_markdown_verbatim() {
        let selection = crate::agent::events::RunContext {
            article_id: "article-1".into(),
            title: "标题：\"原文\"".into(),
            base_version: 7,
            selected_markdown: "## 小节\n\n- [x] 保留 **原文**\n```rust\nlet x = 1;\n```".into(),
            selected_text_hash: "sha256:abc".into(),
            before_context: "before".into(),
            after_context: "after".into(),
        };

        let attached = selection_attachment_markdown(&selection);

        assert!(attached.contains("source: sophonote-selection"));
        assert!(attached.contains("title: \"标题：\\\"原文\\\"\""));
        assert_eq!(
            editable_markdown(&attached).unwrap(),
            selection.selected_markdown
        );
        assert!(attached.ends_with(EDITABLE_END));
        assert!(!attached.contains("before"));
        assert!(!attached.contains("after"));
    }

    #[test]
    fn focus_document_attachment_preserves_editor_draft_verbatim() {
        let document = HermesFocusDocument {
            article_id: "article-2".into(),
            title: "草稿：\"未保存\"".into(),
            base_version: 11,
            markdown: "# 当前草稿\n\n- [ ] 不要经 MCP 回读\n```md\n**原样**\n```".into(),
        };

        let attached = focus_document_attachment_markdown(&document, Some("project-1"));

        assert!(attached.contains("source: sophonote-document"));
        assert!(attached.contains("title: \"草稿：\\\"未保存\\\"\""));
        assert!(attached.contains("baseVersion: 11"));
        assert_eq!(editable_title(&attached).unwrap(), document.title);
        assert_eq!(editable_markdown(&attached).unwrap(), document.markdown);
        assert!(attached.contains(PROJECT_ACTIONS_START));
        assert!(project_document_actions(&attached).unwrap().is_empty());
        assert!(attached.ends_with(EDITABLE_END));
    }

    #[test]
    fn project_attachment_exposes_manifest_without_document_bodies() {
        let fixture = crate::documents::repository::tests::RepoFixture::setup("project-context");
        fixture.seed_article(
            "article-1",
            "DeepSeek Harness",
            "private body must not leak",
        );
        fixture
            .conn()
            .execute_batch(
                "INSERT INTO projects (id, name, description) VALUES ('project-1', 'Agent Harness', '研究 Agent 运行时');
                 INSERT INTO project_documents (project_id, article_id) VALUES ('project-1', 'article-1');",
            )
            .unwrap();
        let binding = HermesSessionBinding {
            db_path: fixture.db_path.clone(),
            notes_dir: fixture.notes.clone(),
            project_id: Some("project-1".into()),
            thread_id: "thread-1".into(),
            run_id: "run-1".into(),
        };

        let attached = project_attachment_markdown(&binding, "project-1").unwrap();

        assert!(attached.contains("source: sophonote-project"));
        assert!(attached.contains(PROJECT_MANIFEST_START));
        assert!(attached.contains("DeepSeek Harness"));
        assert!(!attached.contains("private body must not leak"));
        assert!(attached.contains(PROJECT_ACTIONS_START));
        assert!(project_document_actions(&attached).unwrap().is_empty());
    }

    #[test]
    fn focused_document_always_registers_safe_writeback_context() {
        let binding = HermesSessionBinding {
            db_path: std::path::PathBuf::from("sophonote.db"),
            notes_dir: std::path::PathBuf::from("notes"),
            project_id: None,
            thread_id: "thread-1".into(),
            run_id: "run-1".into(),
        };
        let document = HermesFocusDocument {
            article_id: "article-1".into(),
            title: "当前笔记".into(),
            base_version: 1,
            markdown: String::new(),
        };

        let context = host_patch_context_for_attachment(
            Some(&binding),
            std::path::PathBuf::from("sophonote-document.md"),
            WorkingCopyTarget::Document(document),
        )
        .expect("natural-language document edits must be recovered without an explicit skill");

        assert_eq!(context.binding.run_id, "run-1");
        assert!(matches!(context.target, WorkingCopyTarget::Document(_)));
    }

    #[test]
    fn natural_language_workcopy_change_becomes_a_reviewable_diff() {
        let fixture = crate::documents::repository::tests::RepoFixture::setup("host-writeback");
        fixture.seed_article("article-1", "未命名文档", "");
        fixture
            .conn()
            .execute(
                "INSERT INTO agent_runs (id, thread_id, status, created_at, updated_at) VALUES ('run-1', 'thread-1', 'running', 1, 1)",
                [],
            )
            .unwrap();
        let document = HermesFocusDocument {
            article_id: "article-1".into(),
            title: "未命名文档".into(),
            base_version: 1,
            markdown: String::new(),
        };
        let staged_path = fixture.dir.join("sophonote-document.md");
        std::fs::write(
            &staged_path,
            focus_document_attachment_markdown(&document, None).replace(
                &format!("{EDITABLE_START}{EDITABLE_END}"),
                &format!("{EDITABLE_START}\n# 调研文章\n\n正文内容\n{EDITABLE_END}"),
            ),
        )
        .unwrap();
        let context = host_patch_context_for_attachment(
            Some(&HermesSessionBinding {
                db_path: fixture.db_path.clone(),
                notes_dir: fixture.notes.clone(),
                project_id: None,
                thread_id: "thread-1".into(),
                run_id: "run-1".into(),
            }),
            staged_path,
            WorkingCopyTarget::Document(document),
        )
        .unwrap();
        let transport = Arc::new(RecordingTransport::default());
        let emitter = EventEmitter::new("thread-1", "run-1", transport.clone());

        emit_host_patch(&context, Some(&emitter));

        let events = transport.0.lock().unwrap();
        let has_diff = events.iter().any(|event| {
            matches!(
                &event.payload,
                AgentEventPayload::ToolCompleted {
                    name,
                    ok: true,
                    ui_artifact: Some(artifact),
                    ..
                } if name == "sophonote_document_patch" && artifact.kind == "diff"
            )
        });
        assert!(
            has_diff,
            "expected a diff artifact, got: {:?}",
            events
                .iter()
                .map(|event| &event.payload)
                .collect::<Vec<_>>()
        );
        drop(events);
        let conn = fixture.conn();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM document_operations WHERE document_id = 'article-1' AND status = 'proposed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
    }

    #[test]
    fn project_action_parser_supports_nested_documents() {
        let value = format!(
            "{PROJECT_ACTIONS_START}\n[{{\"type\":\"create_document\",\"client_id\":\"root\",\"title\":\"研究\"}},{{\"type\":\"create_document\",\"client_id\":\"child\",\"title\":\"资料\",\"parent_client_id\":\"root\"}}]\n{PROJECT_ACTIONS_END}"
        );
        assert_eq!(project_document_actions(&value).unwrap().len(), 2);
    }

    #[test]
    fn project_actions_create_a_nested_sophonote_document_tree() {
        let fixture = crate::documents::repository::tests::RepoFixture::setup("host-tree");
        let conn = fixture.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('project-1', '研究项目')",
            [],
        )
        .unwrap();
        drop(conn);
        let binding = HermesSessionBinding {
            db_path: fixture.db_path.clone(),
            notes_dir: fixture.notes.clone(),
            project_id: Some("project-1".into()),
            thread_id: "thread-1".into(),
            run_id: "run-tree-1".into(),
        };
        let actions = project_document_actions(&format!(
            "{PROJECT_ACTIONS_START}\n[{{\"type\":\"create_document\",\"client_id\":\"root\",\"title\":\"研究\"}},{{\"type\":\"create_document\",\"client_id\":\"child\",\"title\":\"资料\",\"parent_client_id\":\"root\"}}]\n{PROJECT_ACTIONS_END}"
        ))
        .unwrap();

        let result = apply_project_document_actions(&binding, "project-1", actions).unwrap();
        let created = result["created"].as_array().unwrap();
        assert_eq!(created.len(), 2);
        let root_id = created[0]["articleId"].as_str().unwrap();
        let child_id = created[1]["articleId"].as_str().unwrap();
        let conn = fixture.conn();
        let parent_id: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM project_documents WHERE article_id = ?1",
                rusqlite::params![child_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_id.as_deref(), Some(root_id));
    }

    #[test]
    fn image_attachment_response_requires_attached_true() {
        assert!(validate_image_attach_response(&json!({"attached": true}), "x.png").is_ok());
        let error = validate_image_attach_response(
            &json!({"attached": false, "message": "unsupported"}),
            "x.png",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }

    #[test]
    fn editable_markdown_rejects_lost_or_duplicated_boundaries() {
        assert!(editable_markdown("plain text").is_err());
        assert!(editable_markdown(&format!(
            "{EDITABLE_START}one{EDITABLE_START}two{EDITABLE_END}"
        ))
        .is_err());
    }

    #[test]
    fn native_slash_result_distinguishes_output_from_prompt_dispatch() {
        match parse_native_slash_result(&json!({"output": "Session saved"})).unwrap() {
            NativeSlashResult::Output(value) => assert_eq!(value, "Session saved"),
            NativeSlashResult::Prompt(_) => panic!("output command must not submit a prompt"),
        }
        match parse_native_slash_result(&json!({"type": "send", "message": "expanded prompt"}))
            .unwrap()
        {
            NativeSlashResult::Prompt(value) => assert_eq!(value, "expanded prompt"),
            NativeSlashResult::Output(_) => {
                panic!("send dispatch must continue through prompt.submit")
            }
        }
        match parse_native_slash_result(&json!({
            "type": "prefill",
            "message": "上一轮",
            "notice": "↶ Undid 1 turn"
        }))
        .unwrap()
        {
            NativeSlashResult::Output(value) => assert!(value.contains("Undid 1 turn")),
            NativeSlashResult::Prompt(_) => {
                panic!("/undo prefill must not be sent as a new prompt")
            }
        }
    }

    #[test]
    fn finds_completed_answer_after_matching_user_turn() {
        let payload = json!({
            "messages": [
                {"role":"user","text":"上一轮"},
                {"role":"assistant","text":"旧回答"},
                {"role":"user","text":"请生成网页\n\n@file:/tmp/spec.md"},
                {"role":"assistant","text":"网页已经完成并验证。"}
            ]
        });
        assert_eq!(
            final_answer_after_user(&payload, Some("请生成网页")),
            Some("网页已经完成并验证。".into())
        );
    }

    #[test]
    fn does_not_borrow_an_answer_from_an_older_turn() {
        let payload = json!({
            "messages": [
                {"role":"user","text":"上一轮"},
                {"role":"assistant","text":"旧回答"},
                {"role":"user","text":"尚未完成的任务"}
            ]
        });
        assert_eq!(
            final_answer_after_user(&payload, Some("尚未完成的任务")),
            None
        );
    }

    #[test]
    fn does_not_borrow_an_answer_from_a_later_turn() {
        let payload = json!({
            "messages": [
                {"role": "user", "text": "尚未完成的任务"},
                {"role": "user", "text": "另一个客户端的新问题"},
                {"role": "assistant", "text": "这是后来一轮的答案"}
            ]
        });
        assert_eq!(
            final_answer_after_user(&payload, Some("尚未完成的任务")),
            None
        );
    }
}
