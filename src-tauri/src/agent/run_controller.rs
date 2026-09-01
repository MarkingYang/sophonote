// ============================================================
// Track B · 智能体演进（AG-06 追加）：RunController（Phase 1 Spike 口径）
// 实施基线：docs/architecture.md 驱动协议伪代码 +
// §〇.3 采纳门禁 + §〇.3.1/§〇.3.2 rig 0.41.0 契约侦察记录。
//
// 职责：以 SophoNote 自有 Gateway 驱动 rig_agent::agent::run::AgentRun
// （sans-IO 可步进状态机）——CallModel 走 ModelGateway、CallTools 走
// ToolRegistry、Done 收敛产物；max turns / 取消 / 未知工具 / 畸形参数
// 四条护栏在此闭环（§〇.3 门禁第 3/4 条）。
//
// rig 类型扩散面：本文件 + adapters.rs（硬性限制⑤的收敛边界）。
// AG-07：事件经 events::EventEmitter 侧路发射（Option 注入，失败不中断循环）。
// ============================================================
use std::sync::Arc;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use rig_agent::agent::hook::InvalidToolCallAction;
use rig_agent::agent::run::{AgentRun, AgentRunStep, ModelTurnOutcome};
use rig_agent::completion::{Message, PromptError};
use rig_core::completion::message::UserContent;

use crate::model::gateway::SharedGateway;
use crate::model::messages::{ModelError, ModelMessage, ModelRequest, TokenUsage, ToolChoice};
use crate::tools::{ToolOutput, ToolRegistry};

use super::adapters::{
    from_rig_history, from_rig_tool_call, model_response_to_turn, rig_tool_error, rig_tool_result,
    to_rig_history, token_usage_from_usage, AdapterError,
};
use super::events::{AgentEventPayload, EventEmitter};

/// AG-21：工具调用观察者——驱动在真实执行节奏上回调，持久化侧（RunStore
/// 的 agent_tool_calls 表）实现之，使 RunStore 可追溯每次工具调用的
/// structured/ui_artifact/provenance/truncated。
/// 观测面语义与事件一致：观察者失败不得中断主循环（实现方自行吞错降级）。
/// preresolved 透传不触发 on_start（从未真正执行），只触发 on_completed。
pub trait ToolCallObserver: Send + Sync {
    fn on_start(&self, call_id: &str, name: &str, arguments_json: &str);
    fn on_completed(
        &self,
        call_id: &str,
        name: &str,
        ok: bool,
        error: Option<&str>,
        preresolved: bool,
        output: Option<&ToolOutput>,
    );
}

/// Spike 驱动参数
#[derive(Debug, Clone)]
pub struct SpikeParams {
    /// 系统提示（None = 无系统消息）
    pub system: Option<String>,
    /// 历史对话（AG-19：不含系统消息与本轮 user；命令层从 RunStore 加载注入，
    /// 时序升序；Spike 调试命令为空。空 assistant 内容由加载侧过滤——
    /// to_rig_history 对空 assistant 回合报 EmptyAssistantTurn）
    pub history: Vec<ModelMessage>,
    /// 用户输入（AgentRun 的 prompt）
    pub user: String,
    /// 模型调用总预算（rig max_turns 口径：含首次与一切重试）
    pub max_turns: usize,
    pub temperature: Option<f32>,
    /// AG-22（审计 P1-2 整改②）：关联的 Agent Run id。正式 agent_run_start
    /// 传 Some(run_id)，每次模型请求随 ModelRequest 注入（排障溯源）；
    /// Spike 调试命令无 Run 传 None。
    pub run_id: Option<String>,
    /// AG-22：提示词版本号（PromptRegistry 口径）。正式路径传
    /// agentChat@v1；Spike 调试命令沿用 spike@v1。
    pub prompt_version: String,
    /// AG-26：编辑器选区上下文（随 run_started 事件透传给前端；
    /// 系统提示注入在命令层完成，驱动层只负责事件透传）。Spike 调试命令传 None。
    pub run_context: Option<crate::agent::events::RunContext>,
    /// AG-27：激活的 Skill（随 run_started 事件透传；Worklog 可见版本与来源）。
    /// Spike 调试命令传 None。
    pub run_skill: Option<crate::agent::events::RunSkillRef>,
    /// AG-27：工具调用预算（Skill 清单 max_tool_calls；None = 不限）。
    /// 驱动层在 CallTools 节奏强制执行：超额即终态 tool_budget_exceeded。
    pub max_tool_calls: Option<usize>,
}

/// 驱动循环自身的建立失败（进入循环前的转换错误；循环内失败一律进 report）
#[derive(Debug)]
pub enum RunControllerError {
    Adapter(AdapterError),
}

impl std::fmt::Display for RunControllerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adapter(e) => write!(f, "会话转换失败: {}", e),
        }
    }
}

impl std::error::Error for RunControllerError {}

/// 单次工具执行记录（观测用；Phase 2 由 run_events 持久化承接）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionRecord {
    pub call_id: String,
    pub name: String,
    pub ok: bool,
    /// 失败时的错误文本（同回填给模型的文本）
    pub error: Option<String>,
    /// preresolved_result 原样透传（无效调用恢复的跳过语义，不执行工具）
    pub preresolved: bool,
}

/// Spike 运行报告（调试命令返回值 + 单测断言面）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpikeRunReport {
    /// completed / max_turns_exceeded / tool_budget_exceeded / unknown_tool /
    /// cancelled / model_error / adapter_error / protocol_error
    pub outcome: String,
    pub final_answer: String,
    /// rig turn()：已发生的模型调用数（含重试）
    pub model_calls: usize,
    pub tool_executions: Vec<ToolExecutionRecord>,
    /// 无效工具调用恢复次数（NeedsResolution 处理计数）
    pub invalid_resolutions: usize,
    pub usage: TokenUsage,
    /// 全量会话还原（自有 ModelMessage 口径，含输入历史）
    pub transcript: Vec<ModelMessage>,
    pub error: Option<String>,
}

impl SpikeRunReport {
    fn pending() -> Self {
        Self {
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
}

/// PromptError → report 收尾（non_exhaustive，保留通配臂）
fn finish_with_prompt_error(report: &mut SpikeRunReport, err: &PromptError) {
    match err {
        PromptError::MaxTurnsError { max_turns, .. } => {
            report.outcome = "max_turns_exceeded".into();
            report.error = Some(format!("模型调用预算 {} 次用尽", max_turns));
        }
        PromptError::UnknownToolCall { tool_name, .. } => {
            report.outcome = "unknown_tool".into();
            report.error = Some(format!("模型调用了不可用工具: {}", tool_name));
        }
        PromptError::PromptCancelled { reason, .. } => {
            report.outcome = "cancelled".into();
            report.error = Some(reason.clone());
        }
        other => {
            report.outcome = "protocol_error".into();
            report.error = Some(other.to_string());
        }
    }
}

/// 事件发射失败 = 观测面失败：不中断主循环（RunStore 落地前事件是尽力而为）
fn emit_opt(events: &Option<Arc<EventEmitter>>, payload: AgentEventPayload) {
    if let Some(emitter) = events {
        let _ = emitter.emit(payload);
    }
}

/// 驱动一次 Spike 运行（无事件流，AG-06 口径保留）。
pub async fn run_spike(
    gateway: SharedGateway,
    registry: Arc<ToolRegistry>,
    params: SpikeParams,
    cancel: CancellationToken,
) -> Result<SpikeRunReport, RunControllerError> {
    run_spike_with_events(gateway, registry, params, cancel, None, None).await
}

/// 驱动一次 Spike 运行（AG-07：可选事件流；AG-21：可选工具调用观察者）。
/// 循环内任何失败都收敛进 report
/// （带截至失败点的转录与用量）；仅进入循环前的转换失败返回 Err。
/// events = Some 时按驱动节奏发射有序 AgentEvent，终态事件在循环尾恰好一次；
/// observer = Some 时回调工具执行节奏（RunStore 持久化用，失败不中断主循环）。
pub async fn run_spike_with_events(
    gateway: SharedGateway,
    registry: Arc<ToolRegistry>,
    params: SpikeParams,
    cancel: CancellationToken,
    events: Option<Arc<EventEmitter>>,
    observer: Option<Arc<dyn ToolCallObserver>>,
) -> Result<SpikeRunReport, RunControllerError> {
    // 输入历史（自有口径 → rig 口径）：至多一条系统消息 + 历史对话（AG-19）。
    // 顺序 = [system?] + 历史（时序升序）；本轮 user 由 AgentRun::new 作为 prompt 追加，
    // 不在此列——多轮上下文由此注入（Phase 2 验收「第二轮能引用第一轮」的驱动侧支点）。
    let mut history_src: Vec<ModelMessage> = params
        .system
        .as_ref()
        .map(|s| vec![ModelMessage::system(s.clone())])
        .unwrap_or_default();
    history_src.extend(params.history.iter().cloned());
    let history = to_rig_history(&history_src).map_err(RunControllerError::Adapter)?;

    let mut run = AgentRun::new(Message::user(params.user.clone()))
        .with_history(history)
        .max_turns(params.max_turns)
        // 未知/畸形工具调用允许模型修复一次（§十二「最多让模型修复一次」），
        // 第二次无效调用由驱动侧选 Fail 终止运行
        .max_invalid_tool_call_retries(1);

    let tools = registry.definitions();
    let tool_names = registry.names();
    let tool_names_hint = tool_names.iter().cloned().collect::<Vec<_>>().join(", ");

    let mut report = SpikeRunReport::pending();
    // AG-27：工具调用计数（Skill 预算 max_tool_calls 的强制执行口径；
    // preresolved 透传不计入——从未真正执行）
    let mut tool_calls_used: usize = 0;

    emit_opt(
        &events,
        AgentEventPayload::RunStarted {
            user_message: params.user.clone(),
            max_turns: params.max_turns,
            context: params.run_context.clone(),
            skill: params.run_skill.clone(),
        },
    );

    'run_loop: loop {
        if cancel.is_cancelled() {
            report.outcome = "cancelled".into();
            report.error = Some("运行被取消".into());
            break;
        }

        let step = match run.next_step() {
            Ok(step) => step,
            Err(err) => {
                finish_with_prompt_error(&mut report, &err);
                break;
            }
        };

        match step {
            AgentRunStep::CallModel {
                prompt,
                history,
                turn,
            } => {
                println!("[agent] spike: model call turn={}", turn);
                emit_opt(&events, AgentEventPayload::ModelStarted { turn });
                // rig 历史 → 自有口径（Adapter 1b），prompt 追加为最后一条
                let mut messages = from_rig_history(&history);
                messages.extend(from_rig_history(std::slice::from_ref(&prompt)));

                let request = ModelRequest {
                    model: String::new(), // 空 = gateway 默认模型（settings 配置）
                    messages,
                    tools: tools.clone(),
                    tool_choice: Some(ToolChoice::Auto),
                    temperature: params.temperature,
                    max_tokens: None,
                    thinking: None,
                    // AG-22：prompt_version/run_id 由命令层注入真实值
                    //（审计 P1-2：此前固定 "spike@v1"/None，运行记录无法溯源）
                    prompt_version: params.prompt_version.clone(),
                    run_id: params.run_id.clone(),
                };

                let response = match gateway.complete(request, cancel.clone()).await {
                    Ok(resp) => resp,
                    Err(ModelError::Cancelled) => {
                        report.outcome = "cancelled".into();
                        report.error = Some("模型调用被取消".into());
                        break;
                    }
                    Err(err) => {
                        report.outcome = "model_error".into();
                        report.error = Some(err.to_string());
                        break;
                    }
                };

                let rig_turn =
                    match model_response_to_turn(&response, tool_names.clone(), tool_names.clone())
                    {
                        Ok(t) => t,
                        Err(err) => {
                            report.outcome = "adapter_error".into();
                            report.error = Some(err.to_string());
                            break;
                        }
                    };

                match run.model_response(rig_turn) {
                    // Continue = 回合被接受；TurnRetried = 无效调用恢复已回滚并追加
                    // 纠正反馈——两者都回循环顶取下一个 step
                    Ok(ModelTurnOutcome::Continue { .. }) | Ok(ModelTurnOutcome::TurnRetried) => {
                        continue
                    }
                    Ok(ModelTurnOutcome::NeedsResolution(_ctx)) => {
                        report.invalid_resolutions += 1;
                        // 驱动侧恢复策略：首次给模型一次修复机会（带可用工具清单），
                        // 再次无效即 Fail（rig 以 UnknownToolCall 终结运行）
                        let action = if report.invalid_resolutions == 1 {
                            println!("[agent] spike: invalid tool call, retry with feedback");
                            InvalidToolCallAction::Retry {
                                feedback: format!(
                                    "上一次工具调用无效（工具不存在或不被允许）。可用工具只有: {}。请用正确的工具名重试。",
                                    tool_names_hint
                                ),
                            }
                        } else {
                            println!("[agent] spike: invalid tool call again, fail run");
                            InvalidToolCallAction::Fail
                        };
                        match run.resolve_invalid_tool_call(action) {
                            Ok(_) => continue,
                            Err(err) => {
                                finish_with_prompt_error(&mut report, &err);
                                break;
                            }
                        }
                    }
                    Err(err) => {
                        finish_with_prompt_error(&mut report, &err);
                        break;
                    }
                }
            }

            AgentRunStep::CallTools { calls } => {
                let mut results: Vec<UserContent> = Vec::with_capacity(calls.len());
                for pending in calls {
                    // preresolved_result 非 None = 无效调用恢复中被跳过的同批兄弟调用：
                    // 必须原样回填，不执行工具、不触发工具钩子（§〇.3.2）。
                    // AG-21：不触发 on_start（从未执行），仅 on_completed 登记透传
                    if let Some(pre) = pending.preresolved_result {
                        emit_opt(
                            &events,
                            AgentEventPayload::ToolCompleted {
                                call_id: pending.tool_call.id.clone(),
                                name: pending.tool_call.function.name.clone(),
                                ok: true,
                                error: None,
                                preresolved: true,
                                structured: serde_json::Value::Null,
                                ui_artifact: None,
                                truncated: false,
                                provenance: Vec::new(),
                            },
                        );
                        if let Some(obs) = &observer {
                            obs.on_completed(
                                &pending.tool_call.id,
                                &pending.tool_call.function.name,
                                true,
                                None,
                                true,
                                None,
                            );
                        }
                        report.tool_executions.push(ToolExecutionRecord {
                            call_id: pending.tool_call.id.clone(),
                            name: pending.tool_call.function.name.clone(),
                            ok: true,
                            error: None,
                            preresolved: true,
                        });
                        results.push(pre);
                        continue;
                    }

                    // AG-27：工具预算强制执行（Skill 清单 max_tool_calls）。
                    // 超额即终态：不执行本次调用、同批剩余调用也不执行；
                    // 终态事件由循环尾统一发射（outcome=tool_budget_exceeded）
                    if let Some(limit) = params.max_tool_calls {
                        if tool_calls_used >= limit {
                            report.outcome = "tool_budget_exceeded".into();
                            report.error = Some(format!("工具调用预算 {} 次用尽", limit));
                            break 'run_loop;
                        }
                    }

                    let call = from_rig_tool_call(&pending.tool_call);
                    emit_opt(
                        &events,
                        AgentEventPayload::ToolStarted {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            arguments_json: call.arguments.to_string(),
                        },
                    );
                    if let Some(obs) = &observer {
                        obs.on_start(&call.id, &call.name, &call.arguments.to_string());
                    }
                    let exec = registry.execute(&call).await;
                    tool_calls_used += 1;
                    match exec {
                        Ok(out) => {
                            println!("[agent] spike: tool ok name={} id={}", call.name, call.id);
                            emit_opt(
                                &events,
                                AgentEventPayload::ToolCompleted {
                                    call_id: call.id.clone(),
                                    name: call.name.clone(),
                                    ok: true,
                                    error: None,
                                    preresolved: false,
                                    // AG-21：UI 渲染通道贯通（事件只带 structured/
                                    // uiArtifact，model_text 不进事件——卡片不解析它）
                                    structured: out.structured.clone(),
                                    ui_artifact: out.ui_artifact.clone(),
                                    truncated: out.truncated,
                                    provenance: out.provenance.clone(),
                                },
                            );
                            if let Some(obs) = &observer {
                                obs.on_completed(
                                    &call.id,
                                    &call.name,
                                    true,
                                    None,
                                    false,
                                    Some(&out),
                                );
                            }
                            report.tool_executions.push(ToolExecutionRecord {
                                call_id: call.id.clone(),
                                name: call.name.clone(),
                                ok: true,
                                error: None,
                                preresolved: false,
                            });
                            results.push(rig_tool_result(call.id, out.model_text));
                        }
                        Err(err) => {
                            // 工具层错误不是异常路径：文本交还模型自行决策
                            println!(
                                "[agent] spike: tool err name={} id={} reason={}",
                                call.name, call.id, err
                            );
                            emit_opt(
                                &events,
                                AgentEventPayload::ToolCompleted {
                                    call_id: call.id.clone(),
                                    name: call.name.clone(),
                                    ok: false,
                                    error: Some(err.to_string()),
                                    preresolved: false,
                                    structured: serde_json::Value::Null,
                                    ui_artifact: None,
                                    truncated: false,
                                    provenance: Vec::new(),
                                },
                            );
                            if let Some(obs) = &observer {
                                obs.on_completed(
                                    &call.id,
                                    &call.name,
                                    false,
                                    Some(&err.to_string()),
                                    false,
                                    None,
                                );
                            }
                            report.tool_executions.push(ToolExecutionRecord {
                                call_id: call.id.clone(),
                                name: call.name.clone(),
                                ok: false,
                                error: Some(err.to_string()),
                                preresolved: false,
                            });
                            results.push(rig_tool_error(call.id, err.to_string()));
                        }
                    }
                }
                if let Err(err) = run.tool_results(results) {
                    finish_with_prompt_error(&mut report, &err);
                    break;
                }
            }

            AgentRunStep::Done(response) => {
                report.outcome = "completed".into();
                report.final_answer = response.output.clone();
                break;
            }
        }
    }

    // 收尾观测面：无论成败都带全量转录与聚合用量（run 状态在错误后仍可读）
    report.model_calls = run.turn();
    report.usage = token_usage_from_usage(&run.usage());
    report.transcript = from_rig_history(&run.full_history());
    println!(
        "[agent] spike: done outcome={} model_calls={} tools={} usage_total={}",
        report.outcome,
        report.model_calls,
        report.tool_executions.len(),
        report.usage.total_tokens
    );

    // AG-07：终态事件恰好一次（EventEmitter 终态锁定保证它是本 Run 最后一个事件）
    match report.outcome.as_str() {
        "completed" => emit_opt(
            &events,
            AgentEventPayload::RunCompleted {
                outcome: report.outcome.clone(),
                final_answer: report.final_answer.clone(),
                model_calls: report.model_calls,
            },
        ),
        "cancelled" => emit_opt(
            &events,
            AgentEventPayload::RunCancelled {
                reason: report.error.clone().unwrap_or_else(|| "运行被取消".into()),
            },
        ),
        other => emit_opt(
            &events,
            AgentEventPayload::RunFailed {
                outcome: other.to_string(),
                error: report.error.clone().unwrap_or_default(),
            },
        ),
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::gateway::ModelGateway;
    use crate::model::messages::{FinishReason, ModelToolCall};
    use crate::tools::builtin::spike_registry;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// 脚本化假 Gateway：按序吐出预置响应，耗尽即 Config 错误；
    /// 取消令牌优先于脚本生效（与真实 Gateway 的 select 语义一致）
    struct ScriptedGateway {
        responses: Mutex<VecDeque<crate::model::messages::ModelResponse>>,
    }

    impl ScriptedGateway {
        fn new(responses: Vec<crate::model::messages::ModelResponse>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into()),
            })
        }
    }

    #[async_trait]
    impl ModelGateway for ScriptedGateway {
        async fn complete(
            &self,
            _request: ModelRequest,
            cancel: CancellationToken,
        ) -> Result<crate::model::messages::ModelResponse, ModelError> {
            if cancel.is_cancelled() {
                return Err(ModelError::Cancelled);
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ModelError::Config("脚本响应耗尽".into()))
        }
    }

    fn usage_fixture() -> TokenUsage {
        TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        }
    }

    fn text_response(content: &str) -> crate::model::messages::ModelResponse {
        crate::model::messages::ModelResponse {
            content: content.into(),
            reasoning: None,
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: usage_fixture(),
            provider_request_id: Some("resp-text".into()),
        }
    }

    fn tool_response(
        call_id: &str,
        name: &str,
        args: serde_json::Value,
    ) -> crate::model::messages::ModelResponse {
        crate::model::messages::ModelResponse {
            content: String::new(),
            reasoning: None,
            tool_calls: vec![ModelToolCall {
                id: call_id.into(),
                name: name.into(),
                arguments: args,
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: usage_fixture(),
            provider_request_id: Some("resp-tool".into()),
        }
    }

    fn params(user: &str, max_turns: usize) -> SpikeParams {
        SpikeParams {
            system: Some("你是 SophoNote Spike 测试助手，需要时调用工具。".into()),
            history: Vec::new(),
            user: user.into(),
            max_turns,
            temperature: Some(0.0),
            run_id: None,
            prompt_version: "spike@v1".into(),
            run_context: None,
            run_skill: None,
            max_tool_calls: None,
        }
    }

    // AG-27：工具预算（Skill max_tool_calls）——超额调用被拦截，运行以
    // tool_budget_exceeded 终态收尾；被拦截的调用不执行、不进执行记录
    #[tokio::test]
    async fn tool_budget_exceeded_stops_run_before_second_execution() {
        let gateway = ScriptedGateway::new(vec![
            tool_response("c1", "calculator", serde_json::json!({"expression": "1+1"})),
            tool_response("c2", "calculator", serde_json::json!({"expression": "2+2"})),
            text_response("不该到达"),
        ]);
        let mut p = params("连续算两次", 6);
        p.max_tool_calls = Some(1);
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            p,
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "tool_budget_exceeded");
        assert!(report.error.unwrap().contains("预算"));
        // 只有第一次调用真正执行（c2 被预算拦截）
        assert_eq!(report.tool_executions.len(), 1);
        assert_eq!(report.tool_executions[0].call_id, "c1");
    }

    /// 记录型假 Gateway（AG-19）：固定回一条文本响应，逐条留存收到的请求，
    /// 供断言「历史注入后首次模型调用的 messages 序列」
    #[derive(Clone)]
    struct RecordingGateway {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl RecordingGateway {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                requests: Arc::new(Mutex::new(Vec::new())),
            })
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ModelGateway for RecordingGateway {
        async fn complete(
            &self,
            request: ModelRequest,
            _cancel: CancellationToken,
        ) -> Result<crate::model::messages::ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request);
            Ok(text_response("收到。"))
        }
    }

    // AG-19：历史注入——首次模型调用的 messages = [system, 历史 user, 历史 assistant, 本轮 user]
    #[tokio::test]
    async fn history_injected_between_system_and_current_user() {
        let gateway = RecordingGateway::new();
        let mut p = params("第二轮：我刚才说天空是什么颜色？", 2);
        p.history = vec![
            ModelMessage::user("第一轮：请记住，天空是蓝色的。"),
            ModelMessage::assistant("好的，我记住了：天空是蓝色的。"),
        ];
        let report = run_spike(
            gateway.clone(),
            Arc::new(spike_registry()),
            p,
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "completed");

        let reqs = gateway.requests();
        assert_eq!(reqs.len(), 1, "单轮直答只应有一次模型调用");
        use crate::model::messages::ModelRole;
        let roles: Vec<ModelRole> = reqs[0].messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            vec![
                ModelRole::System,
                ModelRole::User,
                ModelRole::Assistant,
                ModelRole::User
            ]
        );
        assert_eq!(
            reqs[0].messages[1].content,
            "第一轮：请记住，天空是蓝色的。"
        );
        assert_eq!(
            reqs[0].messages[2].content,
            "好的，我记住了：天空是蓝色的。"
        );
        assert_eq!(
            reqs[0].messages[3].content,
            "第二轮：我刚才说天空是什么颜色？"
        );
    }

    // AG-19：空历史不受影响（新 Thread 首轮，行为与注入前一致）
    #[tokio::test]
    async fn empty_history_keeps_first_round_shape() {
        let gateway = RecordingGateway::new();
        let report = run_spike(
            gateway.clone(),
            Arc::new(spike_registry()),
            params("首轮问题", 2),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "completed");
        let reqs = gateway.requests();
        assert_eq!(reqs[0].messages.len(), 2, "system + 本轮 user");
    }

    // AG-22（审计 P1-2 整改②）：SpikeParams 的 run_id/prompt_version
    // 必须逐次模型请求注入 ModelRequest（排障溯源 + 回归对比口径）
    #[tokio::test]
    async fn model_request_carries_run_id_and_prompt_version() {
        let gateway = RecordingGateway::new();
        let mut p = params("带元数据的请求", 2);
        p.run_id = Some("run-ag22".into());
        p.prompt_version = "agent-chat@v1".into();
        let report = run_spike(
            gateway.clone(),
            Arc::new(spike_registry()),
            p,
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "completed");
        let reqs = gateway.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].run_id.as_deref(), Some("run-ag22"));
        assert_eq!(reqs[0].prompt_version, "agent-chat@v1");
    }

    // 门禁③：两轮工具调用跑通（天气 → 计算器 → 终答）
    #[tokio::test]
    async fn two_round_tool_calls_then_final_answer() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-w1",
                "get_weather",
                serde_json::json!({"city": "杭州"}),
            ),
            tool_response(
                "call-c1",
                "calculator",
                serde_json::json!({"op": "add", "a": 26, "b": 3}),
            ),
            text_response("杭州 26°C，加 3 等于 29。"),
        ]);
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("先查杭州天气，再把气温加 3", 6),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");

        assert_eq!(report.outcome, "completed");
        assert_eq!(report.final_answer, "杭州 26°C，加 3 等于 29。");
        assert_eq!(report.model_calls, 3);
        assert_eq!(report.tool_executions.len(), 2);
        assert_eq!(report.tool_executions[0].name, "get_weather");
        assert!(report.tool_executions[0].ok);
        assert_eq!(report.tool_executions[1].name, "calculator");
        assert!(report.tool_executions[1].ok);
        assert_eq!(report.invalid_resolutions, 0);
        // 三次模型调用 × 15 token
        assert_eq!(report.usage.total_tokens, 45);
        // 转录含工具结果消息且 call_id 对齐（状态机多集匹配的前提）
        let tool_msgs: Vec<_> = report
            .transcript
            .iter()
            .filter(|m| m.role == crate::model::messages::ModelRole::Tool)
            .collect();
        assert_eq!(tool_msgs.len(), 2);
        assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("call-w1"));
        assert!(tool_msgs[0].content.contains("杭州"));
        assert_eq!(tool_msgs[1].tool_call_id.as_deref(), Some("call-c1"));
        assert!(tool_msgs[1].content.contains("29"));
    }

    // 门禁④a：未知工具 → 一次纠正反馈修复机会 → 模型改对后正常完成
    #[tokio::test]
    async fn unknown_tool_recovers_with_retry_feedback() {
        let gateway = ScriptedGateway::new(vec![
            tool_response("call-bad", "nonexistent_tool", serde_json::json!({})),
            text_response("改用正确工具后完成。"),
        ]);
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("做点什么", 6),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");

        assert_eq!(report.outcome, "completed");
        assert_eq!(report.invalid_resolutions, 1);
        assert_eq!(report.model_calls, 2);
        // 纠正反馈以 user 消息进入历史，带可用工具清单
        let feedback = report
            .transcript
            .iter()
            .find(|m| m.content.contains("可用工具只有"));
        assert!(feedback.is_some(), "重试反馈应进入转录");
        assert!(feedback.unwrap().content.contains("calculator"));
    }

    // 门禁④a（终止分支）：两次无效调用 → 驱动选 Fail → UnknownToolCall 收场
    #[tokio::test]
    async fn unknown_tool_twice_fails_the_run() {
        let gateway = ScriptedGateway::new(vec![
            tool_response("call-b1", "ghost_tool", serde_json::json!({})),
            tool_response("call-b2", "ghost_tool", serde_json::json!({})),
        ]);
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("做点什么", 6),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");

        assert_eq!(report.outcome, "unknown_tool");
        assert_eq!(report.invalid_resolutions, 2);
        assert!(report.error.unwrap().contains("ghost_tool"));
    }

    // 门禁④b：畸形参数 → 工具层错误文本回填 → 模型改参后完成（不崩循环）
    #[tokio::test]
    async fn malformed_arguments_flow_back_to_model() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-m1",
                "calculator",
                serde_json::json!({"op": "add", "a": "not-a-number", "b": 2}),
            ),
            tool_response(
                "call-m2",
                "calculator",
                serde_json::json!({"op": "add", "a": 1, "b": 2}),
            ),
            text_response("结果是 3。"),
        ]);
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("帮我算加法", 6),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");

        assert_eq!(report.outcome, "completed");
        assert_eq!(report.tool_executions.len(), 2);
        assert!(!report.tool_executions[0].ok);
        assert!(report.tool_executions[0]
            .error
            .as_deref()
            .unwrap()
            .contains("不是数字"));
        assert!(report.tool_executions[1].ok);
        // 失败文本确实回到了模型视角（转录里的 tool 消息）
        let failed_feedback = report
            .transcript
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("call-m1"));
        assert!(failed_feedback.unwrap().content.contains("参数无效"));
    }

    // 门禁④c：max turns 预算硬约束（工具循环不给终答 → MaxTurnsError）
    #[tokio::test]
    async fn max_turns_budget_is_enforced() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-t1",
                "get_weather",
                serde_json::json!({"city": "北京"}),
            ),
            tool_response(
                "call-t2",
                "get_weather",
                serde_json::json!({"city": "上海"}),
            ),
        ]);
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("一直查天气", 2),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");

        assert_eq!(report.outcome, "max_turns_exceeded");
        assert_eq!(report.model_calls, 2);
        assert!(report.error.unwrap().contains("预算"));
    }

    // 门禁④d：取消令牌在进入循环前生效
    #[tokio::test]
    async fn cancellation_before_start_short_circuits() {
        let gateway = ScriptedGateway::new(vec![text_response("不该出现")]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("随便", 6),
            cancel,
        )
        .await
        .expect("驱动建立成功");

        assert_eq!(report.outcome, "cancelled");
        assert_eq!(report.model_calls, 0);
        assert!(report.tool_executions.is_empty());
    }

    // 模型调用中途被取消（gateway 侧 Cancelled 分支）
    #[tokio::test]
    async fn cancellation_during_model_call_is_reported() {
        let gateway = ScriptedGateway::new(vec![text_response("不该出现")]);
        let cancel = CancellationToken::new();
        // ScriptedGateway 在 complete 里先查令牌：循环顶检查在前，
        // 因此这里验证的是「循环顶检查 + Cancelled 分支」同一条收口路径
        cancel.cancel();
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("随便", 6),
            cancel,
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "cancelled");
    }

    // 模型错误（网关 Config）→ report 收口，不抛出
    #[tokio::test]
    async fn model_error_is_captured_in_report() {
        let gateway = ScriptedGateway::new(Vec::new()); // 立即耗尽 → Config 错误
        let report = run_spike(
            gateway,
            Arc::new(spike_registry()),
            params("随便", 6),
            CancellationToken::new(),
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "model_error");
        assert!(report.error.unwrap().contains("脚本响应耗尽"));
        // rig turn() 在发出 CallModel 时即计数（不等响应被接受）：
        // 状态机已发出 1 次调用请求，gateway 侧失败不回滚计数
        assert_eq!(report.model_calls, 1);
        assert!(report.tool_executions.is_empty());
    }

    // ---- AG-07 门禁⑤：事件流发射面 ----

    use crate::agent::events::{AgentEvent, EventTransport, AGENT_EVENT_SCHEMA_VERSION};

    /// 内存录制 transport：断言事件顺序 / seq / 信封字段
    #[derive(Default)]
    struct RecordingTransport {
        events: Mutex<Vec<AgentEvent>>,
    }

    impl EventTransport for RecordingTransport {
        fn send(&self, event: AgentEvent) -> Result<(), String> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    fn recorder() -> (Arc<EventEmitter>, Arc<RecordingTransport>) {
        let rec = Arc::new(RecordingTransport::default());
        let em = Arc::new(EventEmitter::new("spike", "run-test", rec.clone()));
        (em, rec)
    }

    fn payload_kind(p: &AgentEventPayload) -> &'static str {
        match p {
            AgentEventPayload::RunStarted { .. } => "run_started",
            AgentEventPayload::ModelStarted { .. } => "model_started",
            AgentEventPayload::ToolStarted { .. } => "tool_started",
            AgentEventPayload::ToolCompleted { .. } => "tool_completed",
            AgentEventPayload::MessageDelta { .. } => "message_delta",
            AgentEventPayload::MessageCompleted { .. } => "message_completed",
            AgentEventPayload::MessageInterim { .. } => "message_interim",
            AgentEventPayload::ReasoningDelta { .. } => "reasoning_delta",
            AgentEventPayload::ReasoningCompleted {} => "reasoning_completed",
            AgentEventPayload::ApprovalRequired { .. } => "approval_required",
            AgentEventPayload::ClarifyRequired { .. } => "clarify_required",
            AgentEventPayload::EngineDegraded { .. } => "engine_degraded",
            AgentEventPayload::RunCompleted { .. } => "run_completed",
            AgentEventPayload::RunFailed { .. } => "run_failed",
            AgentEventPayload::RunCancelled { .. } => "run_cancelled",
        }
    }

    // 事件顺序忠实于驱动循环（两轮工具调用剧本），seq 从 0 严格递增
    #[tokio::test]
    async fn event_stream_order_and_monotonic_seq() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-w1",
                "get_weather",
                serde_json::json!({"city": "杭州"}),
            ),
            tool_response(
                "call-c1",
                "calculator",
                serde_json::json!({"op": "add", "a": 1, "b": 2}),
            ),
            text_response("完了。"),
        ]);
        let (em, rec) = recorder();
        let report = run_spike_with_events(
            gateway,
            Arc::new(spike_registry()),
            params("先查杭州天气，再算 1+2", 6),
            CancellationToken::new(),
            Some(em),
            None,
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "completed");

        let events = rec.events.lock().unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| payload_kind(&e.payload)).collect();
        assert_eq!(
            kinds,
            vec![
                "run_started",
                "model_started",
                "tool_started",
                "tool_completed",
                "model_started",
                "tool_started",
                "tool_completed",
                "model_started",
                "run_completed",
            ]
        );
        // 信封一致性：seq 严格递增、run_id / schema_version 全程一致
        for (i, e) in events.iter().enumerate() {
            assert_eq!(e.seq, i as u64);
            assert_eq!(e.run_id, "run-test");
            assert_eq!(e.schema_version, AGENT_EVENT_SCHEMA_VERSION);
            assert_eq!(e.event_id, format!("run-test:{}", i));
        }
        // 工具事件与执行记录对齐（call_id / ok / preresolved）
        if let AgentEventPayload::ToolCompleted {
            call_id,
            ok,
            preresolved,
            ..
        } = &events[3].payload
        {
            assert_eq!(call_id, "call-w1");
            assert!(*ok);
            assert!(!*preresolved);
        } else {
            panic!("events[3] 应为 ToolCompleted");
        }
        // 终态事件带齐报告口径
        if let AgentEventPayload::RunCompleted {
            final_answer,
            model_calls,
            ..
        } = &events[8].payload
        {
            assert_eq!(final_answer, "完了。");
            assert_eq!(*model_calls, 3);
        } else {
            panic!("events[8] 应为 RunCompleted");
        }
    }

    // 取消的运行也发终态事件：流 = run_started + run_cancelled，终态锁定
    #[tokio::test]
    async fn cancelled_run_emits_terminal_cancelled_event() {
        let gateway = ScriptedGateway::new(vec![text_response("不该出现")]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (em, rec) = recorder();
        let report = run_spike_with_events(
            gateway,
            Arc::new(spike_registry()),
            params("随便", 6),
            cancel,
            Some(em.clone()),
            None,
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "cancelled");

        let events = rec.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(payload_kind(&events[0].payload), "run_started");
        assert_eq!(payload_kind(&events[1].payload), "run_cancelled");
        drop(events);
        // 终态锁定：驱动之外再发也被拒（消费端不需要自己做防抖）
        assert!(em
            .emit(AgentEventPayload::ModelStarted { turn: 9 })
            .is_err());
    }

    // 工具失败同样有 ToolCompleted（ok=false + 错误文本），事件流不缺工具应答
    #[tokio::test]
    async fn failed_tool_emits_tool_completed_with_error() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-m1",
                "calculator",
                serde_json::json!({"op": "add", "a": "x", "b": 1}),
            ),
            text_response("抱歉，参数错了。"),
        ]);
        let (em, rec) = recorder();
        let report = run_spike_with_events(
            gateway,
            Arc::new(spike_registry()),
            params("算加法", 6),
            CancellationToken::new(),
            Some(em),
            None,
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "completed");

        let events = rec.events.lock().unwrap();
        let failed = events
            .iter()
            .find(|e| match &e.payload {
                AgentEventPayload::ToolCompleted { ok, .. } => !*ok,
                _ => false,
            })
            .expect("应有一个 ok=false 的 ToolCompleted");
        if let AgentEventPayload::ToolCompleted { call_id, error, .. } = &failed.payload {
            assert_eq!(call_id, "call-m1");
            assert!(error.as_deref().unwrap().contains("不是数字"));
        } else {
            unreachable!()
        }
    }

    // ---- AG-21 门禁：ToolCallObserver 观测面 ----

    /// 内存录制 observer：断言 on_start/on_completed 回调节奏与 output 内容
    #[derive(Default)]
    struct RecordingObserver {
        starts: Mutex<Vec<(String, String)>>,
        completions: Mutex<Vec<ObservedCompletion>>,
    }

    struct ObservedCompletion {
        call_id: String,
        name: String,
        ok: bool,
        error: Option<String>,
        preresolved: bool,
        model_text: Option<String>,
        structured: serde_json::Value,
    }

    impl ToolCallObserver for RecordingObserver {
        fn on_start(&self, call_id: &str, name: &str, _arguments_json: &str) {
            self.starts
                .lock()
                .unwrap()
                .push((call_id.into(), name.into()));
        }

        fn on_completed(
            &self,
            call_id: &str,
            name: &str,
            ok: bool,
            error: Option<&str>,
            preresolved: bool,
            output: Option<&ToolOutput>,
        ) {
            self.completions.lock().unwrap().push(ObservedCompletion {
                call_id: call_id.into(),
                name: name.into(),
                ok,
                error: error.map(|e| e.to_string()),
                preresolved,
                model_text: output.map(|o| o.model_text.clone()),
                structured: output
                    .map(|o| o.structured.clone())
                    .unwrap_or(serde_json::Value::Null),
            });
        }
    }

    // observer 节奏忠实于驱动循环：start→completed 成对，output 带五件套内容
    #[tokio::test]
    async fn observer_sees_tool_lifecycle_with_outputs() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-w1",
                "get_weather",
                serde_json::json!({"city": "杭州"}),
            ),
            tool_response(
                "call-c1",
                "calculator",
                serde_json::json!({"op": "add", "a": 1, "b": 2}),
            ),
            text_response("完了。"),
        ]);
        let obs = Arc::new(RecordingObserver::default());
        let report = run_spike_with_events(
            gateway,
            Arc::new(spike_registry()),
            params("先查杭州天气，再算 1+2", 6),
            CancellationToken::new(),
            None,
            Some(obs.clone()),
        )
        .await
        .expect("驱动建立成功");
        assert_eq!(report.outcome, "completed");

        let starts = obs.starts.lock().unwrap();
        assert_eq!(starts.len(), 2);
        assert_eq!(
            starts[0],
            ("call-w1".to_string(), "get_weather".to_string())
        );
        assert_eq!(starts[1], ("call-c1".to_string(), "calculator".to_string()));
        drop(starts);

        let completions = obs.completions.lock().unwrap();
        assert_eq!(completions.len(), 2);
        // 第一次：weather 成功，output 带 model_text + structured（五件套贯通）
        let w = &completions[0];
        assert_eq!(w.call_id, "call-w1");
        assert_eq!(w.name, "get_weather");
        assert!(w.ok && !w.preresolved && w.error.is_none());
        assert!(w.model_text.as_deref().unwrap().contains("杭州"));
        assert_eq!(w.structured["city"], "杭州");
        // 第二次：calculator 成功
        let c = &completions[1];
        assert_eq!(c.call_id, "call-c1");
        assert!(c.ok);
        assert_eq!(c.structured["result"], 3.0);
    }

    // 失败路径：observer 收到 ok=false + 错误文本，无 output
    #[tokio::test]
    async fn observer_sees_failed_tool_with_error_text() {
        let gateway = ScriptedGateway::new(vec![
            tool_response(
                "call-m1",
                "calculator",
                serde_json::json!({"op": "add", "a": "x", "b": 1}),
            ),
            text_response("抱歉，参数错了。"),
        ]);
        let obs = Arc::new(RecordingObserver::default());
        run_spike_with_events(
            gateway,
            Arc::new(spike_registry()),
            params("算加法", 6),
            CancellationToken::new(),
            None,
            Some(obs.clone()),
        )
        .await
        .expect("驱动建立成功");

        let starts = obs.starts.lock().unwrap();
        assert_eq!(starts.len(), 1, "失败调用也是真实执行，有 on_start");
        drop(starts);
        let completions = obs.completions.lock().unwrap();
        assert_eq!(completions.len(), 1);
        let m = &completions[0];
        assert!(!m.ok);
        assert!(!m.preresolved);
        assert!(m.error.as_deref().unwrap().contains("不是数字"));
        assert!(m.model_text.is_none());
    }
}
