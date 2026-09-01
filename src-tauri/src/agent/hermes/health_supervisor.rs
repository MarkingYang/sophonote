//! Hermes Health Supervisor：运行时保活、自动重连、状态推送。
//!
//! 启动后后台轮询 Hermes /api/health，连续失败时自动重启 bundled runtime，
//! 并通过 Tauri Event `sophonote:hermes-status-changed` 向前端推送连接状态变化。
//!
//! 重启克制（ISSUE-037/041 教训）：137（SIGKILL）下每一次自动重启都会进一步
//! 挤占资源，若无上限会变成「越重启越坏」的死循环（037 为内存压力，041 为
//! provenance 标记导致 exec 即杀、重试完全无效）。因此自动重启带单会话预算
//! 与指数退避；预算耗尽后只轮询、不再拉起进程，把恢复交给手动
//! `restart_hermes_runtime`（成功即重置预算）。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::AppState;

/// 健康检查间隔（秒）
const HEALTH_INTERVAL_SECS: u64 = 10;
/// 连续失败阈值（触发重启）
const FAILURE_THRESHOLD: u32 = 3;
/// 单会话自动重启预算：耗尽后停止自动重启，避免在内存压力下放大故障
const MAX_AUTO_RESTARTS: u32 = 3;
/// 重启退避基数（秒）：随已用预算指数增长 30s → 60s → 120s …，封顶 240s
const RESTART_BASE_COOLDOWN_SECS: u64 = 30;
const RESTART_MAX_COOLDOWN_SECS: u64 = 240;
/// HTTP 健康检查超时（秒）
const HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Hermes 连接状态（前端对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HermesHealthStatus {
    Connected,
    Disconnected,
    Restarting,
}

impl HermesHealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Restarting => "restarting",
        }
    }
}

/// Health Supervisor 句柄。
pub struct HermesHealthSupervisor {
    cancel: CancellationToken,
    status: Arc<Mutex<HermesHealthStatus>>,
    budget_used: Arc<AtomicU32>,
}

impl HermesHealthSupervisor {
    /// 启动后台轮询循环。初始状态按 Disconnected 处理，首次检查通过才转 Connected，
    /// 避免在 Runtime 尚未就绪时向前端谎报已连接。
    pub fn start(app: AppHandle) -> Self {
        let cancel = CancellationToken::new();
        let status = Arc::new(Mutex::new(HermesHealthStatus::Disconnected));
        let budget_used = Arc::new(AtomicU32::new(0));

        tauri::async_runtime::spawn(run_supervisor_loop(
            app,
            status.clone(),
            budget_used.clone(),
            cancel.clone(),
        ));

        Self {
            cancel,
            status,
            budget_used,
        }
    }

    /// 当前状态快照。
    pub async fn current_status(&self) -> HermesHealthStatus {
        *self.status.lock().await
    }

    /// 已使用的自动重启预算。
    pub fn restart_budget_used(&self) -> u32 {
        self.budget_used.load(Ordering::SeqCst)
    }

    /// 手动重启成功后调用：清零自动重启预算，让监督器恢复自动兜底能力。
    pub fn reset_restart_budget(&self) {
        self.budget_used.store(0, Ordering::SeqCst);
    }

    /// 停止轮询（应用退出时调用）。
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

/// 指数退避冷却：已用预算越多，下一次自动重启等待越久（30s/60s/120s/240s 封顶）。
fn backoff_secs(restarts_used: u32) -> u64 {
    let shift = restarts_used.min(5);
    let secs = RESTART_BASE_COOLDOWN_SECS.saturating_mul(1u64 << shift);
    secs.min(RESTART_MAX_COOLDOWN_SECS)
}

/// 状态变化时写入并推送事件。返回是否发生了实际变化（供调用方打恢复日志）。
async fn set_status_and_emit(
    app: &AppHandle,
    status: &Mutex<HermesHealthStatus>,
    next: HermesHealthStatus,
) -> bool {
    let mut guard = status.lock().await;
    if *guard == next {
        return false;
    }
    *guard = next;
    drop(guard);
    let _ = app.emit("sophonote:hermes-status-changed", next.as_str());
    true
}

async fn run_supervisor_loop(
    app: AppHandle,
    status: Arc<Mutex<HermesHealthStatus>>,
    budget_used: Arc<AtomicU32>,
    cancel: CancellationToken,
) {
    let mut consecutive_failures = 0u32;
    let mut last_restart_attempt: Option<std::time::Instant> = None;
    let mut budget_exhausted_logged = false;

    // 启动后立即执行一次健康检查，快速确认状态
    match perform_health_check(&app).await {
        Ok(()) => {
            eprintln!("[hermes-health] initial check passed");
            set_status_and_emit(&app, &status, HermesHealthStatus::Connected).await;
        }
        Err(error) => {
            consecutive_failures = 1;
            eprintln!("[hermes-health] initial check failed: {error}");
        }
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                eprintln!("[hermes-health] supervisor loop stopped");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(HEALTH_INTERVAL_SECS)) => {
                match perform_health_check(&app).await {
                    Ok(()) => {
                        let was_degraded = consecutive_failures > 0
                            || *status.lock().await != HermesHealthStatus::Connected;
                        consecutive_failures = 0;
                        budget_exhausted_logged = false;
                        if set_status_and_emit(&app, &status, HermesHealthStatus::Connected).await
                            && was_degraded
                        {
                            eprintln!("[hermes-health] reconnected");
                        }
                    }
                    Err(ref error) => {
                        consecutive_failures += 1;
                        eprintln!(
                            "[hermes-health] check failed ({consecutive_failures}/{FAILURE_THRESHOLD}): {error}"
                        );
                        set_status_and_emit(&app, &status, HermesHealthStatus::Disconnected).await;

                        if consecutive_failures < FAILURE_THRESHOLD {
                            continue;
                        }

                        let used = budget_used.load(Ordering::SeqCst);
                        if used >= MAX_AUTO_RESTARTS {
                            if !budget_exhausted_logged {
                                budget_exhausted_logged = true;
                                eprintln!(
                                    "[hermes-health] auto-restart budget exhausted ({MAX_AUTO_RESTARTS}/{MAX_AUTO_RESTARTS}); \
                                     不再自动拉起进程，等待前端「重启 Hermes」手动恢复。若反复 137，请检查宿主孤儿进程：\
                                     pgrep -fl hermes_cli"
                                );
                            }
                            continue;
                        }

                        let cooldown = backoff_secs(used);
                        let can_restart = last_restart_attempt
                            .map(|t| t.elapsed().as_secs() >= cooldown)
                            .unwrap_or(true);
                        if !can_restart {
                            continue;
                        }

                        budget_used.fetch_add(1, Ordering::SeqCst);
                        last_restart_attempt = Some(std::time::Instant::now());
                        set_status_and_emit(&app, &status, HermesHealthStatus::Restarting).await;
                        eprintln!(
                            "[hermes-health] initiating restart ({}/{MAX_AUTO_RESTARTS})...",
                            used + 1
                        );

                        match crate::restart_bundled_hermes(&app).await {
                            Ok(()) => {
                                eprintln!("[hermes-health] restart succeeded");
                                consecutive_failures = 0;
                                set_status_and_emit(
                                    &app,
                                    &status,
                                    HermesHealthStatus::Connected,
                                )
                                .await;
                            }
                            Err(ref error) => {
                                eprintln!("[hermes-health] restart failed: {error}");
                                set_status_and_emit(
                                    &app,
                                    &status,
                                    HermesHealthStatus::Disconnected,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 执行一次健康检查：先确认 endpoint 配置，再检查进程存活，最后 HTTP GET /api/health。
async fn perform_health_check(app: &AppHandle) -> Result<(), String> {
    // 1. endpoint 是否已配置
    let endpoint = crate::agent::hermes::HermesGatewayEndpoint::from_env()
        .ok_or_else(|| "endpoint not configured".to_string())?;

    // 2. 进程是否还活着
    {
        let state = app.state::<AppState>();
        let mut runtime_guard = state.hermes.lock().await;
        if let Some(runtime) = runtime_guard.as_mut() {
            if !runtime.is_alive() {
                return Err("sidecar process exited".into());
            }
        } else {
            return Err("no runtime instance".into());
        }
    }

    // 3. HTTP /api/health
    let base = endpoint.dashboard_base_url().map_err(|e| e.to_string())?;
    let health_url = format!("{}api/health", base);

    // Sidecar 永远监听 loopback；系统代理不得截获健康检查（同 bundled_runtime 口径）。
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(&health_url)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP status: {}", response.status()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        assert_eq!(backoff_secs(0), 30);
        assert_eq!(backoff_secs(1), 60);
        assert_eq!(backoff_secs(2), 120);
        assert_eq!(backoff_secs(3), 240);
        assert_eq!(backoff_secs(10), RESTART_MAX_COOLDOWN_SECS);
    }
}
