//! H3/H4：Hermes 只读 Run 引擎（协议 stub + Adapter 内执行只读工具）。
//! 默认生产路径仍为 Rig；本引擎供测试与后续双跑使用。
//! H4：经 `recovery` 做 SSE 重连/对账，禁止假 completed。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::engine::{AgentEngine, EngineError, RunEnvelope};
use crate::agent::events::AgentEventPayload;
use crate::agent::run_controller::SpikeRunReport;
use crate::model::messages::ModelToolCall;
use crate::tools::ToolRegistry;

use super::config::HermesSidecarConfig;
use super::recovery::{consume_with_recovery, RecoveryOutcome};
use super::runs_client::HermesRunsClient;
use super::supervisor::start_sidecar;
use super::{ENGINE_ID, STUB_PROTOCOL_VERSION};

/// H3 允许的只读工具名
pub const READONLY_TOOL_NAMES: &[&str] = &["list_project_documents", "read_document"];

const WRITE_TOOL_NAMES: &[&str] = &[
    "create_document",
    "propose_document_patch",
    "move_document",
    "rename_article",
    "set_document_parent",
];

/// 校验 registry 仅含只读工具；写工具或未知工具名一律拒绝。
pub fn assert_readonly_registry(reg: &ToolRegistry) -> Result<(), EngineError> {
    let allowed: BTreeSet<&str> = READONLY_TOOL_NAMES.iter().copied().collect();
    for name in reg.names() {
        if WRITE_TOOL_NAMES.contains(&name.as_str()) {
            return Err(EngineError::Setup(format!("H3 只读路径禁止写工具: {name}")));
        }
        if !allowed.contains(name.as_str()) {
            return Err(EngineError::Setup(format!("H3 只读路径不允许工具: {name}")));
        }
    }
    if reg.names().is_empty() {
        return Err(EngineError::Setup("H3 只读 registry 为空".into()));
    }
    Ok(())
}

/// 从 list 工具输出中解析首个 articleId（与 stub 脚本协同）
pub fn first_article_id_from_list_text(text: &str) -> Option<String> {
    for part in text.split("articleId:") {
        let id = part
            .split_whitespace()
            .next()?
            .trim_matches(|c| c == '）' || c == ')' || c == ',' || c == '。' || c == '\n');
        if id.len() >= 8 {
            return Some(id.to_string());
        }
    }
    None
}

#[derive(Clone)]
pub struct HermesReadonlyEngine {
    pub sidecar: HermesSidecarConfig,
}

impl HermesReadonlyEngine {
    pub fn new(sidecar: HermesSidecarConfig) -> Self {
        Self { sidecar }
    }
}

impl AgentEngine for HermesReadonlyEngine {
    fn engine_id(&self) -> &'static str {
        ENGINE_ID
    }

    fn engine_version(&self) -> &'static str {
        STUB_PROTOCOL_VERSION
    }

    fn health(&self) -> Result<(), EngineError> {
        super::config::verify_binary_hash(&self.sidecar.binary_path, &self.sidecar.expected_sha256)
            .map_err(EngineError::Unhealthy)
    }

    async fn run_with_events(&self, envelope: RunEnvelope) -> Result<SpikeRunReport, EngineError> {
        assert_readonly_registry(&envelope.registry)?;
        self.health()?;

        let handle = start_sidecar(&self.sidecar)
            .await
            .map_err(|e| EngineError::Unhealthy(e.to_string()))?;

        let runs = HermesRunsClient::new(handle.base_url.clone(), handle.bearer.clone());
        let article_hint = envelope
            .context_pack
            .as_ref()
            .and_then(|p| p.get("articleIdHint"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let created = runs
            .create_run(
                &envelope.params.user,
                envelope.context_pack.as_ref(),
                article_hint.as_deref(),
            )
            .await
            .map_err(|e| EngineError::Setup(e.to_string()))?;

        let registry = envelope.registry.clone();
        let emitter = envelope.events.clone();
        let observer = envelope.observer.clone();
        let cancel = envelope.cancel.clone();

        let mut report = SpikeRunReport {
            outcome: String::new(),
            final_answer: String::new(),
            model_calls: 0,
            tool_executions: Vec::new(),
            invalid_resolutions: 0,
            usage: Default::default(),
            transcript: Vec::new(),
            error: None,
        };

        let external_run_id = created.run_id.clone();
        let runs_for_cb = runs.clone();
        let mut last_external_event_id: Option<String> = None;

        let recovery = consume_with_recovery(
            &runs,
            &external_run_id,
            &emitter,
            &mut last_external_event_id,
            |ev| {
                let registry = registry.clone();
                let observer = observer.clone();
                let cancel = cancel.clone();
                let runs_for_cb = runs_for_cb.clone();
                let external_run_id = external_run_id.clone();
                async move {
                    if cancel.is_cancelled() {
                        let _ = runs_for_cb.stop(&external_run_id).await;
                        return Err(super::client::HermesClientError::Transport(
                            "cancelled".into(),
                        ));
                    }

                    let ty = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if ty == "tool.started" {
                        let call_id = ev
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = ev
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = ev
                            .get("arguments")
                            .cloned()
                            .unwrap_or(Value::Object(Default::default()));

                        if WRITE_TOOL_NAMES.contains(&name.as_str()) {
                            return Err(super::client::HermesClientError::Parse(format!(
                                "write tool forbidden: {name}"
                            )));
                        }

                        if let Some(obs) = &observer {
                            obs.on_start(&call_id, &name, &args.to_string());
                        }

                        let call = ModelToolCall {
                            id: call_id.clone(),
                            name: name.clone(),
                            arguments: args,
                        };
                        let (ok, text, output) = match registry.execute(&call).await {
                            Ok(out) => {
                                let text = out.model_text.clone();
                                (true, text, Some(out))
                            }
                            Err(e) => (false, e.to_string(), None),
                        };

                        if let Some(obs) = &observer {
                            obs.on_completed(
                                &call_id,
                                &name,
                                ok,
                                if ok { None } else { Some(text.as_str()) },
                                false,
                                output.as_ref(),
                            );
                        }

                        runs_for_cb
                            .post_tool_result(&external_run_id, &call_id, &name, ok, &text)
                            .await?;
                    }
                    Ok(())
                }
            },
        )
        .await;

        let status = runs.get_run(&external_run_id).await.ok();
        let _ = handle.shutdown();

        match recovery {
            Ok(RecoveryOutcome::Terminal) => {
                if let Some(status) = status {
                    match status.status.as_str() {
                        "completed" => {
                            report.outcome = "completed".into();
                            report.final_answer = status.output;
                        }
                        "cancelled" => {
                            report.outcome = "cancelled".into();
                        }
                        "failed" => {
                            report.outcome = "failed".into();
                            report.error = Some(status.output);
                        }
                        other => {
                            // 事件已终态但对账状态异常：仍不猜 completed
                            report.outcome = "interrupted".into();
                            report.error = Some(format!("post-terminal status={other}"));
                        }
                    }
                } else {
                    report.outcome = "completed".into();
                }
                Ok(report)
            }
            Ok(RecoveryOutcome::Interrupted { reason }) => {
                report.outcome = "interrupted".into();
                report.error = Some(reason);
                Ok(report)
            }
            Err(e) => {
                if e.to_string().contains("cancelled") || cancel.is_cancelled() {
                    report.outcome = "cancelled".into();
                    emit_opt(
                        &emitter,
                        AgentEventPayload::RunCancelled {
                            reason: "cancelled".into(),
                        },
                    );
                    return Ok(report);
                }
                report.outcome = "failed".into();
                report.error = Some(e.to_string());
                emit_opt(
                    &emitter,
                    AgentEventPayload::RunFailed {
                        outcome: "failed".into(),
                        error: e.to_string(),
                    },
                );
                Ok(report)
            }
        }
    }
}

fn emit_opt(events: &Option<Arc<crate::agent::events::EventEmitter>>, payload: AgentEventPayload) {
    if let Some(em) = events {
        let _ = em.emit(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::project::{ListProjectDocumentsTool, ReadDocumentTool};
    use std::path::PathBuf;

    #[test]
    fn rejects_write_tools() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ListProjectDocumentsTool::new(
            PathBuf::from("/tmp/x.db"),
            "p".into(),
        )));
        assert!(assert_readonly_registry(&reg).is_ok());

        reg.register(Arc::new(
            crate::tools::documents::ProposeDocumentPatchTool::new(
                PathBuf::from("/tmp/x.db"),
                PathBuf::from("/tmp/notes"),
                "p".into(),
                "r".into(),
            ),
        ));
        let err = assert_readonly_registry(&reg).unwrap_err().to_string();
        assert!(err.contains("禁止写工具") || err.contains("propose"));
    }

    #[test]
    fn rejects_spike_tools_by_default() {
        let reg = crate::tools::builtin::spike_registry();
        assert!(assert_readonly_registry(&reg).is_err());
    }

    #[test]
    fn accepts_list_and_read() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ListProjectDocumentsTool::new(
            PathBuf::from("/tmp/x.db"),
            "p".into(),
        )));
        reg.register(Arc::new(ReadDocumentTool::new(
            PathBuf::from("/tmp/x.db"),
            PathBuf::from("/tmp/notes"),
            "p".into(),
        )));
        assert!(assert_readonly_registry(&reg).is_ok());
    }
}
