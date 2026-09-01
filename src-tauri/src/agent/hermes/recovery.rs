//! H4 / NEXT-021：SSE 有限重连 + GET `/v1/runs/{id}` 对账。
//! 不可恢复 → `interrupted`；**禁止**猜测 `completed`。

use std::sync::Arc;

use serde_json::Value;

use crate::agent::events::{AgentEventPayload, EventEmitter};

use super::client::HermesClientError;
use super::event_mapper::HermesEventMapper;
use super::runs_client::{HermesRunsClient, SseStreamResult};
use super::user_facing::sanitize_user_facing_text;

/// 初始连接之外最多再重连次数
pub const MAX_SSE_RECONNECTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// 远端已达终态（completed / failed / cancelled）
    Terminal,
    /// 对账后仍不可恢复 → 已发出 `run_failed{outcome:interrupted}`
    Interrupted { reason: String },
}

/// 带 Last-Event-ID 的 SSE 消费：断流后最多重连 3 次，再 `get_run` 对账。
///
/// `on_raw`：每个 stub JSON（含已映射前）；用于 tool.started 等副作用。
/// 映射后的 AgentEvent 经 `emitter` 发出（若有）。
/// `last_external_event_id`：进出均为当前已处理的最大外部事件 id。
pub async fn consume_with_recovery<F, Fut>(
    runs: &HermesRunsClient,
    external_run_id: &str,
    emitter: &Option<Arc<EventEmitter>>,
    last_external_event_id: &mut Option<String>,
    mut on_raw: F,
) -> Result<RecoveryOutcome, HermesClientError>
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Result<(), HermesClientError>>,
{
    let mut reconnects: u32 = 0;
    // 必须跨 SSE 重连保留：旧 Hermes tool.completed 没有 call_id，配对状态
    // 若每次重连清空会把同一次工具调用拆成两张卡。
    let mut event_mapper = HermesEventMapper::default();
    loop {
        let resume = last_external_event_id.clone();
        let stream_result = runs
            .stream_events_from(external_run_id, resume.as_deref(), |id, ev| {
                if let Some(eid) = id.clone() {
                    *last_external_event_id = Some(eid);
                }
                if let Some(payload) = event_mapper.map(&ev) {
                    emit_opt(emitter, payload);
                }
                on_raw(ev)
            })
            .await;

        match stream_result {
            Ok(SseStreamResult::ReachedTerminal { last_event_id }) => {
                if let Some(id) = last_event_id {
                    *last_external_event_id = Some(id);
                }
                return Ok(RecoveryOutcome::Terminal);
            }
            Ok(SseStreamResult::ConnectionEnded { last_event_id }) => {
                if let Some(id) = last_event_id {
                    *last_external_event_id = Some(id);
                }
            }
            Err(e) => {
                // 传输错误视同断流，进入重连/对账
                if reconnects >= MAX_SSE_RECONNECTS {
                    return reconcile_or_interrupt(
                        runs,
                        external_run_id,
                        emitter,
                        format!("sse_error:{e}"),
                    )
                    .await;
                }
                reconnects += 1;
                emit_opt(
                    emitter,
                    AgentEventPayload::EngineDegraded {
                        reason: format!("sse_error:{e}"),
                        reconnecting: true,
                    },
                );
                continue;
            }
        }

        // 断流且未终态
        if reconnects >= MAX_SSE_RECONNECTS {
            return reconcile_or_interrupt(
                runs,
                external_run_id,
                emitter,
                "sse_reconnect_exhausted".into(),
            )
            .await;
        }
        reconnects += 1;
        emit_opt(
            emitter,
            AgentEventPayload::EngineDegraded {
                reason: "sse_dropped".into(),
                reconnecting: true,
            },
        );
    }
}

async fn reconcile_or_interrupt(
    runs: &HermesRunsClient,
    external_run_id: &str,
    emitter: &Option<Arc<EventEmitter>>,
    reason: String,
) -> Result<RecoveryOutcome, HermesClientError> {
    let status = match runs.get_run(external_run_id).await {
        Ok(s) => s,
        Err(e) => {
            emit_interrupted(emitter, format!("{reason}; get_run failed: {e}"));
            return Ok(RecoveryOutcome::Interrupted {
                reason: format!("{reason}; get_run failed: {e}"),
            });
        }
    };

    match status.status.as_str() {
        "completed" => {
            // 远端已终态：用对账结果补写，不是猜测
            let output = sanitize_user_facing_text(&status.output);
            if !output.is_empty() {
                emit_opt(
                    emitter,
                    AgentEventPayload::MessageCompleted {
                        text: output.clone(),
                    },
                );
            }
            emit_opt(
                emitter,
                AgentEventPayload::RunCompleted {
                    outcome: "completed".into(),
                    final_answer: output,
                    model_calls: 0,
                },
            );
            Ok(RecoveryOutcome::Terminal)
        }
        "failed" => {
            emit_opt(
                emitter,
                AgentEventPayload::RunFailed {
                    outcome: "failed".into(),
                    error: if status.output.is_empty() {
                        "remote failed".into()
                    } else {
                        status.output
                    },
                },
            );
            Ok(RecoveryOutcome::Terminal)
        }
        "cancelled" => {
            emit_opt(
                emitter,
                AgentEventPayload::RunCancelled {
                    reason: "remote cancelled".into(),
                },
            );
            Ok(RecoveryOutcome::Terminal)
        }
        // running / unknown → 禁止假 completed
        other => {
            let msg = format!("SSE 对账不可恢复（status={other}）：{reason}");
            emit_interrupted(emitter, msg.clone());
            Ok(RecoveryOutcome::Interrupted { reason: msg })
        }
    }
}

fn emit_interrupted(emitter: &Option<Arc<EventEmitter>>, error: String) {
    emit_opt(
        emitter,
        AgentEventPayload::EngineDegraded {
            reason: error.clone(),
            reconnecting: false,
        },
    );
    emit_opt(
        emitter,
        AgentEventPayload::RunFailed {
            outcome: "interrupted".into(),
            error,
        },
    );
}

fn emit_opt(events: &Option<Arc<EventEmitter>>, payload: AgentEventPayload) {
    if let Some(em) = events {
        let _ = em.emit(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_reconnects_is_three() {
        assert_eq!(MAX_SSE_RECONNECTS, 3);
    }
}
