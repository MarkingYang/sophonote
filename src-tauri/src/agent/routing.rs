//! Shared binding and immutable, credential-free routing evidence.
//! Automatic policy lives in automatic.rs; cross-engine retries remain prohibited.
use serde::{Deserialize, Serialize};

use super::registry::EngineId;
use super::store::RunStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RouteSource {
    Explicit,
    SessionAffinity,
    DraftPreference,
    LegacyDefault,
    Automatic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RouteDecision {
    schema_version: u32,
    policy_version: String,
    selected_engine: EngineId,
    source: RouteSource,
    requested_engine: Option<EngineId>,
    bound_engine: Option<EngineId>,
    #[serde(default)]
    pub(crate) candidates: Vec<super::candidates::Candidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shadow: Option<super::routing_shadow::Report>,
    #[serde(skip)]
    pub(crate) submission: Option<super::submission::live::Receipt>,
    #[serde(skip)]
    pub(crate) fence: Option<super::automatic::Fence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_profile: Option<super::task_profile::TaskProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferred_engine: Option<EngineId>,
}

impl RouteDecision {
    pub(crate) fn automatic(
        &mut self,
        engine: EngineId,
        fence: super::automatic::Fence,
        report: super::routing_shadow::Report,
        profile: super::task_profile::TaskProfile,
        preferred_engine: EngineId,
    ) {
        self.selected_engine = engine;
        self.source = if self.bound_engine.is_some() {
            RouteSource::SessionAffinity
        } else {
            RouteSource::Automatic
        };
        self.policy_version = "automatic-v3".into();
        self.schema_version = 5;
        self.task_profile = Some(profile);
        self.preferred_engine = Some(preferred_engine);
        self.fence = Some(fence);
        self.shadow = Some(report);
    }
    pub(crate) fn verify_fence(&self, conn: &rusqlite::Connection) -> Result<(), String> {
        if let Some(fence) = &self.fence {
            fence.verify(conn)?;
        }
        Ok(())
    }

    pub(crate) fn bound_engine(&self) -> Option<EngineId> {
        self.bound_engine
    }

    pub(crate) fn carry_preflight(&mut self, initial: &Self) -> Result<(), String> {
        if self.engine() != initial.engine() {
            return Err("会话引擎已变化，请重试".into());
        }
        self.source = initial.source;
        self.policy_version = initial.policy_version.clone();
        self.schema_version = initial.schema_version;
        self.requested_engine = initial.requested_engine;
        self.fence = initial.fence.clone();
        self.candidates = initial.candidates.clone();
        self.shadow = initial.shadow.clone();
        self.task_profile = initial.task_profile;
        self.preferred_engine = initial.preferred_engine;
        Ok(())
    }

    pub(crate) fn engine(&self) -> EngineId {
        self.selected_engine
    }
}

pub(crate) fn resolve_request(
    app: &tauri::AppHandle,
    request: &super::commands::AgentRunStartArgs,
) -> Result<RouteDecision, String> {
    let store = RunStore::new(
        rusqlite::Connection::open(crate::db::get_db_path(app)).map_err(|e| e.to_string())?,
    );
    let mut route = resolve_from_store(
        &store,
        request.thread_id.as_deref(),
        request.engine.as_deref(),
    )?;
    route.candidates = super::candidates::for_request(app, request, &route);
    Ok(route)
}

pub(crate) fn resolve_from_store(
    store: &RunStore,
    thread_id: Option<&str>,
    requested: Option<&str>,
) -> Result<RouteDecision, String> {
    let thread = thread_id
        .map(|id| {
            store
                .get_thread(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "会话不存在".to_string())
        })
        .transpose()?;
    let has_session = match thread_id {
        Some(id) => store
            .external_session_id_for_thread(id)
            .map_err(|e| e.to_string())?
            .is_some(),
        None => false,
    };
    resolve_binding(
        requested,
        thread.as_ref().map(|t| t.engine.as_str()),
        has_session || thread.as_ref().is_some_and(|t| t.latest_run_id.is_some()),
    )
}

pub(crate) fn resolve_binding(
    requested: Option<&str>,
    stored: Option<&str>,
    is_bound: bool,
) -> Result<RouteDecision, String> {
    let requested_engine = requested.map(EngineId::parse).transpose()?;
    let stored = stored.map(EngineId::parse).transpose()?;
    let bound_engine = if is_bound { stored } else { None };
    if let (Some(bound), Some(requested)) = (bound_engine, requested_engine) {
        if bound != requested {
            return Err("已有会话的智能体引擎不能切换，请新建会话".into());
        }
    }
    let selected_engine = bound_engine
        .or(requested_engine)
        .or(stored)
        .unwrap_or(EngineId::Hermes);
    let source = if bound_engine.is_some() {
        RouteSource::SessionAffinity
    } else if requested_engine.is_some() {
        RouteSource::Explicit
    } else if stored.is_some() {
        RouteSource::DraftPreference
    } else {
        RouteSource::LegacyDefault
    };
    Ok(RouteDecision {
        schema_version: 3,
        policy_version: "manual-v1".into(),
        selected_engine,
        source,
        requested_engine,
        bound_engine,
        candidates: Vec::new(),
        shadow: None,
        submission: None,
        fence: None,
        task_profile: None,
        preferred_engine: None,
    })
}

/// Safe UI projection: never forward the audit JSON or external error text.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingSummary {
    pub engine: String,
    pub policy_version: String,
    pub task_label: Option<String>,
    pub reason: String,
}

pub(crate) fn summary(raw: &str) -> Option<RoutingSummary> {
    let audit: serde_json::Value = serde_json::from_str(raw).ok()?;
    let decision: RouteDecision = serde_json::from_value(audit.get("decision")?.clone()).ok()?;
    // Policy text must also be an allowlisted value, not arbitrary DB content.
    let policy = match decision.policy_version.as_str() {
        "automatic-v3" => "automatic-v3",
        "automatic-v2" => "automatic-v2",
        "automatic-v1" => "automatic-v1",
        "manual-v1" => "manual-v1",
        _ => return None,
    };
    let mut reason = match decision.source {
        RouteSource::SessionAffinity => "延续已有会话的执行环境与原生上下文".to_string(),
        RouteSource::Automatic => decision
            .task_profile
            .map(|p| {
                p.reason(if decision.preferred_engine == Some(EngineId::ClaudeCode) {
                    "anthropic"
                } else {
                    "openai"
                })
                .to_string()
            })
            .unwrap_or_else(|| "按当时的兼容性与初始规则选择，未记录执行需求画像".into()),
        _ => "历史会话采用当时的执行器配置".to_string(),
    };
    if matches!(decision.source, RouteSource::Automatic)
        && decision
            .preferred_engine
            .is_some_and(|preferred| preferred != decision.selected_engine)
    {
        reason.push_str("；首选未通过兼容性或运行时检查，采用当前可用候选");
    }
    Some(RoutingSummary {
        engine: decision.selected_engine.as_str().into(),
        policy_version: policy.into(),
        task_label: decision.task_profile.map(|p| p.label().into()),
        reason,
    })
}

/// Per-run observation, not a cached readiness claim or a grant of tool access.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CapabilitySnapshot {
    pub(crate) engine: EngineId,
    pub(crate) adapter_version: String,
    pub(crate) transport: String,
    pub(crate) runtime_version: Option<String>,
    pub(crate) observed_at: u64,
    pub(crate) preparation: PreparationEvidence,
    pub(crate) host_tools: Option<Vec<String>>,
    pub(crate) host_delegation: bool,
    pub(crate) native_subagents: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreparationEvidence {
    ValidatedRuntime,
    HermesConfigurationOnly,
    Unavailable,
}

impl CapabilitySnapshot {
    pub(crate) fn new(
        engine: EngineId,
        runtime_version: Option<&str>,
        preparation: PreparationEvidence,
        host_tools: Option<Vec<String>>,
        observed_at: u64,
    ) -> Self {
        Self {
            engine,
            adapter_version: format!("{}-adapter-v1", engine.as_str()),
            transport: engine.transport().into(),
            runtime_version: runtime_version.map(str::to_owned),
            observed_at,
            preparation,
            host_tools,
            host_delegation: false,
            native_subagents: false,
        }
    }
}

/// Called inside the same transaction as Run claim, before native dispatch.
pub(crate) fn persist(
    conn: &rusqlite::Connection,
    run_id: &str,
    decision: &RouteDecision,
    snapshot: &CapabilitySnapshot,
) -> Result<(), String> {
    if decision.engine() != snapshot.engine {
        return Err("路由与实际执行引擎不一致".into());
    }
    let json = serde_json::to_string(&serde_json::json!({
        "decision": decision,
        "capabilities": snapshot,
    }))
    .map_err(|e| e.to_string())?;
    let changed = conn.execute(
        "UPDATE agent_runs SET route_decision_json=?1 WHERE id=?2 AND engine=?3 AND route_decision_json IS NULL",
        rusqlite::params![json, run_id, decision.engine().as_str()],
    ).map_err(|e| e.to_string())?;
    if changed != 1 {
        return Err("路由审计写入失败或运行已经绑定".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_distinguish_current_legacy_and_affinity_without_raw_content() {
        let mut route = resolve_binding(Some("pi"), None, false).unwrap();
        let summary_of = |route: &RouteDecision| {
            summary(
                &serde_json::json!({"decision":route,"capabilities":{"secret":"PRIVATE"}})
                    .to_string(),
            )
            .unwrap()
        };
        assert!(summary_of(&route).reason.contains("历史"));
        route.source = RouteSource::Automatic;
        route.policy_version = "automatic-v2".into();
        route.schema_version = 5;
        route.task_profile = Some(super::super::task_profile::TaskProfile::RepositoryExecution);
        route.preferred_engine = Some(EngineId::Opencode);
        let result = summary_of(&route);
        assert!(result.reason.contains("OpenAI"));
        assert!(result.reason.contains("首选未通过"));
        assert!(!serde_json::to_string(&result).unwrap().contains("PRIVATE"));
        for policy in ["automatic-v1", "automatic-v2", "automatic-v3"] {
            route.policy_version = policy.into();
            assert_eq!(summary_of(&route).policy_version, policy);
        }
        route.source = RouteSource::SessionAffinity;
        assert!(summary_of(&route).reason.contains("延续已有会话"));
        assert!(!summary_of(&route).reason.contains("首选未通过"));
        let mut value = serde_json::to_value(&route).unwrap();
        value.as_object_mut().unwrap().remove("taskProfile");
        value.as_object_mut().unwrap().remove("preferredEngine");
        assert!(summary(&serde_json::json!({"decision":value}).to_string()).is_some());
        assert!(summary("malformed").is_none());
        value["policyVersion"] = "PRIVATE".into();
        assert!(summary(&serde_json::json!({"decision":value}).to_string()).is_none());
    }

    #[test]
    fn native_preparation_preserves_execution_profile() {
        let mut initial = resolve_binding(Some("opencode"), None, false).unwrap();
        initial.policy_version = "automatic-v3".into();
        initial.schema_version = 5;
        initial.task_profile = Some(super::super::task_profile::TaskProfile::RepositoryExploration);
        initial.preferred_engine = Some(EngineId::Opencode);
        let mut native = resolve_binding(Some("opencode"), None, false).unwrap();
        native.carry_preflight(&initial).unwrap();
        assert_eq!(native.task_profile, initial.task_profile);
        assert_eq!(native.preferred_engine, initial.preferred_engine);
        assert_eq!(native.schema_version, 5);
        assert_eq!(native.policy_version, "automatic-v3");
    }

    #[test]
    fn default_explicit_and_native_session_affinity() {
        assert_eq!(
            resolve_binding(None, None, false).unwrap().source,
            RouteSource::LegacyDefault
        );
        for name in ["hermes", "pi", "claude_code", "opencode"] {
            let explicit = resolve_binding(Some(name), None, false).unwrap();
            assert_eq!(explicit.engine().as_str(), name);
            assert_eq!(explicit.source, RouteSource::Explicit);
            assert_eq!(
                resolve_binding(None, Some(name), false).unwrap().source,
                RouteSource::DraftPreference
            );
            assert_eq!(
                resolve_binding(None, Some(name), true).unwrap().source,
                RouteSource::SessionAffinity
            );
            for other in ["hermes", "pi", "claude_code", "opencode"] {
                assert_eq!(
                    resolve_binding(Some(other), Some(name), true).is_ok(),
                    other == name
                );
            }
        }
    }

    #[test]
    fn audit_is_immutable_matches_run_and_rolls_back_with_claim() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_runs(id TEXT PRIMARY KEY,engine TEXT,route_decision_json TEXT);",
        )
        .unwrap();
        let route = resolve_binding(Some("pi"), None, false).unwrap();
        let snapshot = CapabilitySnapshot::new(
            EngineId::Pi,
            Some("test"),
            PreparationEvidence::ValidatedRuntime,
            Some(vec!["read".into()]),
            1,
        );
        {
            let tx = conn.transaction().unwrap();
            tx.execute("INSERT INTO agent_runs(id,engine) VALUES ('r','pi')", [])
                .unwrap();
            persist(&tx, "r", &route, &snapshot).unwrap();
            assert!(persist(&tx, "r", &route, &snapshot).is_err());
            // A later claim failure must remove both the Run and its audit.
        }
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute(
            "INSERT INTO agent_runs(id,engine) VALUES ('r','hermes')",
            [],
        )
        .unwrap();
        assert!(persist(&conn, "r", &route, &snapshot).is_err());
        conn.execute("UPDATE agent_runs SET engine='pi'", [])
            .unwrap();
        persist(&conn, "r", &route, &snapshot).unwrap();
        let raw: String = conn
            .query_row("SELECT route_decision_json FROM agent_runs", [], |r| {
                r.get(0)
            })
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["decision"]["selectedEngine"], "pi");
        assert_eq!(value["capabilities"]["hostDelegation"], false);
        assert_eq!(value["capabilities"]["nativeSubagents"], false);
    }
}
