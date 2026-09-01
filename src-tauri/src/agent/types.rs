// ============================================================
// Track B · Phase 2 (AG-13 追加)：Agent 数据域类型定义
// 实施基线：docs/architecture.md — agent_threads / agent_runs /
//           agent_messages / agent_tool_calls / agent_approvals / agent_run_events
//
// 口径：
// - Thread = 用户对话的容器（一个 Thread = 一次「话题」）；
// - Run = Thread 内的一次 Agent 完整执行（可能跨越多个模型调用回合）；
// - Message = 线程中一条消息（User 输入或 Assistant 输出）；
// - ToolCall = 单次工具调用的元数据记录（name/arguments/result/error）；
// - Approval = 审批请求（Phase 3 DocumentService 用，本阶段表结构预留）；
// - Event = 版本化 AgentEvent 持久化条目（见 events.rs 契约）。
// ============================================================
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use crate::agent::events::AGENT_EVENT_SCHEMA_VERSION;

/// Thread 状态（Spike 期仅 running/completed/cancelled/failed，Phase 3 扩展 waiting_approval/interrupted）
/// serde 口径（AG-15 契约收口）：JSON 输出 snake_case 小写，与前端 agentStore.ts
/// ThreadStatus 联合类型一致；alias 兼容 dev 库旧格式 PascalCase（`"Running"`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    #[serde(alias = "Running")]
    Running,
    #[serde(alias = "Completed")]
    Completed,
    #[serde(alias = "Cancelled")]
    Cancelled,
    #[serde(alias = "Failed")]
    Failed,
}

/// Thread：用户话题容器。
/// serde 口径（AG-15 契约收口）：camelCase 对齐前端 agentStore.ts AgentThread。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThread {
    pub id: String,
    /// 标题（用户可编辑）
    pub title: String,
    /// 状态
    pub status: ThreadStatus,
    /// 关联项目 ID（None = 全局/未归属话题）
    pub project_id: Option<String>,
    /// 最近一次运行的 run_id（供 UI 快速定位最新对话）
    pub latest_run_id: Option<String>,
    pub created_at: u64, // ms epoch
    pub updated_at: u64,
    /// 关闭进历史的时间；None = 仍在活跃 tab
    #[serde(default)]
    pub closed_at: Option<u64>,
    /// 归档时间；归档后不对 UI 列出；仅当配置了正 TTL 才可能逾时硬删
    #[serde(default)]
    pub archived_at: Option<u64>,
    /// 置顶时间；None = 未置顶。置顶会话在侧栏「置顶」段优先展示
    #[serde(default)]
    pub pinned_at: Option<u64>,
    /// 收藏夹分类 ID；None = 未收藏。一个会话至多归属一个分类（可移动/移除）
    #[serde(default)]
    pub collection_id: Option<String>,
}

impl AgentThread {
    pub fn new(id: String, title: String, project_id: Option<String>, now_ms: u64) -> Self {
        Self {
            id,
            title,
            status: ThreadStatus::Running,
            project_id,
            latest_run_id: None,
            created_at: now_ms,
            updated_at: now_ms,
            closed_at: None,
            archived_at: None,
            pinned_at: None,
            collection_id: None,
        }
    }
}

/// Thread 列表范围（AG-01 多会话）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadListScope {
    /// 活跃 tab：未关闭、未归档
    Active,
    /// 历史抽屉：已关闭、未归档
    History,
}

/// 收藏夹分类：侧栏「收藏夹」段的组织单元；会话经 collection_id 归入，供后续查阅。
/// serde 口径同 AgentThread：camelCase 对齐前端。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCollection {
    pub id: String,
    pub name: String,
    pub created_at: u64,
}

/// Run 状态（§十一：queued/running/waiting_approval/completed/failed/cancelled/interrupted）
/// serde 口径同 ThreadStatus：snake_case 小写对齐前端，alias 兼容 dev 库旧 PascalCase。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    #[serde(alias = "Queued")]
    Queued,
    #[serde(alias = "Running")]
    Running,
    #[serde(alias = "WaitingApproval")]
    WaitingApproval,
    #[serde(alias = "Completed")]
    Completed,
    #[serde(alias = "Failed")]
    Failed,
    #[serde(alias = "Cancelled")]
    Cancelled,
    #[serde(alias = "Interrupted")]
    Interrupted,
}

/// Run：一次 Agent 执行的完整上下文。
/// serde 口径（AG-15 契约收口）：camelCase 对齐前端 agentStore.ts AgentRun。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    pub thread_id: String,
    /// 关联项目 ID（可为空，与 Thread.project_id 正交——Run 也可不归属项目）
    pub project_id: Option<String>,
    /// 运行时状态
    pub status: RunStatus,
    /// 使用的提供者（如 "openai_compat"）
    pub provider: String,
    /// 使用的模型
    pub model: String,
    /// 系统提示版本
    pub prompt_version: Option<String>,
    /// 最大模型调用预算（来自 SpikeParams.max_turns）
    pub max_model_calls: usize,
    /// 当前已完成模型调用数
    pub current_model_calls: usize,
    /// 引擎标识（rig）+ 引擎版本，checkpoint 追溯用（硬性限制②）
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub engine_version: String,
    pub created_at: u64,
    pub updated_at: u64,
}

impl AgentRun {
    pub fn new(
        id: String,
        thread_id: String,
        project_id: Option<String>,
        provider: String,
        model: String,
        max_model_calls: usize,
        now_ms: u64,
    ) -> Self {
        Self {
            id,
            thread_id,
            project_id,
            status: RunStatus::Queued,
            provider,
            model,
            prompt_version: None,
            max_model_calls,
            current_model_calls: 0,
            engine: crate::agent::ENGINE.into(),
            engine_version: crate::agent::ENGINE_VERSION.into(),
            created_at: now_ms,
            updated_at: now_ms,
        }
    }
}

/// AgentMessage：线程中的一条消息（User 输入 / Assistant 最终回答）。
/// serde 口径（AG-15 契约收口）：camelCase 对齐前端（threadId/runId/createdAt）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    pub id: String,
    pub thread_id: String,
    pub run_id: String,
    pub role: String, // "user" | "assistant"
    pub content: String,
    /// 来源标记（spike / phase2 / skill → future）
    #[serde(default)]
    pub source: String,
    pub created_at: u64,
}

impl AgentMessage {
    pub fn new(
        id: String,
        thread_id: String,
        run_id: String,
        role: impl Into<String>,
        content: impl Into<String>,
        now_ms: u64,
    ) -> Self {
        Self {
            id,
            thread_id,
            run_id,
            role: role.into(),
            content: content.into(),
            source: "spike".into(),
            created_at: now_ms,
        }
    }
}

/// AgentToolCall：单次工具调用的持久化元数据。
/// serde 口径（AG-15 契约收口）：camelCase（toolCallId/toolName/argumentsJson 等）。
/// AG-21 扩列：五件套中 structured/uiArtifact/provenance/truncated 持久化
/// （serde(default) —— AG-21 前落的 Snapshot JSON 无这些字段，反序列化兜底）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolCall {
    pub id: String,
    pub run_id: String,
    /// 与 rig PendingToolCall.tool_call.id 对齐
    pub tool_call_id: String,
    pub tool_name: String,
    /// JSON 参数对象
    pub arguments_json: Option<String>,
    /// 成功时的结果文本
    pub result_text: Option<String>,
    /// 失败时的错误信息
    pub error_text: Option<String>,
    /// 是否被 preresolved（跳过执行的同批兄弟调用）
    pub preresolved: bool,
    pub created_at: u64,
    /// AG-21：结构化结果 JSON（UI 渲染主通道）
    #[serde(default)]
    pub structured_json: Option<String>,
    /// AG-21：UiArtifact 包络 JSON（kind 已过 Rust 侧 allowlist）
    #[serde(default)]
    pub ui_artifact_json: Option<String>,
    /// AG-21：来源引用 JSON 数组（ProvenanceRef[]）
    #[serde(default)]
    pub provenance_json: Option<String>,
    /// AG-21：大结果截断标记
    #[serde(default)]
    pub truncated: bool,
}

impl AgentToolCall {
    pub fn new(
        id: String,
        run_id: String,
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments_json: Option<String>,
        now_ms: u64,
    ) -> Self {
        Self {
            id,
            run_id,
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            arguments_json,
            result_text: None,
            error_text: None,
            preresolved: false,
            created_at: now_ms,
            structured_json: None,
            ui_artifact_json: None,
            provenance_json: None,
            truncated: false,
        }
    }
}

/// AgentApproval：审批请求（Phase 3 落实现实审批流时填充）。
/// serde 口径（AG-15 契约收口）：camelCase（approvalType/resourceSummary/resolvedAt）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentApproval {
    pub id: String,
    pub run_id: String,
    pub approval_type: String, // e.g. "document_write"
    pub status: String,        // "pending" | "approved" | "rejected"
    pub resource_summary: String,
    pub created_at: u64,
    pub resolved_at: Option<u64>,
}

/// AgentRunEvent：单轮 AgentEvent 的持久化条目。
/// 序列化 payload 为 json blob 存于 data 列（agents.rs 已有 AgentEvent 和 EVENT_SCHEMA_VERSION，
/// 此处只存 envelope 字段 + 序列化的 payload JSON）。
/// serde 口径（AG-15 契约收口）：camelCase 与数据域其余类型统一。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunEvent {
    pub event_id: String,
    pub thread_id: String,
    pub run_id: String,
    pub seq: u64,
    pub timestamp: u64,
    pub schema_version: u32,
    pub data: String, // serialized AgentEvent JSON blob
}

impl AgentRunEvent {
    pub fn from_agent_event(event: &crate::agent::events::AgentEvent, json: &str) -> Self {
        Self {
            event_id: event.event_id.clone(),
            thread_id: event.thread_id.clone(),
            run_id: event.run_id.clone(),
            seq: event.seq,
            timestamp: event.timestamp,
            schema_version: event.schema_version,
            data: json.into(),
        }
    }

    /// 从事件重建 AgentEvent（需反序列化回 json::Value）
    pub fn to_json(&self) -> &str {
        &self.data
    }
}

#[cfg(test)]
mod serde_contract_tests {
    use super::*;

    /// AG-15 契约钉死：AgentThread 输出 camelCase、status 为 snake_case 小写——
    /// 与前端 agentStore.ts 的 AgentThread/ThreadStatus 接口逐字段对应。
    /// 防回退断言同 AG-07 口径（snake_case 字段不得泄漏）。
    #[test]
    fn thread_serde_camel_case_contract() {
        let thread = AgentThread::new(
            "t-1".into(),
            "测试".into(),
            Some("p-1".into()),
            1_700_000_000_000,
        );
        let v = serde_json::to_value(&thread).expect("serialize");
        assert_eq!(v["projectId"], "p-1");
        assert_eq!(v["latestRunId"], serde_json::Value::Null);
        assert_eq!(v["createdAt"], 1_700_000_000_000u64);
        assert_eq!(v["updatedAt"], 1_700_000_000_000u64);
        assert_eq!(v["status"], "running");
        assert!(v.get("project_id").is_none());
        assert!(v.get("latest_run_id").is_none());
        assert!(v.get("created_at").is_none());
    }

    /// Run/Thread 状态枚举：snake_case 序列化 + PascalCase 旧数据兼容
    /// （AG-15 收口前的 dev 库旧行存的是 `"Running"`，alias 保证读出不出错）
    #[test]
    fn status_serde_snake_case_and_legacy_alias() {
        assert_eq!(
            serde_json::to_string(&RunStatus::WaitingApproval).unwrap(),
            "\"waiting_approval\""
        );
        assert_eq!(
            serde_json::to_string(&ThreadStatus::Running).unwrap(),
            "\"running\""
        );
        let legacy: ThreadStatus = serde_json::from_str("\"Running\"").unwrap();
        assert_eq!(legacy, ThreadStatus::Running);
        let legacy_run: RunStatus = serde_json::from_str("\"WaitingApproval\"").unwrap();
        assert_eq!(legacy_run, RunStatus::WaitingApproval);
    }

    /// AgentMessage 输出 camelCase（threadId/runId/createdAt）
    #[test]
    fn message_serde_camel_case_contract() {
        let msg = AgentMessage::new(
            "m-1".into(),
            "t-1".into(),
            "r-1".into(),
            "user",
            "hello",
            1_700_000_000_000,
        );
        let v = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(v["threadId"], "t-1");
        assert_eq!(v["runId"], "r-1");
        assert_eq!(v["createdAt"], 1_700_000_000_000u64);
        assert!(v.get("thread_id").is_none());
        assert!(v.get("run_id").is_none());
    }
}
