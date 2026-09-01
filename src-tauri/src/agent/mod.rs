// ============================================================
// Hermes Client Surface（DEC-011 / DEC-020）
// 实施基线：docs/architecture.md §23.1
//
// 产品会话固定使用 Hermes Session/Skill/Memory/Tool/Event 协议；SophoNote 只保留
// Surface 展示、本地索引和项目/文档权限边界。Rig 仅是未注册的历史测试资产。
// ============================================================

/// 回退引擎标识（checkpoint / agent_runs.engine；H8 选型结果可为 hermes）
pub const ENGINE: &str = "rig";

/// 锁定的 Rig 精确版本（与 Cargo.toml 一致；升级须跑回归；H9 删除）
pub const ENGINE_VERSION: &str = "0.41.0";

// Spike 编译门禁：证明依赖解析与 crate 编译通过。
// rig-agent 0.41.0 为 edition 2024（要求 rustc ≥1.85）。
#[allow(unused_imports)]
use rig_agent::agent::run::{AgentRun, AgentRunStep, ModelTurn, ModelTurnOutcome};
#[allow(unused_imports)]
use rig_core::completion::message::{AssistantContent, Message, ToolCall, ToolResult, UserContent};

// Rig ↔ 自有消息纯转换（仅 Rig 路径；H9 删除）
pub mod adapters;
// DEC-014：用户显式附件 → Hermes 多模态/有界上下文适配
pub mod attachments;

// H1：执行平面抽象；Rig 实现该接口（H8 起为回退）
pub mod engine;
// DEC-019：Hermes-only 就绪探测（无产品引擎切换/回退）
pub mod engine_select;

// H2+：Hermes sidecar Supervisor / health Client / 只读 Run（协议 stub）
pub mod hermes;

// Tauri 命令 + Rig 驱动循环（Rig 类型扩散面收敛在 adapters / run_controller）
pub mod commands;
pub mod run_controller;

// 版本化 AgentEvent + EventEmitter（零框架类型）
pub mod events;

// Thread/Run/Message + RunStore（规范真相源）
pub mod store;
pub mod types;

#[cfg(test)]
mod tests {
    use super::*;

    /// 引擎标识与锁定版本登记（未来 checkpoint 元数据的来源）
    #[test]
    fn engine_identity() {
        assert_eq!(ENGINE, "rig");
        assert_eq!(ENGINE_VERSION, "0.41.0");
    }
}
