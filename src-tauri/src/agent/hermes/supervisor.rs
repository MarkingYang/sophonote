//! Hermes sidecar Supervisor：校验哈希、环回监听、短 Token、空 cwd、崩溃隔离。

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use uuid::Uuid;

use super::client::{HermesClientError, HermesHttpClient};
use super::config::{verify_binary_hash, HermesSidecarConfig};

#[derive(Debug)]
pub enum HermesSupervisorError {
    Config(String),
    Spawn(String),
    Health(String),
    Io(String),
}

impl std::fmt::Display for HermesSupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(m) => write!(f, "Hermes 配置错误: {m}"),
            Self::Spawn(m) => write!(f, "Hermes 拉起失败: {m}"),
            Self::Health(m) => write!(f, "Hermes 健康检查失败: {m}"),
            Self::Io(m) => write!(f, "Hermes IO 错误: {m}"),
        }
    }
}

impl std::error::Error for HermesSupervisorError {}

impl From<HermesClientError> for HermesSupervisorError {
    fn from(err: HermesClientError) -> Self {
        Self::Health(err.to_string())
    }
}

/// 运行中的 sidecar 句柄；Drop 时强制终止子进程
pub struct HermesSidecarHandle {
    child: Child,
    pub port: u16,
    pub bearer: String,
    pub base_url: String,
    /// 空临时工作目录（不持有 notes 路径）
    workdir: PathBuf,
    hermes_home: PathBuf,
    client: HermesHttpClient,
}

impl HermesSidecarHandle {
    pub fn client(&self) -> &HermesHttpClient {
        &self.client
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn hermes_home(&self) -> &Path {
        &self.hermes_home
    }

    pub async fn health_detailed_ok(&self) -> Result<(), HermesSupervisorError> {
        let detailed = self.client.health_detailed().await?;
        if !detailed.is_ready() {
            return Err(HermesSupervisorError::Health(format!(
                "readiness status={}",
                detailed.status
            )));
        }
        Ok(())
    }

    /// 显式关闭（与 Drop 同效）
    pub fn shutdown(mut self) -> Result<(), HermesSupervisorError> {
        self.kill_child()
    }

    fn kill_child(&mut self) -> Result<(), HermesSupervisorError> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for HermesSidecarHandle {
    fn drop(&mut self) {
        let _ = self.kill_child();
        // 清理空 cwd（忽略失败，避免 Drop panic）
        let _ = std::fs::remove_dir_all(&self.workdir);
    }
}

/// 选择 `127.0.0.1` 上空闲端口
pub fn pick_loopback_port() -> Result<u16, HermesSupervisorError> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| HermesSupervisorError::Io(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| HermesSupervisorError::Io(e.to_string()))?
        .port();
    drop(listener);
    Ok(port)
}

fn random_bearer() -> String {
    // ≥32 hex：两个 UUID 拼接
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// 拉起 sidecar：哈希校验 → 空 cwd → 环回+Token → 轮询 `/health`
pub async fn start_sidecar(
    config: &HermesSidecarConfig,
) -> Result<HermesSidecarHandle, HermesSupervisorError> {
    verify_binary_hash(&config.binary_path, &config.expected_sha256)
        .map_err(HermesSupervisorError::Config)?;

    // hermes_home 不得落在 notes 路径内（防御性检查）
    let home_str = config.hermes_home.to_string_lossy();
    if home_str.contains("/notes") || home_str.contains("notes/") {
        return Err(HermesSupervisorError::Config(
            "HERMES_HOME 不得位于 notes 目录".into(),
        ));
    }

    std::fs::create_dir_all(&config.hermes_home)
        .map_err(|e| HermesSupervisorError::Io(format!("创建 HERMES_HOME: {e}")))?;

    let workdir = config
        .hermes_home
        .join(format!("cwd-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&workdir)
        .map_err(|e| HermesSupervisorError::Io(format!("创建 sidecar cwd: {e}")))?;

    let port = pick_loopback_port()?;
    let bearer = random_bearer();
    let base_url = format!("http://127.0.0.1:{port}");

    let mut cmd = Command::new(&config.binary_path);
    cmd.current_dir(&workdir)
        .env("API_SERVER_ENABLED", "true")
        .env("API_SERVER_HOST", "127.0.0.1")
        .env("API_SERVER_PORT", port.to_string())
        .env("API_SERVER_KEY", &bearer)
        .env("HERMES_HOME", &config.hermes_home)
        .env_remove("API_SERVER_CORS_ORIGINS")
        .env_remove("HERMES_STUB_DROP_AFTER")
        .env_remove("HERMES_STUB_IDLE_CLOSE")
        // 不向子进程传递 notes 路径类变量
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in &config.stub_env {
        cmd.env(k, v);
    }

    let child = cmd
        .spawn()
        .map_err(|e| HermesSupervisorError::Spawn(e.to_string()))?;

    let client = HermesHttpClient::new(&base_url, &bearer);
    let mut handle = HermesSidecarHandle {
        child,
        port,
        bearer,
        base_url,
        workdir,
        hermes_home: config.hermes_home.clone(),
        client,
    };

    // 轮询 liveness（最长 ~3s）
    let mut last_err = String::from("未开始探测");
    for _ in 0..30 {
        match handle.client.health().await {
            Ok(h) if h.status.eq_ignore_ascii_case("ok") => {
                // detailed 就绪
                handle.health_detailed_ok().await?;
                return Ok(handle);
            }
            Ok(h) => last_err = format!("liveness status={}", h.status),
            Err(e) => last_err = e.to_string(),
        }
        // 子进程若已退出则早失败
        match handle.child.try_wait() {
            Ok(Some(status)) => {
                let _ = handle.kill_child();
                return Err(HermesSupervisorError::Spawn(format!(
                    "sidecar 提前退出: {status}; last_err={last_err}"
                )));
            }
            Ok(None) => {}
            Err(e) => {
                let _ = handle.kill_child();
                return Err(HermesSupervisorError::Spawn(e.to_string()));
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = handle.kill_child();
    Err(HermesSupervisorError::Health(format!(
        "超时未就绪: {last_err}"
    )))
}
