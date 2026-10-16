//! Model input is a request for work, never a source of authority.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const WORKFLOW_MS: i64 = 600_000;
pub const CHILD_MS: i64 = 180_000;
pub const MAX_CHILDREN: usize = 4;
pub const MAX_NODES: usize = 8;
pub const MAX_ITERATIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "file.read")]
    FileRead,
    #[serde(rename = "document.read")]
    DocumentRead,
    #[serde(rename = "web.read")]
    WebRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Researcher,
    Reviewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delegation {
    pub profile_id: Profile,
    pub objective: String,
    pub context_ref_ids: Vec<String>,
    pub required_capabilities: BTreeSet<Capability>,
    pub output_schema_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextRef {
    pub id: String,
    pub capability: Capability,
    /// Opaque version/hash recorded by the Host. Never supplied by the model.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskPackage {
    pub policy_version: String,
    pub delegation: Delegation,
    pub context: Vec<ContextRef>,
    pub capabilities: BTreeSet<Capability>,
    pub deadline_ms: i64,
    pub max_iterations: usize,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

impl Delegation {
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.len() > 16 * 1024 {
            return Err("子任务请求超过 16 KiB".into());
        }
        let request: Self = serde_json::from_str(json).map_err(|_| "子任务请求格式或字段无效")?;
        if request.objective.trim().is_empty() || request.objective.chars().count() > 2000 {
            return Err("子任务目标须为 1～2000 字".into());
        }
        if request.output_schema_id != "evidence-review.v1" {
            return Err("不支持的子任务结果格式".into());
        }
        if request.context_ref_ids.len() > 20
            || request.context_ref_ids.iter().any(|id| !identifier(id))
            || request
                .context_ref_ids
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != request.context_ref_ids.len()
        {
            return Err("子任务引用无效、重复或超过 20 个".into());
        }
        Ok(request)
    }

    /// Resolve the complete role minimum against Host-owned references and grants.
    /// A caller must still revalidate each reference at tool use / result receipt.
    pub fn authorize(
        &self,
        parent_depth: usize,
        available: &BTreeMap<String, ContextRef>,
        granted: &BTreeSet<Capability>,
        workflow_deadline: i64,
        now: i64,
    ) -> Result<TaskPackage, String> {
        // Validate typed callers too, not only the JSON tool entrypoint.
        Self::parse(&serde_json::to_string(self).map_err(|_| "子任务无法编码")?)?;
        if parent_depth != 0 {
            return Err("子任务不能再次委托".into());
        }
        if now < 0
            || workflow_deadline <= now
            || workflow_deadline.saturating_sub(now) > WORKFLOW_MS
        {
            return Err("整轮任务已到期或时钟无效".into());
        }
        let context: Vec<_> = self
            .context_ref_ids
            .iter()
            .map(|id| {
                available
                    .get(id)
                    .filter(|reference| reference.id == *id && !reference.version.is_empty())
                    .cloned()
                    .ok_or_else(|| "子任务引用不可见或版本缺失".to_string())
            })
            .collect::<Result<_, _>>()?;
        // Both first-release roles operate on supplied evidence. An empty package
        // is legal only for a researcher with an explicitly granted web capability.
        if context.is_empty()
            && (self.profile_id == Profile::Reviewer
                || !self.required_capabilities.contains(&Capability::WebRead))
        {
            return Err("子任务缺少可核实的资料引用".into());
        }
        let mut capabilities = self.required_capabilities.clone();
        capabilities.extend(context.iter().map(|reference| reference.capability));
        if !capabilities.is_subset(granted) {
            return Err("子任务超出父任务授权能力".into());
        }
        Ok(TaskPackage {
            policy_version: "readonly-collaboration-v1".into(),
            delegation: self.clone(),
            context,
            capabilities,
            deadline_ms: workflow_deadline.min(now.saturating_add(CHILD_MS)),
            max_iterations: MAX_ITERATIONS,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Succeeded,
    Partial,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildResult {
    pub summary: String,
    pub findings: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub unresolved: Vec<String>,
}

impl ChildResult {
    /// Model output contains no status/effects/usage fields: these are Host facts.
    pub fn parse(json: &str, package: &TaskPackage) -> Result<Self, String> {
        if json.len() > 32 * 1024 {
            return Err("子任务结果超过 32 KiB".into());
        }
        let result: Self = serde_json::from_str(json).map_err(|_| "子任务结果格式无效")?;
        if result.summary.trim().is_empty()
            || result.summary.chars().count() > 2000
            || result.evidence_refs.len() > 20
            || result.findings.len() > 20
            || result.unresolved.len() > 20
            || result
                .findings
                .iter()
                .chain(&result.unresolved)
                .any(|value| value.chars().count() > 2000)
            || result.evidence_refs.iter().collect::<BTreeSet<_>>().len()
                != result.evidence_refs.len()
        {
            return Err("子任务结果为空、重复或超过限制".into());
        }
        let visible: BTreeSet<_> = package
            .context
            .iter()
            .map(|reference| &reference.id)
            .collect();
        if result.evidence_refs.iter().any(|id| !visible.contains(id)) {
            return Err("子任务结果包含未授权的证据引用".into());
        }
        if !result.findings.is_empty() && result.evidence_refs.is_empty() {
            return Err("子任务结论缺少证据引用".into());
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Plan,
    Research,
    Synthesize,
    Proposal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub role: NodeRole,
    pub required: bool,
    pub depends_on: Vec<String>,
}

/// First-release topology is deliberately closed. Arbitrary DAGs and replanning
/// are not accepted just because they happen to be acyclic.
pub fn validate_plan(nodes: &[Node]) -> Result<(), String> {
    if !(4..=MAX_NODES).contains(&nodes.len()) {
        return Err("协作节点数量无效".into());
    }
    let by_id: BTreeMap<_, _> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    if by_id.len() != nodes.len() || nodes.iter().any(|node| !identifier(&node.id)) {
        return Err("协作节点身份无效或重复".into());
    }
    for role in [NodeRole::Plan, NodeRole::Synthesize, NodeRole::Proposal] {
        if nodes.iter().filter(|node| node.role == role).count() != 1 {
            return Err("协作阶段缺失或重复".into());
        }
    }
    let research: Vec<_> = nodes
        .iter()
        .filter(|node| node.role == NodeRole::Research)
        .collect();
    if research.is_empty() || research.len() > 2 {
        return Err("只支持最多两个只读分析节点".into());
    }
    for node in nodes {
        let parents: Vec<_> = node
            .depends_on
            .iter()
            .map(|id| by_id.get(id.as_str()).copied().ok_or("依赖节点不存在"))
            .collect::<Result<_, _>>()?;
        if node.depends_on.iter().collect::<BTreeSet<_>>().len() != parents.len() {
            return Err("依赖重复".into());
        }
        let valid = match node.role {
            NodeRole::Plan => parents.is_empty() && node.required,
            NodeRole::Research => parents.len() == 1 && parents[0].role == NodeRole::Plan,
            NodeRole::Synthesize => {
                node.required
                    && parents.len() == research.len()
                    && parents
                        .iter()
                        .all(|parent| parent.role == NodeRole::Research)
            }
            NodeRole::Proposal => {
                node.required && parents.len() == 1 && parents[0].role == NodeRole::Synthesize
            }
        };
        if !valid {
            return Err("仅支持规划、只读分析、汇总和提案的固定流程".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn request() -> serde_json::Value {
        json!({"profileId":"reviewer","objective":"核对结论","contextRefIds":["ref-1"],"requiredCapabilities":[],"outputSchemaId":"evidence-review.v1"})
    }
    fn package() -> TaskPackage {
        let reference = ContextRef {
            id: "ref-1".into(),
            capability: Capability::DocumentRead,
            version: "version-1".into(),
        };
        Delegation::parse(&request().to_string())
            .unwrap()
            .authorize(
                0,
                &BTreeMap::from([("ref-1".into(), reference)]),
                &BTreeSet::from([Capability::DocumentRead]),
                600_001,
                1,
            )
            .unwrap()
    }
    #[test]
    fn delegation_cannot_supply_authority_or_drop_role_minimum() {
        for field in [
            "parentRunId",
            "engine",
            "apiKey",
            "workspaceRoot",
            "permissionMode",
            "maxIterations",
        ] {
            let mut raw = request();
            raw[field] = json!("forged");
            assert!(Delegation::parse(&raw.to_string()).is_err(), "{field}");
        }
        let mut raw = request();
        raw["requiredCapabilities"] = json!(["file.write"]);
        assert!(Delegation::parse(&raw.to_string()).is_err());
        let request = Delegation::parse(&request().to_string()).unwrap();
        let allowed = BTreeMap::from([("ref-1".into(), package().context[0].clone())]);
        assert!(request
            .authorize(0, &allowed, &BTreeSet::new(), 1000, 1)
            .is_err());
        assert!(request
            .authorize(1, &allowed, &package().capabilities, 1000, 1)
            .is_err());
        assert!(request
            .authorize(0, &BTreeMap::new(), &package().capabilities, 1000, 1)
            .is_err());
        assert_eq!(
            package().capabilities,
            BTreeSet::from([Capability::DocumentRead])
        );
        assert_eq!(package().deadline_ms, CHILD_MS + 1);
    }
    #[test]
    fn input_bounds_versions_and_remaining_deadline_are_enforced() {
        for refs in [
            json!(["../../secret"]),
            json!(["ref-1", "ref-1"]),
            json!(["file:///secret"]),
        ] {
            let mut raw = request();
            raw["contextRefIds"] = refs;
            assert!(Delegation::parse(&raw.to_string()).is_err());
        }
        let mut raw = request();
        raw["objective"] = json!("字".repeat(2001));
        assert!(Delegation::parse(&raw.to_string()).is_err());
        let request = Delegation::parse(&request().to_string()).unwrap();
        let allowed = BTreeMap::from([("ref-1".into(), package().context[0].clone())]);
        assert_eq!(
            request
                .authorize(0, &allowed, &package().capabilities, 1000, 1)
                .unwrap()
                .deadline_ms,
            1000
        );
        assert!(request
            .authorize(0, &allowed, &package().capabilities, 1000, 1000)
            .is_err());
        assert!(request
            .authorize(0, &allowed, &package().capabilities, WORKFLOW_MS + 2, 1)
            .is_err());
    }
    #[test]
    fn model_result_cannot_claim_status_or_invent_sources() {
        let raw = json!({"summary":"结论","findings":["结论有来源"],"evidenceRefs":["ref-1"],"unresolved":[]});
        assert!(ChildResult::parse(&raw.to_string(), &package()).is_ok());
        for (key, value) in [
            ("effects", json!([])),
            ("status", json!("succeeded")),
            ("evidenceRefs", json!(["other-project"])),
            ("summary", json!("字".repeat(2001))),
        ] {
            let mut changed = raw.clone();
            changed[key] = value;
            assert!(ChildResult::parse(&changed.to_string(), &package()).is_err());
        }
    }
    #[test]
    fn fixed_graph_rejects_cycles_missing_edges_and_parallel_writers() {
        let node = |id: &str, role, deps: &[&str]| Node {
            id: id.into(),
            role,
            required: true,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        };
        let nodes = vec![
            node("p", NodeRole::Plan, &[]),
            node("a", NodeRole::Research, &["p"]),
            node("b", NodeRole::Research, &["p"]),
            node("s", NodeRole::Synthesize, &["a", "b"]),
            node("out", NodeRole::Proposal, &["s"]),
        ];
        assert!(validate_plan(&nodes).is_ok());
        for index in 0..nodes.len() {
            let mut bad = nodes.clone();
            bad[index].depends_on = vec![bad[index].id.clone()];
            assert!(validate_plan(&bad).is_err());
        }
        let mut bad = nodes.clone();
        bad[3].depends_on.pop();
        assert!(validate_plan(&bad).is_err());
        let mut bad = nodes.clone();
        bad[1].role = NodeRole::Proposal;
        assert!(validate_plan(&bad).is_err());
        let mut bad = nodes.clone();
        bad[2].id = "a".into();
        assert!(validate_plan(&bad).is_err());
    }
}
