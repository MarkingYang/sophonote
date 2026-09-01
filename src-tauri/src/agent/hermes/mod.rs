//! DEC-011/020：Hermes 正式 Client Surface 适配层。
//!
//! 产品会话只连接 `hermes serve` JSON-RPC/WebSocket Gateway；旧 HTTP Runs、
//! Supervisor 和只读引擎保留为迁移测试资产，不参与产品执行与回退。

pub mod attached_engine;
pub mod bridge_mount;
pub mod bundled_runtime;
pub mod client;
pub mod config;
pub mod event_mapper;
pub mod gateway_client;
pub mod health_supervisor;
pub mod local_proxy;
pub mod readonly_engine;
pub mod recovery;
pub mod runs_client;
pub mod session_surface;
pub mod sidecar_update;
pub mod supervisor;
pub mod user_facing;

/// Hermes 产品引擎元数据标识。
pub const ENGINE_ID: &str = "hermes";

/// 当前正式 Surface 协议版本。
pub const STUB_PROTOCOL_VERSION: &str = "gateway-jsonrpc-v1";

pub use attached_engine::{attached_env_configured, AttachedHermesEngine};
pub use bridge_mount::{
    clear_bundled_home, configure_sophonote_surface, hermes_home, install_bundled_home,
    ENV_HERMES_HOME,
};
pub use client::{DetailedHealth, HealthStatus, HermesClientError, HermesHttpClient};
pub use config::{file_sha256_hex, HermesSidecarConfig};
pub use gateway_client::{
    gateway_env_configured, HermesGatewayEndpoint, ENV_GATEWAY_TOKEN, ENV_GATEWAY_URL,
};
pub use readonly_engine::{assert_readonly_registry, HermesReadonlyEngine, READONLY_TOOL_NAMES};
pub use recovery::{consume_with_recovery, RecoveryOutcome, MAX_SSE_RECONNECTS};
pub use runs_client::{HermesModel, HermesRunsClient, HermesSession, SseStreamResult};
pub use supervisor::{start_sidecar, HermesSidecarHandle, HermesSupervisorError};
