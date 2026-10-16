//! Conservative preflight evidence. Neither a candidate nor cached runtime evidence
//! authorizes execution: the selected adapter must still prepare every run.
use serde::{Deserialize, Serialize};

use super::{commands::AgentRunStartArgs, registry::EngineId, routing::RouteDecision};
use crate::model::openai_compat::ProviderSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Exclusion {
    SessionBinding,
    ExplicitSelection,
    NativeHermesFeature,
    UnsupportedProtocol,
    ModelNotConfigured,
    InvalidPermissionMode,
    CollaborationUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PendingCheck {
    ModelConfiguration,
    NativeModelCatalog,
    RuntimeVerification,
    Credentials,
    ScopeAndApproval,
    Resources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateState {
    Excluded,
    NeedsPreparation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Candidate {
    pub(crate) engine: EngineId,
    state: CandidateState,
    exclusions: Vec<Exclusion>,
    pending_checks: Vec<PendingCheck>,
    runtime: super::runtime_observations::Observation,
    effective_host_tools: Option<Vec<String>>,
}

impl Candidate {
    pub(crate) fn can_probe(&self) -> bool {
        self.exclusions.is_empty()
    }

    /// A rule may recommend preparation, never dispatch, with these facts.
    pub(crate) fn can_recommend_preparation(&self) -> bool {
        self.can_probe()
            && !self
                .pending_checks
                .contains(&PendingCheck::ModelConfiguration)
            && !self
                .pending_checks
                .contains(&PendingCheck::NativeModelCatalog)
    }
}

/// Known incompatibilities fail before runtime preparation. Pending checks
/// remain the responsibility of the selected adapter.
pub(crate) fn validate_selected(candidates: &[Candidate], engine: EngineId) -> Result<(), String> {
    let selected = candidates
        .iter()
        .find(|c| c.engine == engine)
        .ok_or("所选引擎缺少候选检查，请重试")?;
    let Some(reason) = selected.exclusions.first() else {
        return Ok(());
    };
    Err(match reason {
        Exclusion::SessionBinding => "已有会话的智能体引擎不能切换，请新建会话",
        Exclusion::ExplicitSelection => "候选引擎与手动选择不一致，请重试",
        Exclusion::NativeHermesFeature => {
            "所选引擎不支持 Hermes 的 Skill、原生命令或项目资料范围，请移除后重试"
        }
        Exclusion::UnsupportedProtocol => "所选引擎不支持当前供应商协议，请检查模型设置",
        Exclusion::ModelNotConfigured => "所选模型不在供应商配置白名单中，请检查模型设置",
        Exclusion::InvalidPermissionMode => "工作区权限模式无效",
        Exclusion::CollaborationUnsupported => "当前会话无法使用只读协作，请新建任务或关闭本轮协作",
    }
    .into())
}

/// Uses the same protocol adaptation as execution. No endpoint or credential is
/// serialized; no Keychain prompt, network call, or runtime installation occurs.
pub(crate) fn screen(
    request: &AgentRunStartArgs,
    route: &RouteDecision,
    provider: Option<&ProviderSnapshot>,
    observation: impl Fn(EngineId) -> super::runtime_observations::Observation,
) -> Vec<Candidate> {
    EngineId::ALL
        .into_iter()
        .map(|engine| {
            let mut exclusions = Vec::new();
            let mut pending_checks = vec![
                PendingCheck::RuntimeVerification,
                PendingCheck::Credentials,
                PendingCheck::ScopeAndApproval,
                PendingCheck::Resources,
            ];
            if route.bound_engine().is_some_and(|bound| bound != engine) {
                exclusions.push(Exclusion::SessionBinding);
            }
            if request
                .engine
                .as_deref()
                .is_some_and(|requested| requested != engine.as_str())
            {
                exclusions.push(Exclusion::ExplicitSelection);
            }
            if request.read_only_collaboration && engine != EngineId::Pi {
                exclusions.push(Exclusion::CollaborationUnsupported);
            }
            if engine != EngineId::Hermes && super::task_profile::requires_native(request) {
                exclusions.push(Exclusion::NativeHermesFeature);
            }
            if !matches!(
                request
                    .workspace_permission_mode
                    .as_deref()
                    .unwrap_or("ask"),
                "ask" | "autoEdit" | "plan"
            ) {
                exclusions.push(Exclusion::InvalidPermissionMode);
            }
            if engine == EngineId::Hermes {
                // Hermes slugs/model defaults are resolved from its native catalogue;
                // a Host default provider is not evidence about that selection.
                pending_checks.push(PendingCheck::NativeModelCatalog);
            } else if let Some(provider) = provider {
                use super::model_preparation::{select, PreparationError};
                match select(engine, provider.clone(), request.hermes_model.as_deref()) {
                    Ok(_) => {}
                    Err(PreparationError::UnsupportedProtocol) => {
                        exclusions.push(Exclusion::UnsupportedProtocol)
                    }
                    Err(PreparationError::ModelNotConfigured) => {
                        exclusions.push(Exclusion::ModelNotConfigured)
                    }
                    // Metadata screening never acquires credentials. Any other
                    // unresolved contract remains unknown, not compatible.
                    Err(_) => pending_checks.push(PendingCheck::ModelConfiguration),
                }
            } else {
                pending_checks.push(PendingCheck::ModelConfiguration);
            }
            Candidate {
                engine,
                state: if exclusions.is_empty() {
                    CandidateState::NeedsPreparation
                } else {
                    CandidateState::Excluded
                },
                exclusions,
                pending_checks,
                runtime: observation(engine),
                effective_host_tools: (engine != EngineId::Hermes).then(|| {
                    let mode = request
                        .workspace_permission_mode
                        .as_deref()
                        .unwrap_or("ask");
                    if matches!(mode, "ask" | "autoEdit" | "plan") {
                        super::capabilities::prepared_host_tools(mode)
                    } else {
                        Vec::new()
                    }
                }),
            }
        })
        .collect()
}

pub(crate) fn for_request(
    app: &tauri::AppHandle,
    request: &AgentRunStartArgs,
    route: &RouteDecision,
) -> Vec<Candidate> {
    let provider =
        crate::model::openai_compat::resolve_provider(app, request.hermes_provider.as_deref()).ok();
    let namespace = crate::db::get_db_path(app);
    screen(request, route, provider.as_ref(), |engine| {
        super::runtime_observations::get(&namespace, engine)
    })
}

#[cfg(test)]
mod tests {
    use super::super::{routing::resolve_binding, runtime_observations::Observation};
    use super::*;

    fn request(value: serde_json::Value) -> AgentRunStartArgs {
        serde_json::from_value(value).unwrap()
    }
    fn provider(protocol: &str, url: &str) -> ProviderSnapshot {
        ProviderSnapshot {
            id: "fixture".into(),
            protocol: protocol.into(),
            base_url: url.into(),
            model: "model".into(),
            models: vec!["model".into()],
            requires_key: true,
        }
    }
    fn candidates(
        request: &AgentRunStartArgs,
        provider: Option<&ProviderSnapshot>,
    ) -> Vec<Candidate> {
        screen(
            request,
            &resolve_binding(request.engine.as_deref(), None, false).unwrap(),
            provider,
            |_| Observation::Unknown,
        )
    }

    #[test]
    fn protocol_matrix_reuses_claude_adapter_and_never_rewrites_proxy() {
        let request = request(serde_json::json!({"message":"hello"}));
        for (protocol, url, claude_supported) in [
            ("openai", "https://example.test/v1", false),
            ("anthropic", "https://example.test", true),
            ("openai", "https://api.deepseek.com/v1", true),
            ("openai", "https://api.deepseek.com.evil.test/v1", false),
            ("unsupported", "https://example.test", false),
        ] {
            let rows = candidates(&request, Some(&provider(protocol, url)));
            let claude = rows
                .iter()
                .find(|c| c.engine == EngineId::ClaudeCode)
                .unwrap();
            assert_eq!(
                !claude.exclusions.contains(&Exclusion::UnsupportedProtocol),
                claude_supported
            );
            for c in rows
                .iter()
                .filter(|c| matches!(c.engine, EngineId::Pi | EngineId::Opencode))
            {
                assert_eq!(
                    c.exclusions.contains(&Exclusion::UnsupportedProtocol),
                    protocol == "unsupported"
                );
            }
        }
    }

    #[test]
    fn binding_manual_choice_and_native_features_are_intersected() {
        let req = request(serde_json::json!({"message":"hello", "engine":"pi", "skill":"native"}));
        let route = resolve_binding(Some("pi"), Some("pi"), true).unwrap();
        let rows = screen(&req, &route, None, |_| Observation::Unknown);
        assert!(rows
            .iter()
            .all(|c| matches!(c.state, CandidateState::Excluded)));
        assert!(rows[0].exclusions.contains(&Exclusion::SessionBinding));
        assert!(rows[0].exclusions.contains(&Exclusion::ExplicitSelection));
        assert!(rows[1].exclusions.contains(&Exclusion::NativeHermesFeature));
        assert!(validate_selected(&rows, EngineId::Pi).is_err());
    }

    #[test]
    fn unknown_runtime_credentials_scope_never_become_ready() {
        let req = request(serde_json::json!({"message":"hello"}));
        let rows = candidates(&req, None);
        assert_eq!(rows.len(), 4);
        assert!(validate_selected(&rows, EngineId::Pi).is_ok());
        assert!(validate_selected(&[], EngineId::Pi).is_err());
        for c in &rows {
            assert!(matches!(c.state, CandidateState::NeedsPreparation));
            for check in [
                PendingCheck::RuntimeVerification,
                PendingCheck::Credentials,
                PendingCheck::ScopeAndApproval,
                PendingCheck::Resources,
            ] {
                assert!(c.pending_checks.contains(&check));
            }
        }
        assert!(rows[0]
            .pending_checks
            .contains(&PendingCheck::NativeModelCatalog));
        assert!(rows[1]
            .pending_checks
            .contains(&PendingCheck::ModelConfiguration));
    }

    #[test]
    fn cached_runtime_does_not_grant_credentials_or_plan_write_tools() {
        let req = request(serde_json::json!({"message":"hello", "workspacePermissionMode":"plan"}));
        let route = resolve_binding(None, None, false).unwrap();
        let rows = screen(
            &req,
            &route,
            Some(&provider("anthropic", "https://example.test")),
            |_| Observation::Verified {
                version: "fixture".into(),
                checked_at_ms: 1,
            },
        );
        for row in rows {
            assert!(matches!(row.state, CandidateState::NeedsPreparation));
            assert!(row.pending_checks.contains(&PendingCheck::Credentials));
            assert!(row
                .pending_checks
                .contains(&PendingCheck::RuntimeVerification));
            if let Some(tools) = row.effective_host_tools {
                assert!(!tools
                    .iter()
                    .any(|t| matches!(t.as_str(), "write" | "edit" | "bash")));
                assert!(tools.iter().any(|t| t == "read"));
            } else {
                assert_eq!(row.engine, EngineId::Hermes);
            }
        }
    }

    #[test]
    fn fresh_configuration_is_used_and_audit_contains_no_task_or_endpoint() {
        let req = request(
            serde_json::json!({"message":"PRIVATE TASK", "hermesModel":"missing", "workspacePermissionMode":"invalid"}),
        );
        let p = provider("openai", "https://secret.example.test");
        let rows = candidates(&req, Some(&p));
        assert!(rows
            .iter()
            .all(|c| c.exclusions.contains(&Exclusion::InvalidPermissionMode)));
        assert!(rows[1].exclusions.contains(&Exclusion::ModelNotConfigured));
        let mut changed = p;
        changed.models.push("missing".into());
        assert!(!candidates(&req, Some(&changed))[1]
            .exclusions
            .contains(&Exclusion::ModelNotConfigured));
        let json = serde_json::to_string(&rows).unwrap();
        assert!(!json.contains("PRIVATE"));
        assert!(!json.contains("secret.example"));
    }
}
