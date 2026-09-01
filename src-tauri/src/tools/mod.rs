// ============================================================
// Track B · 智能体演进（AG-06 追加）：ToolGateway 层（Phase 1 Spike 口径）
// 实施基线：docs/architecture.md「ToolGateway 是真正的核心」
//
// 边界：
// - 本模块不出现任何 rig 类型（硬性限制⑤：业务契约不依赖外部协议）；
//   rig PendingToolCall/UserContent 的拆包与回填收敛在 agent/run_controller.rs；
// - Spike 期工具为确定性假工具（tools/builtin.rs），不触达 SQLite/.md
//   （硬性限制④：Agent 不直碰存储，写工具考验点保留在 Phase 3 DocumentService）；
// - Phase 2 扩展面（已在 docs/architecture.md 定稿，本轮不预建）：ToolContext（权限/运行上下文）、
//   风险分级/审批/超时/幂等键、并行策略。AG-21 已贯通：ToolOutput 五件套
//   （model_text/structured/ui_artifact/provenance/truncated）+ UiArtifact allowlist。
// ============================================================
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use async_trait::async_trait;

use crate::model::messages::{ModelToolCall, ToolDefinition};

pub mod builtin;
// AG-08：MCP stdio 适配器。rmcp 类型只出现在 mcp.rs 内部（硬性限制⑤同款口径），
// 本文件的契约面（ToolDescriptor/ToolOutput/ToolError/SophoNoteTool）保持不依赖 rmcp。
pub mod mcp;
// AG-19：项目范围只读工具（list_project_documents / read_document）。
// 硬性限制④的 Phase 2 sanctioned 例外：docs/architecture.md 明确「Agent 可以在 Phase 2
// 通过只读工具使用 Project/Article」——仅 SELECT + 读 .md，零写入；范围按 project_id
// 逐 Run 构造隔离。Phase 3 起数据访问切换到 DocumentRepository 读接口。
pub mod project;
// AG-24（审计批次 5 第 5 步）：文档写工具（create / propose-patch / move）。
// 模型侧恒 dry-run：propose_document_patch 只产 diff 提案，落盘必须经
// 用户侧 document_apply_patch 命令批准（documents/commands.rs）。
pub mod documents;

/// 工具描述（交给模型的 function schema + 元数据）。
/// Spike 期只含三字段；风险/并行/超时/幂等元数据随 Phase 2 ToolGateway 完整化（docs/architecture.md）。
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// 来源引用（AG-21，docs/architecture.md ToolOutput.provenance）：
/// 工具结果「来源可追溯」的最小单元——来源类别 + 可选标识/标题 + 获取时间。
/// 随 ToolOutput 贯通 AgentEvent/RunStore/UI（呈现契约见 docs/architecture.md）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceRef {
    /// 来源类别：project-document / mcp / tool / web…（开放字符串，展示用）
    pub source: String,
    /// 来源标识（articleId / URL / 工具名…；无则 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// 人类可读标题（文档标题等；无则 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 获取时刻（毫秒时间戳）
    #[serde(default)]
    pub retrieved_at: u64,
}

impl ProvenanceRef {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            source_id: None,
            title: None,
            retrieved_at: now_ms(),
        }
    }

    pub fn with_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成式 UI 安全包络（AG-21，见 docs/architecture.md）：
/// kind/schema_version/payload/fallback_markdown/provenance 的非可执行信封。
/// **kind 必须来自 ALLOWED_KINDS**（allowlist 校验在 `UiArtifact::new`，
/// 未知 kind 直接拒绝——工具不得发明任意组件类型）；
/// 客户端不识别 kind 或 payload 时回退 `fallback_markdown`（仍可读）。
/// 禁止承载任意 HTML/JS/CSS（禁令 15）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiArtifact {
    /// 卡片类别（不是 React 组件名）；Phase 2 只实现有限 kind
    pub kind: String,
    /// payload 的 schema 版本（kind 内独立演进）
    pub schema_version: u32,
    /// 结构化载荷（渲染前须按 kind 约定取值；未知字段忽略）
    pub payload: serde_json::Value,
    /// 客户端不支持该 kind 时的 Markdown 回退（必须非空）
    pub fallback_markdown: String,
    /// 产物来源（可与 ToolOutput.provenance 相同或更细）
    #[serde(default)]
    pub provenance: Vec<ProvenanceRef>,
}

impl UiArtifact {
    /// kind allowlist（docs/architecture.md「只实现真正需要的有限 kind」）：
    /// table = 表格卡；key-value = 字段卡；markdown = 富文本卡；
    /// diff = 修改提案 diff 卡（AG-24 Phase 3 写路径追加，payload = PatchPreview）；
    /// rename = 标题改名提案卡（payload = RenameProposal，用户批准后前端执行完整改名）。
    /// approval-card 属 AG-26 审批 UI，届时追加。
    pub const ALLOWED_KINDS: &'static [&'static str] =
        &["table", "key-value", "markdown", "diff", "rename"];

    /// 构造并校验 allowlist。未知 kind → Err（工具层错误语义，回填模型）。
    pub fn new(
        kind: impl Into<String>,
        payload: serde_json::Value,
        fallback_markdown: impl Into<String>,
        provenance: Vec<ProvenanceRef>,
    ) -> Result<Self, ToolError> {
        let kind = kind.into();
        if !Self::ALLOWED_KINDS.contains(&kind.as_str()) {
            return Err(ToolError::Execution(format!(
                "UiArtifact kind '{}' 不在 allowlist（{:?}）",
                kind,
                Self::ALLOWED_KINDS
            )));
        }
        let fallback_markdown = fallback_markdown.into();
        Ok(Self {
            kind,
            schema_version: 1,
            payload,
            fallback_markdown,
            provenance,
        })
    }
}

/// 工具产物五件套（AG-21 贯通，docs/architecture.md）：
/// model_text 只进模型上下文；structured/ui_artifact/provenance/truncated 进
/// UI 与 RunStore——**两条通道解耦，前端卡片不从 model_text 反解析**。
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// 回填给模型的简洁文本（经 adapters::rig_tool_result 进状态机）
    pub model_text: String,
    /// 结构化结果（UI 渲染主通道之一，不从 model_text 反解析）
    pub structured: serde_json::Value,
    /// 可选 UI 安全包络（有则前端优先渲染，kind 必须过 allowlist）
    pub ui_artifact: Option<UiArtifact>,
    /// 来源引用（RunStore 可追溯；卡片展示来源行）
    pub provenance: Vec<ProvenanceRef>,
    /// 大结果截断标记（卡片显「内容已截断」提示）
    pub truncated: bool,
}

impl ToolOutput {
    /// 便捷构造：仅 model_text + structured（artifact 空、provenance 空、未截断）。
    /// 简单工具用这个；需要卡片/来源追溯的工具直接构造完整五件套。
    pub fn text(model_text: impl Into<String>, structured: serde_json::Value) -> Self {
        Self {
            model_text: model_text.into(),
            structured,
            ui_artifact: None,
            provenance: Vec::new(),
            truncated: false,
        }
    }
}

/// 工具执行错误分类（工具层错误不进异常路径——以文本交还模型自行决策，
/// 状态机要求「每个调用都有应答」，见 adapters::rig_tool_error）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// 注册表中不存在该工具（防御分支：白名单校验通常在 rig 侧先行拦截）
    UnknownTool { name: String },
    /// 参数缺失/类型不符（§十二「最多让模型修复一次」：错误文本带回模型重试）
    InvalidArguments(String),
    /// 工具自身执行失败
    Execution(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool { name } => write!(f, "未知工具: {}", name),
            Self::InvalidArguments(msg) => write!(f, "参数无效: {}", msg),
            Self::Execution(msg) => write!(f, "执行失败: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}

/// 工具实现契约（Spike 签名）。Phase 2 加 ToolContext（run_id/权限/审批通道）
/// 与 ToolDescriptor 元数据扩展，届时统一升级实现面。
#[async_trait]
pub trait SophoNoteTool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError>;
}

/// 工具注册表：名称唯一；definitions() 产出下发给模型的工具集；
/// execute() 是 Spike 期 ToolGateway 的最小形态（存在性检查 → 执行）。
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn SophoNoteTool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// 注册工具；同名后注册覆盖前者（Spike 期足够，Phase 2 注册表带校验与来源）
    pub fn register(&mut self, tool: Arc<dyn SophoNoteTool>) {
        let name = tool.descriptor().name;
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn SophoNoteTool>> {
        self.tools.get(name)
    }

    /// AG-27：按白名单收缩工具面（权限交集的执行点，docs/architecture.md）。
    /// Skill 激活时保留「声明工具 ∩ 已注册工具」，其余工具对模型不可见——
    /// Skill 只能收窄能力，不能自授权限（§八：不当成权限系统）。
    pub fn retain(&mut self, keep: &BTreeSet<String>) {
        self.tools.retain(|name, _| keep.contains(name));
    }

    /// 下发给模型的工具定义（顺序按名称排序，保证请求可复现）
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|t| {
                let d = t.descriptor();
                ToolDefinition {
                    name: d.name,
                    description: d.description,
                    input_schema: d.input_schema,
                }
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// 已注册工具名集合（ModelTurn 的 executable/allowed 白名单来源）
    pub fn names(&self) -> BTreeSet<String> {
        self.tools.keys().cloned().collect()
    }

    /// 执行单个工具调用（Spike 期 ToolGateway 主流程：存在性 → 执行）。
    /// Phase 2 在此插入 schema 校验/权限/风险/审批/超时/幂等（docs/architecture.md 的执行顺序）。
    pub async fn execute(&self, call: &ModelToolCall) -> Result<ToolOutput, ToolError> {
        let Some(tool) = self.tools.get(&call.name) else {
            return Err(ToolError::UnknownTool {
                name: call.name.clone(),
            });
        };
        tool.execute(call.arguments.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_definitions_are_sorted_and_complete() {
        let registry = builtin::spike_registry();
        let defs = registry.definitions();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "calculator");
        assert_eq!(defs[1].name, "get_weather");
        assert_eq!(
            registry.names(),
            ["calculator", "get_weather"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
    }

    #[tokio::test]
    async fn registry_rejects_unknown_tool() {
        let registry = builtin::spike_registry();
        let call = ModelToolCall {
            id: "call-x".into(),
            name: "not.registered".into(),
            arguments: serde_json::json!({}),
        };
        let err = registry.execute(&call).await.unwrap_err();
        assert_eq!(
            err,
            ToolError::UnknownTool {
                name: "not.registered".into()
            }
        );
    }

    /// AG-27：retain 收缩工具面（Skill 权限交集执行点）——只保留交集内工具，
    /// 交集为空则工具面为空（命令层在此前已拦截空交集激活）
    #[tokio::test]
    async fn registry_retain_keeps_only_intersection() {
        let mut registry = builtin::spike_registry();
        registry.retain(&["calculator"].iter().map(|s| s.to_string()).collect());
        assert_eq!(
            registry.names(),
            ["calculator"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
        assert!(registry.get("get_weather").is_none());

        registry.retain(&BTreeSet::new());
        assert!(registry.names().is_empty());
        assert!(registry.definitions().is_empty());
    }

    // ---- AG-21：UiArtifact allowlist 与 serde 契约 ----

    #[test]
    fn ui_artifact_allows_whitelisted_kinds_only() {
        for kind in UiArtifact::ALLOWED_KINDS {
            assert!(
                UiArtifact::new(*kind, serde_json::json!({}), "回退", Vec::new()).is_ok(),
                "allowlist 内 kind 应通过: {kind}"
            );
        }
        let err = UiArtifact::new("evil-html", serde_json::json!({}), "x", Vec::new())
            .expect_err("未知 kind 必须拒绝");
        assert!(err.to_string().contains("allowlist"));
    }

    #[test]
    fn ui_artifact_serializes_camel_case_envelope() {
        let artifact = UiArtifact::new(
            "key-value",
            serde_json::json!({"rows": [["city", "杭州"]]}),
            "**杭州**",
            vec![ProvenanceRef::new("tool").with_id("get_weather")],
        )
        .expect("allowlist kind");
        let json = serde_json::to_value(&artifact).expect("serialize");
        assert_eq!(json["kind"], "key-value");
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["fallbackMarkdown"], "**杭州**");
        assert_eq!(json["provenance"][0]["sourceId"], "get_weather");
        // snake_case 字段名不得泄漏（防惯例回退，AG-07 同款断言）
        assert!(json.get("schema_version").is_none());
        assert!(json.get("fallback_markdown").is_none());
    }

    #[test]
    fn tool_output_text_constructor_leaves_optional_pieces_empty() {
        let out = ToolOutput::text("模型文本", serde_json::json!({"k": 1}));
        assert_eq!(out.model_text, "模型文本");
        assert!(out.ui_artifact.is_none());
        assert!(out.provenance.is_empty());
        assert!(!out.truncated);
    }
}
