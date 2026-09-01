//! Release Hermes Runtime：从 Tauri resources 解析、校验并监督包内 sidecar。
//!
//! Release 不读取 PATH、`~/.hermes` 或 `SOPHONOTE_HERMES_GATEWAY_*`。Debug 在显式
//! 配置外部 Gateway 时跳过本模块；没有显式附着时可使用本地生成的 bundle 做 D3 验证。

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use super::gateway_client::HermesGatewayEndpoint;

const MANIFEST_SCHEMA: u32 = 1;
const STARTUP_ATTEMPTS: usize = 240;
const ENV_ATTACH_EXTERNAL: &str = "SOPHONOTE_HERMES_ATTACH_EXTERNAL";
const GRACEFUL_SHUTDOWN_ATTEMPTS: usize = 60;
const SOPHONOTE_OWNED_SKILLS: &[&str] = &[
    "sophonote-help",
    "sophonote-markdown-writing",
    "sophonote-note-persistence",
    "sophonote-ai-radar",
    "sophonote-openrouter-rankings",
    "archify",
];

#[derive(Debug, Deserialize)]
struct BundleManifest {
    schema_version: u32,
    hermes_version: String,
    hermes_commit: String,
    target: String,
    launcher: String,
    launcher_sha256: String,
    python: String,
    python_sha256: String,
    files_manifest: String,
    files_manifest_sha256: String,
    uv_lock_sha256: String,
}

pub struct BundledHermesRuntime {
    child: Child,
    pub endpoint: HermesGatewayEndpoint,
}

impl BundledHermesRuntime {
    pub fn shutdown(&mut self) {
        terminate_watchdog(&mut self.child);
        HermesGatewayEndpoint::clear_bundled();
        super::bridge_mount::clear_bundled_home();
    }

    /// 检查 sidecar 进程是否仍在运行。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for BundledHermesRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 本进程包内 Runtime 的就绪时刻（Unix 纪元秒）。
/// HERMES_HOME 跨进程共享： started_at 早于该时刻且无终态的 Cron 会话，
/// 其 worker 必随上一进程死亡——上游 is_active 的 300 秒活跃窗会把它
/// 误投影为 running（`agent_hermes_cron_runs` 用本值即时收口为中断）。
static BUNDLED_GATEWAY_BOOT_EPOCH: std::sync::OnceLock<f64> = std::sync::OnceLock::new();

pub fn bundled_gateway_boot_epoch() -> Option<f64> {
    BUNDLED_GATEWAY_BOOT_EPOCH.get().copied()
}

fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn should_use_external_debug_gateway() -> bool {
    cfg!(debug_assertions)
        && std::env::var(ENV_ATTACH_EXTERNAL)
            .ok()
            .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        && std::env::var("SOPHONOTE_HERMES_GATEWAY_URL")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        && std::env::var("SOPHONOTE_HERMES_GATEWAY_TOKEN")
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        && std::env::var(super::bridge_mount::ENV_HERMES_HOME)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
}

/// Debug only：把显式附着的机器 Hermes 配成同一个 SophoNote Surface。
/// 默认开发路径不会调用这里，也不会读写机器 `~/.hermes`。
pub fn configure_external_debug_surface(app: &AppHandle) -> Result<(), String> {
    if !should_use_external_debug_gateway() {
        return Err("未显式启用外部 Hermes 附着".to_string());
    }
    let layout = crate::storage_layout::StorageLayout::resolve(app)?;
    layout.ensure()?;
    let bridge = crate::sophonote_mcp::ensure_bridge_http()?;
    bridge.install_app_handle(app.clone());
    super::bridge_mount::configure_sophonote_surface(
        &bridge.mcp_url(),
        &bridge.bearer,
        &layout.workspace,
    )?;
    // MODEL-11③：免鉴权本地实例写入 config.yaml providers:（幂等）。
    // 失败不阻断启动——云供应商链路不依赖这一步。
    if let Err(error) = super::bridge_mount::sync_local_providers(app) {
        eprintln!("[hermes] sync local providers failed (non-fatal): {error}");
    }
    crate::sophonote_mcp::http_server::sync_discovery_policy_references(app)?;
    Ok(())
}

pub fn bundled_resource_root(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("解析 app resources 失败: {error}"))?;
    let configured_root = resource_dir.join("hermes").join(target_triple());
    // tauri dev 的 resource_dir 指向 target/debug；开发 bundle 尚未复制时允许
    // 回到仓库 resources 做 D3。Release 没有该分支。
    #[cfg(debug_assertions)]
    let resource_root = if configured_root.join("MANIFEST.toml").exists() {
        configured_root
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/hermes")
            .join(target_triple())
    };
    #[cfg(not(debug_assertions))]
    let resource_root = configured_root;
    Ok(resource_root)
}

pub async fn start(app: &AppHandle) -> Result<BundledHermesRuntime, String> {
    for candidate in super::sidecar_update::managed_candidates(app)? {
        match start_from_resource(app, candidate.root.clone()).await {
            Ok(runtime) => {
                if let Err(error) = super::sidecar_update::candidate_started(app, &candidate) {
                    eprintln!("[hermes] failed to promote managed runtime: {error}");
                }
                return Ok(runtime);
            }
            Err(error) => {
                eprintln!(
                    "[hermes] managed runtime {} rejected; falling back: {error}",
                    candidate.root.display()
                );
                super::sidecar_update::candidate_failed(app, &candidate, &error);
            }
        }
    }
    let resource_root = bundled_resource_root(app)?;
    let runtime = start_from_resource(app, resource_root.clone()).await?;
    let manifest = read_bundle_manifest(&resource_root)?;
    if let Err(error) = super::sidecar_update::record_running(
        app,
        &manifest.hermes_version,
        &manifest.hermes_commit,
        "bundled",
    ) {
        eprintln!("[hermes] failed to record bundled runtime: {error}");
    }
    Ok(runtime)
}

fn read_bundle_manifest(resource_root: &Path) -> Result<BundleManifest, String> {
    let manifest_path = resource_root.join("MANIFEST.toml");
    let raw = std::fs::read_to_string(&manifest_path).map_err(|error| {
        format!(
            "包内 Hermes Manifest 不可读 {}: {error}",
            manifest_path.display()
        )
    })?;
    toml::from_str(&raw).map_err(|error| format!("Hermes Manifest 无效: {error}"))
}

async fn start_from_resource(
    app: &AppHandle,
    resource_root: PathBuf,
) -> Result<BundledHermesRuntime, String> {
    let manifest = read_bundle_manifest(&resource_root)?;
    verify_manifest(&resource_root, &manifest)?;
    // ISSUE-041 自愈：完整性校验只保证内容，管不了扩展属性。带 provenance
    // 标记的 Runtime 会在 spawn 前被清理，否则 python 会在 exec 瞬间被杀。
    strip_provenance_xattrs(&resource_root, &manifest.python);

    let layout = crate::storage_layout::StorageLayout::resolve(app)?;
    match crate::storage_layout::StorageLayout::migrate_legacy_hermes_cron(
        app,
        &manifest.hermes_version,
    ) {
        Ok(true) => println!("[storage] 已补拷 MindBox 原计划任务与本地运行历史（旧数据保留）"),
        Ok(false) => {}
        Err(error) => eprintln!("[storage] Hermes Cron 迁移失败: {error}"),
    }
    layout.ensure()?;
    let hermes_home = layout.hermes_home(&manifest.hermes_version);
    let log_dir = &layout.logs;
    std::fs::create_dir_all(&hermes_home).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(log_dir).map_err(|error| error.to_string())?;
    super::bridge_mount::install_bundled_home(hermes_home.clone());
    let bridge = crate::sophonote_mcp::ensure_bridge_http()?;
    bridge.install_app_handle(app.clone());
    super::bridge_mount::configure_sophonote_surface(
        &bridge.mcp_url(),
        &bridge.bearer,
        &layout.workspace,
    )?;
    // MODEL-11③：启动前把设置中的免鉴权本地实例同步进 config.yaml，
    // Runtime 目录才会出现这些供应商（选择器可见、执行可解析）。
    // 非致命：失败只影响本地免鉴权链路，不阻断云供应商与会话启动。
    if let Err(error) = super::bridge_mount::sync_local_providers(app) {
        eprintln!("[hermes] sync local providers failed (non-fatal): {error}");
    }
    // 开发期自有 Skill 直接取仓库源：包内 seed 是构建期 overlay，容易落后于
    // 仓库迭代（曾导致 sidecar 播种融合前旧协议、打分趟缺失落库步骤）。
    // Release 无此分支，信任构建脚本 overlay 后的包内 seed。
    #[cfg(debug_assertions)]
    let owned_override: Option<PathBuf> = {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../skills/hermes/productivity");
        repo.is_dir().then_some(repo)
    };
    #[cfg(not(debug_assertions))]
    let owned_override: Option<PathBuf> = None;
    seed_skills(
        &resource_root.join("seed/skills"),
        &hermes_home.join("skills"),
        owned_override.as_deref(),
    )?;
    crate::sophonote_mcp::http_server::sync_discovery_policy_references(app)?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("分配 Hermes 环回端口失败: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    drop(listener);

    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let launcher = checked_join(&resource_root, &manifest.launcher)?;
    let python = checked_join(&resource_root, &manifest.python)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("hermes-sidecar.log"))
        .map_err(|error| format!("打开 Hermes 日志失败: {error}"))?;
    let stderr = log.try_clone().map_err(|error| error.to_string())?;

    let mut command = hermes_command(&launcher, &python);
    command
        .args([
            "serve",
            "--isolated",
            "--skip-build",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        // Session 默认 cwd 是持久化 workspace；workdir 只保留进程临时状态，
        // shutdown 时可以安全删除而不会误删用户文件。
        .current_dir(&layout.workspace)
        .env("HERMES_HOME", &hermes_home)
        .env("HERMES_DASHBOARD_SESSION_TOKEN", &token)
        .env("HERMES_SERVE_HEADLESS", "1")
        .env("HERMES_DESKTOP", "1")
        .env("SOPHONOTE_HOST_PID", std::process::id().to_string())
        // Hermes 的 HTTP MCP 客户端会读取系统代理环境。若 loopback 没有进入
        // NO_PROXY，请求会被代理截获并返回 502；Agent 随后看不到 Bridge 工具，
        // 便可能退化成 terminal/文件探索。包内 Surface 的两个本地端点都必须
        // 永远直连，同时保留外部模型请求继续使用用户代理的能力。
        .env("NO_PROXY", "127.0.0.1,localhost,::1")
        .env("no_proxy", "127.0.0.1,localhost,::1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("SOPHONOTE_HERMES_GATEWAY_URL")
        .env_remove("SOPHONOTE_HERMES_GATEWAY_TOKEN")
        .env_remove("API_SERVER_CORS_ORIGINS")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    for (name, value) in provider_environment(app)? {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        if let Some(home) = python.parent() {
            command.env("PYTHONHOME", home);
        }
        command.env(
            "PYTHONPATH",
            resource_root.join("runtime").join("site-packages"),
        );
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("拉起包内 Hermes 失败 {}: {error}", launcher.display()))?;
    let health_url = format!("http://127.0.0.1:{port}/api/health");
    let http = reqwest::Client::builder()
        // Sidecar 永远监听 loopback；系统代理不得截获健康检查，否则会把
        // 一个已经就绪的本地 Hermes 误判为 502 并在启动后将其终止。
        .no_proxy()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| error.to_string())?;
    let mut last_error = String::from("尚未收到健康响应");
    for _ in 0..STARTUP_ATTEMPTS {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!(
                "包内 Hermes 提前退出: {status}; {last_error}{}",
                early_exit_hint(status)
            ));
        }
        match http.get(&health_url).send().await {
            Ok(response) if response.status().is_success() => {
                let endpoint =
                    HermesGatewayEndpoint::bundled(format!("ws://127.0.0.1:{port}/api/ws"), token)
                        .map_err(|error| error.to_string())?;
                HermesGatewayEndpoint::install_bundled(endpoint.clone());
                let _ = BUNDLED_GATEWAY_BOOT_EPOCH.set(now_epoch_seconds());
                eprintln!(
                    "[hermes] runtime ready version={} commit={} target={} port={}",
                    manifest.hermes_version, manifest.hermes_commit, manifest.target, port
                );
                return Ok(BundledHermesRuntime { child, endpoint });
            }
            Ok(response) => last_error = format!("health status={}", response.status()),
            Err(error) => last_error = error.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(125)).await;
    }
    terminate_watchdog(&mut child);
    Err(format!("包内 Hermes 启动超时: {last_error}"))
}

/// 启动即退出的可操作诊断。137/143 等是「128 + 信号」的 shell 约定码：
/// 137=SIGKILL 在 macOS 上的头号根因是 com.apple.provenance（见
/// `strip_provenance_xattrs`）——exec 瞬间被 AMFI 杀掉、日志全无；其次才是
/// ISSUE-037 式的宿主内存压力（孤儿 Python 未回收导致 jetsam 杀新进程）。
fn early_exit_hint(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(137) => " | 137=SIGKILL：macOS 会把带 com.apple.provenance 标记（沙箱进程写入）的二进制在 exec 时直接杀掉，且处决态缓存在 inode 上；SophoNote 启动前已自动清标记并把 python 重写到新 inode，若仍复现说明 Runtime 树被沙箱进程重新写入过；其次检查宿主内存压力与孤儿进程（pgrep -fl hermes_cli）".to_string(),
        Some(143) => " | 143=SIGTERM：进程被外部终止；检查是否有另一个 SophoNote 实例或脚本在管理同一 Runtime".to_string(),
        _ => String::new(),
    }
}

/// macOS：沙箱进程（沙箱化的 IDE/Agent 会话等）写入或复制的文件会被系统打上
/// `com.apple.provenance` 扩展属性。带该标记的 linker-signed adhoc 二进制在
/// exec 时会被 AMFI 直接 SIGKILL——进程不执行任何指令，`codesign --verify`
/// 仍然通过，unified log 也不留痕迹，极难定位（ISSUE-041 根因）。
///
/// 更隐蔽的一层：进程一旦被杀，内核会把处决态缓存在对应 inode 上；此后即便
/// 删掉 xattr，同一 inode 再 exec 仍被杀，必须把内容重写到新 inode 才解除
/// （macOS 26.5.2 实测：删属性后仍 137，cat 重写后立即存活）。
///
/// SophoNote 主进程不是沙箱进程，有权删属性与重写文件。策略：先探测 python
/// 二进制是否带标记（稳态零开销），命中时①递归删除整树属性②把 python 二进
/// 制重写到新 inode。失败不阻断启动，最坏退回本轮启动前的行为。
#[cfg(target_os = "macos")]
fn strip_provenance_xattrs(root: &Path, python_relative: &str) {
    const ATTR: &str = "com.apple.provenance";
    let Ok(python_path) = checked_join(root, python_relative) else {
        return;
    };
    let probe = Command::new("xattr")
        .args(["-p", ATTR])
        .arg(&python_path)
        .stdin(Stdio::null())
        .output();
    let present = matches!(probe, Ok(output) if output.status.success());
    if !present {
        return;
    }
    eprintln!(
        "[hermes] detected {ATTR} on {}; scrubbing runtime tree before spawn",
        python_path.display()
    );
    match Command::new("xattr")
        .args(["-dr", ATTR])
        .arg(root)
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => eprintln!("[hermes] provenance strip exit={:?}", output.status.code()),
        Err(error) => eprintln!("[hermes] provenance strip failed: {error}"),
    }
    rewrite_to_fresh_inode(&python_path);
}

#[cfg(not(target_os = "macos"))]
fn strip_provenance_xattrs(_root: &Path, _python_relative: &str) {}

/// 把文件内容重写到新 inode（读字节 → 写临时文件 → 保留权限 → rename 覆盖）。
/// 符号链接先解析到真实目标——内核处决的是被 exec 的真实文件 inode。字节完
/// 全不变，因此 MANIFEST 哈希校验不受影响。不能用 `fs::copy`：macOS 上它会连
/// 扩展属性一起复制，污染会被原样带到新文件。
#[cfg(target_os = "macos")]
fn rewrite_to_fresh_inode(path: &Path) {
    match rewrite_to_fresh_inode_inner(path) {
        Ok(real) => eprintln!("[hermes] rewrote {} onto a fresh inode", real.display()),
        Err(error) => eprintln!(
            "[hermes] fresh-inode rewrite failed for {}: {error}",
            path.display()
        ),
    }
}

#[cfg(target_os = "macos")]
fn rewrite_to_fresh_inode_inner(path: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&real).map_err(|error| error.to_string())?;
    let permissions = std::fs::metadata(&real)
        .map_err(|error| error.to_string())?
        .permissions();
    let tmp = real.with_extension("sophonote-scrub-tmp");
    std::fs::write(&tmp, &bytes).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&tmp, permissions).map_err(|error| error.to_string())?;
    std::fs::rename(&tmp, &real).map_err(|error| error.to_string())?;
    Ok(real)
}

/// launcher 是一个负责回收 Python 的 watchdog shell。`Child::kill()` 在 Unix
/// 上等同 SIGKILL，trap 没机会运行，会把 Python 留成 PPID=1 的孤儿。正常退出
/// 先发可捕获的 TERM 并等待；只有 watchdog 确实不响应时才强制终止。
fn terminate_watchdog(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }

    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for _ in 0..GRACEFUL_SHUTDOWN_ATTEMPTS {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
    }

    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for _ in 0..GRACEFUL_SHUTDOWN_ATTEMPTS {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    let _ = child.kill();
    let _ = child.wait();
}

fn hermes_command(launcher: &Path, python: &Path) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = Command::new(python);
        command.arg(launcher);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        let _ = python;
        Command::new(launcher)
    }
}

fn target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return "aarch64-apple-darwin";
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return "x86_64-apple-darwin";
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return "x86_64-pc-windows-msvc";
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        return "aarch64-pc-windows-msvc";
    }
    #[allow(unreachable_code)]
    "unsupported-target"
}

fn verify_manifest(root: &Path, manifest: &BundleManifest) -> Result<(), String> {
    if manifest.schema_version != MANIFEST_SCHEMA {
        return Err(format!(
            "Hermes Manifest schema 不支持: {}",
            manifest.schema_version
        ));
    }
    if manifest.target != target_triple() {
        return Err(format!(
            "Hermes Runtime 架构不匹配: manifest={} host={}",
            manifest.target,
            target_triple()
        ));
    }
    if manifest.hermes_version.trim().is_empty()
        || manifest.hermes_commit.len() < 12
        || manifest.uv_lock_sha256.len() != 64
    {
        return Err("Hermes Manifest 缺少钉扎版本/commit/uv.lock hash".into());
    }
    verify_file(root, &manifest.launcher, &manifest.launcher_sha256)?;
    verify_file(root, &manifest.python, &manifest.python_sha256)?;
    verify_file(
        root,
        &manifest.files_manifest,
        &manifest.files_manifest_sha256,
    )?;

    // 逐文件清单避免 Runtime 内任意 Python/native 依赖被替换。路径必须保持相对且不能逃逸。
    let files_path = checked_join(root, &manifest.files_manifest)?;
    let reader = BufReader::new(File::open(files_path).map_err(|error| error.to_string())?);
    for line in reader.lines() {
        let line = line.map_err(|error| error.to_string())?;
        let Some((hash, relative)) = line.split_once("  ") else {
            return Err("Hermes FILES.sha256 行格式无效".into());
        };
        if relative == manifest.files_manifest {
            continue;
        }
        verify_file(root, relative, hash)?;
    }
    Ok(())
}

fn verify_file(root: &Path, relative: &str, expected: &str) -> Result<(), String> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("Hermes hash 格式无效: {relative}"));
    }
    let path = checked_join(root, relative)?;
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("读取 Hermes resource {} 失败: {error}", path.display()))?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("Hermes resource hash 不匹配: {relative}"));
    }
    Ok(())
}

fn checked_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("Hermes resource 路径越界: {relative}"));
    }
    Ok(root.join(path))
}

fn seed_skills(
    source: &Path,
    destination: &Path,
    owned_override: Option<&Path>,
) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    archive_superseded_discovery_skill(destination)?;
    copy_missing_tree(source, destination)?;

    // 上游和用户 Skill 只在缺失时播种；SophoNote 自有 Skill 则是客户端协议，
    // 必须随应用版本升级，否则历史私有 Home 会永远运行首次安装的旧逻辑。
    for skill in SOPHONOTE_OWNED_SKILLS {
        // 仓库源优先（开发期），包内 seed 兜底；copy_overwrite 不清理目标多余
        // 文件，运行时生成的 references/source-policies 得以保留。
        let skill_source = owned_override
            .map(|root| root.join(skill))
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| source.join("productivity").join(skill));
        if skill_source.is_dir() {
            copy_overwrite_tree(&skill_source, &destination.join("productivity").join(skill))?;
        }
    }
    Ok(())
}

/// 融合后的 `sophonote-ai-radar` 是唯一发现 Skill。旧目录若仍在 Hermes 私有
/// Home，会继续参与自然语言路由；移入可恢复备份，并阻止上游 seed 再播种。
fn archive_superseded_discovery_skill(destination: &Path) -> Result<(), String> {
    let legacy = destination.join("productivity/sophonote-discovery-subscriptions");
    if !legacy.is_dir() {
        return Ok(());
    }
    let home = destination
        .parent()
        .ok_or_else(|| "Hermes skills 目录缺少父目录".to_string())?;
    let backup_root = home.join("sophonote-skill-backups");
    std::fs::create_dir_all(&backup_root).map_err(|error| error.to_string())?;
    let backup = backup_root.join(format!(
        "sophonote-discovery-subscriptions-{}",
        Uuid::new_v4()
    ));
    std::fs::rename(legacy, backup).map_err(|error| error.to_string())
}

fn copy_missing_tree(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if destination.ends_with("productivity")
            && entry.file_name() == "sophonote-discovery-subscriptions"
        {
            continue;
        }
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            std::fs::create_dir_all(&destination_path).map_err(|error| error.to_string())?;
            copy_missing_tree(&source_path, &destination_path)?;
        } else if !destination_path.exists() {
            std::fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn copy_overwrite_tree(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_overwrite_tree(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn provider_environment(app: &AppHandle) -> Result<BTreeMap<String, String>, String> {
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| error.to_string())?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ai_config'",
            [],
            |row| row.get(0),
        )
        .ok();
    let parsed: serde_json::Value = raw
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    let default_snapshot = crate::model::openai_compat::resolve_provider(app, None)
        .map_err(|error| error.to_string())?;
    let default_provider = (
        default_snapshot.id.clone(),
        default_snapshot.base_url.clone(),
        default_snapshot.requires_key,
    );
    let configured = provider_entries(&parsed, default_provider);

    let mut environment = BTreeMap::new();
    // MODEL-11③：免鉴权本地实例的占位凭据（非机密）。config.yaml 里这些条目
    // 引用 SOPHONOTE_LOCAL_API_KEY，Runtime 见到非空 Key 才视为已配置；本地端点
    // 不校验该头（与 Hermes 自带 LM Studio 免鉴权占位同一做法）。
    if configured.iter().any(|entry| !entry.2) {
        environment.insert(
            super::bridge_mount::LOCAL_PLACEHOLDER_KEY_ENV.into(),
            "sophonote-local".into(),
        );
    }
    for (provider, base_url, requires_key) in configured {
        let mut key = String::new();
        // 免鉴权实例命中安全网改用真实凭据时置 true：基址不再改写为剥鉴权代理
        //（真实 Bearer 不能被剥离），直接连真实端点。
        let mut keyless_override_to_authed = false;
        if requires_key {
            key = match crate::commands::get_cached_api_key(app, &provider) {
                Ok(key) => key,
                Err(error) => {
                    // 未签名 Debug 二进制可能没有既有 Keychain ACL。不要让一次旧明文
                    // 迁移失败阻断整个随包 Runtime；只读使用仍存在的旧值启动本轮，
                    // 设置页继续保留明确错误，直到用户重新保存完成安全迁移。
                    eprintln!(
                        "[hermes] provider={provider} Keychain unavailable; using retained legacy credential for this process: {error}"
                    );
                    crate::commands::get_legacy_api_key(app, &provider)?
                }
            };
        }
        if key.is_empty() && !requires_key {
            // 免鉴权语义只对本地/私有网络端点成立。若端点是云上（非本地）而设置
            // 里又存着真实凭据，几乎必然是抽屉里误把「需要鉴权」关掉——占位凭据发
            // 往云端必 401。此时安全网改用真实凭据，按鉴权处理。本地端点（Ollama、
            // vLLM 等）保持占位值，让 Hermes 将该 provider 视为已配置。
            let trimmed_url = base_url.as_deref().map(str::trim).unwrap_or("");
            let is_local =
                trimmed_url.is_empty() || super::local_proxy::is_local_endpoint(trimmed_url);
            if is_local {
                key = "sophonote-local".to_string();
            } else if let Ok(saved) = crate::commands::get_cached_api_key(app, &provider) {
                if !saved.is_empty() {
                    key = saved;
                    keyless_override_to_authed = true;
                    eprintln!(
                        "[hermes] provider={provider} marked keyless but non-local endpoint with saved credential; using saved credential"
                    );
                } else {
                    key = "sophonote-local".to_string();
                }
            } else {
                key = "sophonote-local".to_string();
            }
        }
        if key.is_empty() {
            continue;
        }
        let prefix = provider_env_prefix(&provider);
        // 排障锚点：凭据长度与免鉴权判定，不含凭据内容。
        eprintln!(
            "[hermes] provider env: provider={provider} requires_key={requires_key} key_len={}",
            key.len()
        );
        environment.insert(format!("{prefix}_API_KEY"), key);
        if let Some(base_url) = base_url.as_deref() {
            if !base_url.trim().is_empty() {
                // MODEL-11④：免鉴权实例的 env 基址覆盖同样必须指向 loopback
                // 代理——Hermes 内置 ollama 定义带 OLLAMA_BASE_URL 覆盖且优先于
                // config 条目，若注入真实地址请求会绕过代理、占位 Bearer 直发
                // 真实端点被校验 401。安全网改用真实凭据的实例除外（真实 Bearer
                // 不能被代理剥离）。
                let effective = if !requires_key && !keyless_override_to_authed {
                    match super::local_proxy::proxy_port() {
                        Some(port) if super::local_proxy::is_http_target(base_url) => {
                            super::local_proxy::proxy_base_url(port, &provider)
                        }
                        _ => base_url.trim().to_string(),
                    }
                } else {
                    base_url.trim().to_string()
                };
                environment.insert(format!("{prefix}_BASE_URL"), effective);
            }
        }
    }
    Ok(environment)
}

fn provider_entries(
    parsed: &serde_json::Value,
    default_provider: (String, String, bool),
) -> Vec<(String, Option<String>, bool)> {
    parsed
        .get("providers")
        .and_then(serde_json::Value::as_object)
        .filter(|providers| !providers.is_empty())
        .map(|providers| {
            providers
                .iter()
                .map(|(provider, config)| {
                    (
                        provider.clone(),
                        config
                            .get("baseUrl")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        // 仅 requiresKey === false 视为免鉴权，与前端/Rust 网关口径一致。
                        // 该位直接表示 requires_key：true=需要凭据，false=免鉴权，缺省要求 Key。
                        // 历史版本此处误写为 `!flag`（取反），导致鉴权供应商被当免鉴权发占位
                        // 凭据（云端 401）、免鉴权供应商反而拿不到 OLLAMA_BASE_URL 代理注入。
                        config
                            .get("requiresKey")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true),
                    )
                })
                .collect()
        })
        .unwrap_or_else(|| {
            // 用户只填写默认 DeepSeek Key、从未修改过供应商表单时，SQLite 里
            // 还没有 ai_config。与 ModelGateway 使用同一个默认 snapshot，不能
            // 因缺少持久化行而让 Hermes 丢失凭据。
            vec![(
                default_provider.0,
                Some(default_provider.1),
                default_provider.2,
            )]
        })
}

fn provider_env_prefix(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "moonshot" | "kimi" | "kimi-coding" | "kimi-for-coding" | "k3" => "KIMI".into(),
        "kimi-coding-cn" => "KIMI_CN".into(),
        "alibaba" | "dashscope" | "aliyun" | "qwen" => "DASHSCOPE".into(),
        "zai" | "zhipu" | "glm" => "GLM".into(),
        "tencent-tokenhub" => "TOKENHUB".into(),
        value => value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn target_triple_matches_a_known_pack_target() {
        let target = target_triple();
        assert!(
            target == "aarch64-apple-darwin"
                || target == "x86_64-apple-darwin"
                || target == "x86_64-pc-windows-msvc"
                || target == "aarch64-pc-windows-msvc",
            "unexpected pack target {target}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn early_exit_hint_flags_sigkill_and_sigterm_conventions() {
        let killed = Command::new("/bin/sh")
            .args(["-c", "exit 137"])
            .status()
            .unwrap();
        assert!(early_exit_hint(killed).contains("SIGKILL"));
        assert!(early_exit_hint(killed).contains("com.apple.provenance"));
        let termed = Command::new("/bin/sh")
            .args(["-c", "exit 143"])
            .status()
            .unwrap();
        assert!(early_exit_hint(termed).contains("SIGTERM"));
        let other = Command::new("/bin/sh")
            .args(["-c", "exit 1"])
            .status()
            .unwrap();
        assert!(early_exit_hint(other).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn provenance_probe_is_a_noop_without_the_marker_and_tolerates_bad_paths() {
        let root = std::env::temp_dir().join(format!("sophonote-prov-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("python3"), b"#!/bin/sh\n").unwrap();
        // 稳态（无标记）：不产生副作用，也不 panic
        strip_provenance_xattrs(&root, "python3");
        // manifest 相对路径非法：静默返回而不是 panic
        strip_provenance_xattrs(&root, "../escape");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fresh_inode_rewrite_preserves_bytes_and_permissions_but_replaces_inode() {
        use std::os::unix::fs::MetadataExt;
        let root =
            std::env::temp_dir().join(format!("sophonote-rewrite-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("python3");
        std::fs::write(&target, b"\xca\xfe\xba\xbe").unwrap();
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&target, permissions).unwrap();
        let before_ino = std::fs::metadata(&target).unwrap().ino();

        rewrite_to_fresh_inode(&target);

        let after = std::fs::metadata(&target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"\xca\xfe\xba\xbe");
        assert_eq!(after.permissions().mode() & 0o777, 0o755);
        assert_ne!(after.ino(), before_ino, "rewrite must land on a new inode");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_prefix_is_safe_and_maps_kimi_family() {
        assert_eq!(provider_env_prefix("deepseek"), "DEEPSEEK");
        assert_eq!(provider_env_prefix("k3"), "KIMI");
        assert_eq!(provider_env_prefix("kimi-coding-cn"), "KIMI_CN");
        assert_eq!(provider_env_prefix("alibaba"), "DASHSCOPE");
        assert_eq!(provider_env_prefix("zai"), "GLM");
        assert_eq!(provider_env_prefix("minimax-cn"), "MINIMAX_CN");
        assert_eq!(provider_env_prefix("tencent-tokenhub"), "TOKENHUB");
        assert_eq!(provider_env_prefix("my-provider"), "MY_PROVIDER");
    }

    #[test]
    fn provider_entries_carry_requires_key_flag() {
        let parsed = serde_json::json!({
            "providers": {
                "ollama": {"baseUrl": "http://localhost:11434/v1", "requiresKey": false},
                "deepseek": {"baseUrl": "https://api.deepseek.com/v1"},
                "alibaba": {"baseUrl": "https://dashscope.aliyuncs.com/v1", "requiresKey": true}
            }
        });
        let entries = provider_entries(
            &parsed,
            ("deepseek".into(), "https://api.deepseek.com".into(), true),
        );
        assert_eq!(entries.len(), 3);
        let ollama = entries.iter().find(|(id, _, _)| id == "ollama").unwrap();
        assert_eq!(ollama.1.as_deref(), Some("http://localhost:11434/v1"));
        assert!(!ollama.2, "requiresKey=false 应标记为免鉴权");
        let deepseek = entries.iter().find(|(id, _, _)| id == "deepseek").unwrap();
        assert!(deepseek.2, "缺省 requiresKey 视为需要鉴权");
        let alibaba = entries.iter().find(|(id, _, _)| id == "alibaba").unwrap();
        assert!(
            alibaba.2,
            "requiresKey=true 必须标记为需要鉴权（历史取反 bug 回归防护）"
        );
    }

    #[test]
    fn provider_entries_fallback_defaults_require_key() {
        let entries = provider_entries(
            &serde_json::Value::Null,
            ("deepseek".into(), "https://api.deepseek.com".into(), true),
        );
        assert_eq!(
            entries,
            vec![(
                "deepseek".into(),
                Some("https://api.deepseek.com".into()),
                true
            )]
        );
    }

    #[test]
    fn resource_join_rejects_escape() {
        let root = Path::new("/tmp/runtime");
        assert!(checked_join(root, "runtime/bin/hermes").is_ok());
        assert!(checked_join(root, "../hermes").is_err());
        assert!(checked_join(root, "/tmp/hermes").is_err());
    }

    #[test]
    fn seed_upgrades_owned_skills_but_preserves_third_party_skills() {
        let root = std::env::temp_dir().join(format!("sophonote-skill-seed-{}", Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("destination");
        for skill in ["sophonote-ai-radar", "community-skill"] {
            std::fs::create_dir_all(source.join("productivity").join(skill)).unwrap();
            std::fs::create_dir_all(destination.join("productivity").join(skill)).unwrap();
        }
        std::fs::write(
            source.join("productivity/sophonote-ai-radar/SKILL.md"),
            "owned-new",
        )
        .unwrap();
        std::fs::write(
            destination.join("productivity/sophonote-ai-radar/SKILL.md"),
            "owned-old",
        )
        .unwrap();
        std::fs::write(
            source.join("productivity/community-skill/SKILL.md"),
            "community-new",
        )
        .unwrap();
        std::fs::write(
            destination.join("productivity/community-skill/SKILL.md"),
            "community-user-version",
        )
        .unwrap();

        seed_skills(&source, &destination, None).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.join("productivity/sophonote-ai-radar/SKILL.md"))
                .unwrap(),
            "owned-new"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("productivity/community-skill/SKILL.md"))
                .unwrap(),
            "community-user-version"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn seed_prefers_repo_override_for_owned_skills() {
        let root = std::env::temp_dir().join(format!("sophonote-skill-seed-{}", Uuid::new_v4()));
        let source = root.join("source");
        // owned_override 契约 = productivity 目录本身（start() 传
        // skills/hermes/productivity），因此夹具直接放 <skill>/ 不再垫一层。
        let override_root = root.join("repo");
        let destination = root.join("destination");
        for base in [&source.join("productivity"), &override_root] {
            std::fs::create_dir_all(base.join("sophonote-ai-radar")).unwrap();
        }
        std::fs::create_dir_all(destination.join("productivity/sophonote-ai-radar")).unwrap();
        std::fs::write(
            source.join("productivity/sophonote-ai-radar/SKILL.md"),
            "bundle-stale",
        )
        .unwrap();
        std::fs::write(
            override_root.join("sophonote-ai-radar/SKILL.md"),
            "repo-fresh",
        )
        .unwrap();

        seed_skills(&source, &destination, Some(&override_root)).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.join("productivity/sophonote-ai-radar/SKILL.md"))
                .unwrap(),
            "repo-fresh"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn seed_archives_and_does_not_restore_superseded_discovery_skill() {
        let root = std::env::temp_dir().join(format!("sophonote-skill-retire-{}", Uuid::new_v4()));
        let source = root.join("source");
        let destination = root.join("home/skills");
        let legacy_source = source.join("productivity/sophonote-discovery-subscriptions");
        let legacy_destination = destination.join("productivity/sophonote-discovery-subscriptions");
        std::fs::create_dir_all(&legacy_source).unwrap();
        std::fs::create_dir_all(&legacy_destination).unwrap();
        std::fs::write(legacy_source.join("SKILL.md"), "upstream-old").unwrap();
        std::fs::write(legacy_destination.join("SKILL.md"), "runtime-old").unwrap();

        seed_skills(&source, &destination, None).unwrap();

        assert!(!legacy_destination.exists());
        let backup_root = root.join("home/sophonote-skill-backups");
        assert_eq!(std::fs::read_dir(backup_root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_ai_config_projects_the_default_provider() {
        let entries = provider_entries(
            &serde_json::json!({}),
            (
                "deepseek".to_string(),
                "https://api.deepseek.com".to_string(),
                true,
            ),
        );
        assert_eq!(
            entries,
            vec![(
                "deepseek".to_string(),
                Some("https://api.deepseek.com".to_string()),
                true,
            )]
        );
    }

    #[cfg(unix)]
    #[test]
    fn graceful_shutdown_lets_watchdog_reap_its_child() {
        let root = std::env::temp_dir().join(format!(
            "sophonote-hermes-watchdog-test-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let child_pid_path = root.join("child.pid");
        let script_path = root.join("watchdog.sh");
        let script = format!(
            r#"#!/bin/sh
CHILD_PID=
cleanup() {{
  trap - EXIT INT TERM HUP
  if [ -n "$CHILD_PID" ] && kill -0 "$CHILD_PID" 2>/dev/null; then
    kill "$CHILD_PID" 2>/dev/null || true
    wait "$CHILD_PID" 2>/dev/null || true
  fi
}}
trap cleanup EXIT INT TERM HUP
sleep 30 &
CHILD_PID=$!
echo "$CHILD_PID" > "{}"
while kill -0 "$CHILD_PID" 2>/dev/null; do sleep 1; done
wait "$CHILD_PID" 2>/dev/null || true
"#,
            child_pid_path.display()
        );
        std::fs::write(&script_path, script).unwrap();
        let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script_path, permissions).unwrap();

        let mut watchdog = Command::new(&script_path).spawn().unwrap();
        for _ in 0..40 {
            if child_pid_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let descendant_pid = std::fs::read_to_string(&child_pid_path)
            .unwrap()
            .trim()
            .to_string();
        terminate_watchdog(&mut watchdog);

        let descendant_alive = Command::new("/bin/kill")
            .args(["-0", &descendant_pid])
            .status()
            .is_ok_and(|status| status.success());
        if descendant_alive {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &descendant_pid])
                .status();
        }
        let _ = std::fs::remove_dir_all(&root);
        assert!(!descendant_alive, "watchdog child {descendant_pid} leaked");
    }
}
