//! H5 / NEXT-022 / DEC-012：SophoNote MCP Bridge + SidecarLease。
//!
//! Hermes 唯一工具入口；校验 Lease；不信任模型传来的 projectId/runId。
//! 模型路由只来自 SophoNote settings（无第二套密钥配置）。

pub mod http_server;
pub mod lease;
pub mod policy;
pub mod server;
pub mod tools;

pub use http_server::{bridge_runtime, ensure_bridge_http, BridgeHttpRuntime};
pub use lease::{issue_lease, LeaseError, LeaseRegistry, ModelRoute, SidecarLease};
pub use policy::authorize_tool;
pub use server::{bridge_patch_registry, SophonoteBridge, BRIDGE_PATCH_TOOL};
pub use tools::{BridgeInvokeRequest, BridgeInvokeResult};

/// Bridge 对外逻辑名（Hermes 只挂这一个 MCP）
pub const BRIDGE_MCP_NAME: &str = "sophonote-bridge";

/// 文档工具继续要求逐 Run Lease；发现工具是固定来源或现存 itemId 的 Host
/// 能力，只允许包内 Hermes 经环回 Bearer 调用，不开放路径/URL/命令参数。
pub const BRIDGE_TOOL_NAMES: &[&str] = &[
    "list_project_documents",
    "read_document",
    "propose_document_patch",
    "rename_article",
    "refresh_discovery_sources",
    "list_discovery_candidates",
    "read_discovery_item",
    "save_discovery_analysis",
    "save_discovery_pick",
    "save_discovery_scores",
    "read_discovery_feed",
    "save_discovery_report",
    "refresh_openrouter_rankings",
    "read_openrouter_rankings",
];
