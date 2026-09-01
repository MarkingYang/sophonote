//! DEC-019：产品 Agent 固定 Hermes；未就绪时明确失败，不存在 Rig 回退。

use std::sync::Mutex;

use sophonote_lib::agent::engine_select::{
    is_engine_unavailable, probe_hermes_production_health, resolve_engine, EngineChoice,
    EngineResolve,
};

// 两个健康探测测试会修改同一组进程环境变量；串行避免并发假失败。
static HERMES_ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn ready_runtime_always_resolves_to_hermes() {
    assert_eq!(
        resolve_engine(true),
        EngineResolve::Use(EngineChoice::Hermes)
    );
}

#[test]
fn hermes_not_ready_is_explicitly_unavailable() {
    let resolve = resolve_engine(false);
    assert!(matches!(resolve, EngineResolve::Unavailable { .. }));
    assert!(is_engine_unavailable(&resolve));
}

#[test]
fn production_probe_false_without_attached_or_pinned_bin() {
    let _guard = HERMES_ENV_LOCK.lock().unwrap();
    std::env::remove_var("SOPHONOTE_HERMES_BIN");
    std::env::remove_var("SOPHONOTE_HERMES_GATEWAY_URL");
    std::env::remove_var("SOPHONOTE_HERMES_GATEWAY_TOKEN");
    assert!(!probe_hermes_production_health());
}

#[test]
fn production_probe_true_when_gateway_env_set() {
    let _guard = HERMES_ENV_LOCK.lock().unwrap();
    std::env::set_var("SOPHONOTE_HERMES_GATEWAY_URL", "ws://127.0.0.1:19119/api/ws");
    std::env::set_var(
        "SOPHONOTE_HERMES_GATEWAY_TOKEN",
        "test-token-not-real-but-long-enough",
    );
    assert!(probe_hermes_production_health());
    std::env::remove_var("SOPHONOTE_HERMES_GATEWAY_URL");
    std::env::remove_var("SOPHONOTE_HERMES_GATEWAY_TOKEN");
}
