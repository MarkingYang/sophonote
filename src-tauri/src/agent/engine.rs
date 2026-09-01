//! DEC-011/020：产品执行面固定为 Hermes Client Surface。
//!
//! `RunEnvelope` 当前仍兼容未注册的历史 Rig Spike；产品 Run 只把用户输入、
//! Hermes Session、模型选择和原生附件交给 Gateway，不注入 system/history/
//! Skill 正文/Memory key/SophoNote ToolRegistry。
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::sophonote_mcp::ModelRoute;
use crate::model::gateway::SharedGateway;
use crate::tools::ToolRegistry;

use super::events::EventEmitter;
use super::run_controller::{
    run_spike_with_events, RunControllerError, SpikeParams, SpikeRunReport, ToolCallObserver,
};
use super::{ENGINE, ENGINE_VERSION};

/// 引擎侧错误（不泄漏 Rig / HTTP 框架类型）
#[derive(Debug)]
pub enum EngineError {
    /// 进入循环前的建立失败（如会话转换）
    Setup(String),
    /// 健康检查失败（H2+ sidecar）
    Unhealthy(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(msg) => write!(f, "引擎建立失败: {msg}"),
            Self::Unhealthy(msg) => write!(f, "引擎不可用: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<RunControllerError> for EngineError {
    fn from(err: RunControllerError) -> Self {
        Self::Setup(err.to_string())
    }
}

/// SophoNote 构造的一次 Run 入参。Hermes 产品路径只消费 `hermes_*`、用户原文、
/// 取消和事件通道；其余字段是待物理删除的 Spike 兼容面。
pub struct RunEnvelope {
    pub gateway: SharedGateway,
    pub registry: Arc<ToolRegistry>,
    pub params: SpikeParams,
    pub cancel: CancellationToken,
    pub events: Option<Arc<EventEmitter>>,
    pub observer: Option<Arc<dyn ToolCallObserver>>,
    /// H3：Host 预组装的 search/evidence 等上下文（sidecar 只消费、不扫盘）
    pub context_pack: Option<serde_json::Value>,
    /// H5 / DEC-012：模型路由（来自 SophoNote ai_config；无 API Key）
    pub model_route: Option<ModelRoute>,
    /// SophoNote Thread 绑定的 Hermes Session；Rig 忽略。
    pub hermes_session_id: Option<String>,
    /// Hermes 长期记忆作用域。项目 Chat 使用稳定的 project key，避免跨项目串记忆。
    pub hermes_memory_scope_key: Option<String>,
    /// DEC-014：已由 Host 校验/有界化的 Hermes 输入（字符串或多模态消息数组）。
    pub hermes_input: Option<serde_json::Value>,
    /// DEC-019：用户从当前激活供应商白名单为本 Run 选择的 Hermes 模型。
    pub hermes_model: Option<String>,
    /// 与 `hermes_model` 成对的 Hermes Provider slug；由 Runtime `model.options` 提供。
    pub hermes_provider: Option<String>,
    /// DEC-021：用户在 Composer 输入的 Hermes 原生 `/` 命令。
    /// Attached engine 在同一 Session 上调用 `slash.exec`，不改写成提示词。
    pub hermes_command: Option<String>,
    /// 用户在当前 SophoNote 会话中明确授权的本地工作目录。Hermes Session 以此
    /// 作为 cwd，文件、终端与 Git 工具因此共享同一个真实仓库工作区。
    pub hermes_workspace_root: Option<String>,
    /// Hermes Gateway 原生附件。正式 Surface 不把附件内容拼进 system/user 提示词。
    pub hermes_attachments: Vec<crate::agent::attachments::RunAttachmentInput>,
    /// 用户界面明确显示的当前文档范围。Host 在发送时捕获编辑器草稿，Gateway
    /// 将其上传为 Session Markdown 工作副本；Hermes 只能编辑副本，Host 在终态
    /// 转换成 DocumentService Patch。选区存在时命令层不会设置此字段。
    pub hermes_focus_document: Option<HermesFocusDocument>,
    /// 用户在左侧显式把项目加入会话。Gateway 从 Host 数据库生成只含项目元数据、
    /// 文档清单和有界项目操作区的工作副本；不把项目正文拼入提示词。
    pub hermes_project_context: bool,
    /// 首次发送时由 Gateway 创建持久 Session，并把 1:1 映射写回 SophoNote。
    pub hermes_session_binding: Option<HermesSessionBinding>,
}

/// 当前文档的发送时快照。它只在 Host→Gateway 适配层短暂存在，不写入
/// `run_started` 正文，也不等价于文档写权限。
#[derive(Debug, Clone)]
pub struct HermesFocusDocument {
    pub article_id: String,
    pub title: String,
    pub base_version: i64,
    pub markdown: String,
}

/// Gateway Session 的本地绑定信息。它只用于 Host 持久化，不进入模型上下文。
#[derive(Debug, Clone)]
pub struct HermesSessionBinding {
    pub db_path: std::path::PathBuf,
    pub notes_dir: std::path::PathBuf,
    /// 项目工作室有值；笔记本为 None。只供 Host 生成领域操作提案，
    /// 不进入 Hermes Session 或模型上下文。
    pub project_id: Option<String>,
    pub thread_id: String,
    pub run_id: String,
}

/// 执行平面稳定接口（架构 §23.1.6；H1 以现网驱动签名为锚）
pub trait AgentEngine: Send + Sync {
    fn engine_id(&self) -> &'static str;
    fn engine_version(&self) -> &'static str;

    /// 就绪探测；产品 Hermes 校验 Gateway 配置，历史 Rig Spike 恒 Ok。
    fn health(&self) -> Result<(), EngineError>;

    /// 驱动一次 Run（事件 / 观察者可选）；终态收敛进 SpikeRunReport
    fn run_with_events(
        &self,
        envelope: RunEnvelope,
    ) -> impl std::future::Future<Output = Result<SpikeRunReport, EngineError>> + Send;
}

/// 未注册的历史 Spike/单测实现；不属于产品回退路径。
#[derive(Debug, Default, Clone, Copy)]
pub struct RigAgentEngine;

impl AgentEngine for RigAgentEngine {
    fn engine_id(&self) -> &'static str {
        ENGINE
    }

    fn engine_version(&self) -> &'static str {
        ENGINE_VERSION
    }

    fn health(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn run_with_events(&self, envelope: RunEnvelope) -> Result<SpikeRunReport, EngineError> {
        let _ = envelope.model_route; // Rig 仍走 gateway；route 供审计/Hermes
        let _ = envelope.context_pack;
        let _ = envelope.hermes_session_id;
        let _ = envelope.hermes_memory_scope_key;
        let _ = envelope.hermes_input;
        let _ = envelope.hermes_model;
        let _ = envelope.hermes_provider;
        let _ = envelope.hermes_attachments;
        let _ = envelope.hermes_focus_document;
        let _ = envelope.hermes_session_binding;
        run_spike_with_events(
            envelope.gateway,
            envelope.registry,
            envelope.params,
            envelope.cancel,
            envelope.events,
            envelope.observer,
        )
        .await
        .map_err(EngineError::from)
    }
}

/// 历史 Spike/对照测试专用。DEC-019 后不得由产品 Run 或设置路径调用。
pub fn legacy_spike_engine() -> RigAgentEngine {
    RigAgentEngine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_engine_identity_and_health() {
        let eng = RigAgentEngine;
        assert_eq!(eng.engine_id(), "rig");
        assert_eq!(eng.engine_version(), "0.41.0");
        assert!(eng.health().is_ok());
    }

    #[test]
    fn legacy_spike_engine_is_rig_test_impl() {
        let eng = legacy_spike_engine();
        assert_eq!(eng.engine_id(), ENGINE);
    }
}
