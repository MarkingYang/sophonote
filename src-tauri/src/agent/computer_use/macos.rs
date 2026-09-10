//! The signed GUI process owns TCC and the daemon. Hermes only gets a proxy.
use super::{ComputerUseActionStatus, ComputerUseCheck, ComputerUseStatus};
use rmcp::{model::CallToolRequestParams, ServiceExt};
use serde_json::{json, Value};
use std::{
    os::unix::fs::DirBuilderExt, path::PathBuf, process::Stdio, sync::OnceLock, time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

const HOST_ID: &str = "com.fei.sophonote";
const VERSION: &str = "0.26.0";
static HOST: OnceLock<Host> = OnceLock::new();

struct Host {
    root: PathBuf,
    directory: PathBuf,
    driver_home: PathBuf,
    bundled: bool,
    supported: bool,
    daemon: Mutex<Option<Daemon>>,
    diagnostic: Mutex<()>,
}
struct Daemon {
    child: Child,
    permissions: (bool, bool),
}

fn host() -> Result<&'static Host, String> {
    HOST.get()
        .ok_or_else(|| "SophoNote 电脑操作尚未初始化".into())
}

pub async fn configure(app: &AppHandle, command: &mut std::process::Command) -> Result<(), String> {
    if HOST.get().is_none() {
        // The pinned upstream binary has LC_BUILD_VERSION minos 13.0.
        let supported = Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|v| v.split('.').next().and_then(|n| n.parse::<u32>().ok()))
            .is_some_and(|major| major >= 13);
        let bundled = std::env::current_exe().ok().is_some_and(|p| {
            p.parent()
                .is_some_and(|p| p.ends_with("SophoNote.app/Contents/MacOS"))
        });
        let root = app
            .path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("computer-use");
        #[cfg(debug_assertions)]
        let root = if !bundled {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/computer-use")
        } else {
            root
        };
        let directory =
            std::env::temp_dir().join(format!("sn-cua-{}", uuid::Uuid::new_v4().simple()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|e| e.to_string())?;
        let driver_home = crate::storage_layout::StorageLayout::resolve(app)?
            .runtime
            .join("computer-use");
        std::fs::create_dir_all(&driver_home).map_err(|e| e.to_string())?;
        let candidate = Host {
            driver_home,
            root,
            directory: directory.clone(),
            bundled,
            supported,
            daemon: Mutex::new(None),
            diagnostic: Mutex::new(()),
        };
        if HOST.set(candidate).is_err() {
            let _ = std::fs::remove_dir(directory);
        }
    }
    let h = host()?;
    // An absent resource is an unavailable capability, never a PATH fallback.
    command
        .env("HERMES_CUA_DRIVER_CMD", h.root.join("hermes-cua"))
        .env("SOPHONOTE_CUA_SOCKET", h.directory.join("driver.sock"))
        .env("SOPHONOTE_CUA_HOME", &h.driver_home);
    Ok(())
}

/// Run after the Tauri event loop is ready. Optional native diagnostics must
/// never hold setup() while WKWebView is trying to load its first document.
pub async fn prime() {
    if let Ok(h) = host() {
        if h.bundled && h.supported && h.root.join("cua-driver").is_file() {
            match status().await {
                Ok(report) => {
                    if let Some(error) = report.error {
                        eprintln!("[computer-use] {error}");
                    }
                }
                Err(error) => eprintln!("[computer-use] {error}"),
            }
        }
    }
}

fn driver_command(h: &Host) -> Command {
    let mut command = Command::new(h.root.join("cua-driver"));
    // The child has a private configuration home: upstream otherwise writes a
    // global cua-driver PID/config even in embedded mode. Parent HOME is unchanged.
    // Third-party control code receives no provider credentials or mode overrides.
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &h.driver_home)
        .env("CUA_DRIVER_EMBEDDED", "1")
        .env("CUA_DRIVER_HOST_BUNDLE_ID", HOST_ID)
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "0")
        .kill_on_drop(true);
    command
}

async fn stop_locked(h: &Host, slot: &mut Option<Daemon>) {
    if let Some(mut daemon) = slot.take() {
        // EOF is the upstream parent-liveness contract, including host crashes.
        drop(daemon.child.stdin.take());
        if tokio::time::timeout(Duration::from_secs(3), daemon.child.wait())
            .await
            .is_err()
        {
            let _ = daemon.child.kill().await;
        }
    }
    // Only remove the endpoint inside the directory this process created.
    let _ = std::fs::remove_file(h.directory.join("driver.sock"));
}

async fn ensure_started(h: &Host) -> Result<(), String> {
    if !h.supported {
        return Err("电脑操作需要 macOS 13 或更新版本".into());
    }
    if !h.bundled {
        return Err("请打开打包后的 SophoNote.app 使用电脑操作；开发终端不拥有应用权限身份".into());
    }
    let permissions = native_permissions();
    let mut slot = h.daemon.lock().await;
    if let Some(daemon) = slot.as_mut() {
        if daemon
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_none()
            && daemon.permissions == permissions
        {
            return Ok(());
        }
    }
    stop_locked(h, &mut slot).await;
    let mut command = driver_command(h);
    command
        .args([
            "serve",
            "--embedded",
            "--no-permissions-gate",
            "--permission-mode",
            "standard",
            "--socket",
        ])
        .arg(h.directory.join("driver.sock"))
        .env("CUA_DRIVER_PARENT_LIVENESS_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| format!("启动内置电脑组件失败：{e}"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("内置电脑组件提前退出：{exit}"));
        }
        if tokio::net::UnixStream::connect(h.directory.join("driver.sock"))
            .await
            .is_ok()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("内置电脑组件启动超时".into());
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    *slot = Some(Daemon { child, permissions });
    Ok(())
}

async fn diagnostic(h: &Host) -> Result<(Value, Value), String> {
    use rmcp::{model::CallToolResponse, transport::child_process::TokioChildProcess};
    let mut command = driver_command(h);
    command
        .args(["mcp", "--embedded", "--socket"])
        .arg(h.directory.join("driver.sock"));
    let transport = TokioChildProcess::new(command).map_err(|e| e.to_string())?;
    let client = ().serve(transport).await.map_err(|e| e.to_string())?;
    let calls = async {
        let mut values = Vec::new();
        for (name, args) in [
            ("check_permissions", json!({})),
            ("health_report", json!({"include":["bundle_identity"]})),
        ] {
            let params =
                CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone());
            let response = client
                .call_tool_once(params)
                .await
                .map_err(|e| e.to_string())?;
            let CallToolResponse::Complete(result) = response else {
                return Err("只读诊断意外请求交互".to_string());
            };
            if result.is_error == Some(true) {
                return Err("内置电脑组件诊断失败".into());
            }
            values.push(
                result
                    .structured_content
                    .ok_or("电脑组件未返回结构化诊断")?,
            );
        }
        Ok((values.remove(0), values.remove(0)))
    }
    .await;
    let _ = client.cancel().await;
    calls
}

fn identity_matches(permissions: &Value, health: &Value, pid: u32) -> bool {
    permissions
        .pointer("/source/attribution")
        .and_then(Value::as_str)
        == Some("host")
        && permissions
            .pointer("/source/host_bundle_id")
            .and_then(Value::as_str)
            == Some(HOST_ID)
        && health
            .get("checks")
            .and_then(Value::as_array)
            .is_some_and(|checks| {
                checks.iter().any(|check| {
                    check.get("name").and_then(Value::as_str) == Some("bundle_identity")
                        && check.get("status").and_then(Value::as_str) == Some("pass")
                        && check
                            .pointer("/data/bundle_identifier")
                            .and_then(Value::as_str)
                            == Some(HOST_ID)
                        && check
                            .pointer("/data/identity_source")
                            .and_then(Value::as_str)
                            == Some("parent_application")
                        && check
                            .pointer("/data/parent_process_id")
                            .and_then(Value::as_u64)
                            == Some(pid as u64)
                })
            })
}

pub async fn status() -> Result<ComputerUseStatus, String> {
    let h = host()?;
    let _guard = h.diagnostic.lock().await;
    let installed = h.root.join("cua-driver").is_file() && h.root.join("hermes-cua").is_file();
    let (ax, screen) = native_permissions();
    let mut result = ComputerUseStatus {
        installed,
        platform: "darwin".into(),
        platform_supported: h.supported,
        version: installed.then(|| format!("内置组件 {VERSION}")),
        ready: Some(false),
        can_grant: h.bundled && h.supported,
        accessibility: Some(ax),
        screen_recording: Some(screen),
        checks: Vec::new(),
        error: None,
        embedded: true,
        permission_owner: None,
    };
    if !installed {
        result.error = Some("安装包缺少内置电脑组件，请重新安装完整的 SophoNote.app".into());
        return Ok(result);
    }
    if let Err(error) = ensure_started(h).await {
        result.error = Some(error);
        return Ok(result);
    }
    match tokio::time::timeout(Duration::from_secs(12), diagnostic(h)).await {
        Ok(Ok((permissions, health))) => {
            let identity = identity_matches(&permissions, &health, std::process::id());
            result.permission_owner = identity.then(|| "SophoNote".into());
            result.checks.push(ComputerUseCheck {
                label: "系统权限归属".into(),
                status: if identity { "pass" } else { "fail" }.into(),
                message: if identity {
                    "SophoNote"
                } else {
                    "未验证为 SophoNote 主进程，已停止使用"
                }
                .into(),
            });
            result.accessibility = permissions.get("accessibility").and_then(Value::as_bool);
            result.screen_recording = permissions.get("screen_recording").and_then(Value::as_bool);
            result.ready = Some(
                identity
                    && ax
                    && screen
                    && result.accessibility == Some(true)
                    && result.screen_recording == Some(true),
            );
            if !identity {
                result.error = Some("电脑组件的系统权限身份不符，请重启 SophoNote".into());
            }
        }
        Ok(Err(error)) => result.error = Some(error),
        Err(_) => result.error = Some("电脑组件诊断超时，请重新检测".into()),
    }
    if result.error.is_some() {
        stop_locked(h, &mut *h.daemon.lock().await).await;
    }
    Ok(result)
}

pub async fn grant(app: AppHandle) -> Result<ComputerUseActionStatus, String> {
    let h = host()?;
    if !h.bundled {
        return Err("请打开 SophoNote.app 后申请系统授权".into());
    }
    let _guard = h.diagnostic.lock().await;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        request_native_permissions();
        let _ = sender.send(());
    })
    .map_err(|e| e.to_string())?;
    receiver.await.map_err(|e| e.to_string())?;
    // The user may grant later in Settings; status compares grants and replaces
    // the daemon to invalidate its per-process TCC cache on the next check.
    Ok(ComputerUseActionStatus {
        running: false,
        exit_code: Some(0),
        pid: None,
    })
}

pub async fn shutdown() {
    if let Some(h) = HOST.get() {
        stop_locked(h, &mut *h.daemon.lock().await).await;
        let _ = std::fs::remove_dir(&h.directory);
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const std::ffi::c_void;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionaryCreateMutable(
        allocator: *const std::ffi::c_void,
        capacity: isize,
        keys: *const std::ffi::c_void,
        values: *const std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn CFDictionarySetValue(
        dictionary: *mut std::ffi::c_void,
        key: *const std::ffi::c_void,
        value: *const std::ffi::c_void,
    );
    fn CFRelease(value: *const std::ffi::c_void);
    static kCFBooleanTrue: *const std::ffi::c_void;
}
fn native_permissions() -> (bool, bool) {
    // Both APIs are read-only and run inside the actual Tauri host.
    unsafe { (AXIsProcessTrusted(), CGPreflightScreenCaptureAccess()) }
}
fn request_native_permissions() {
    unsafe {
        let options =
            CFDictionaryCreateMutable(std::ptr::null(), 1, std::ptr::null(), std::ptr::null());
        if !options.is_null() {
            CFDictionarySetValue(options, kAXTrustedCheckOptionPrompt, kCFBooleanTrue);
            AXIsProcessTrustedWithOptions(options);
            CFRelease(options);
        }
        CGRequestScreenCaptureAccess();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_label_alone_does_not_prove_permission_identity() {
        let p = json!({"source":{"attribution":"host","host_bundle_id":HOST_ID}});
        let mut health = json!({"checks":[{"name":"bundle_identity","status":"pass","data":{"bundle_identifier":HOST_ID,"identity_source":"parent_application","parent_process_id":42}}]});
        assert!(identity_matches(&p, &health, 42));
        assert!(!identity_matches(&p, &health, 43));
        assert!(!identity_matches(&p, &json!({}), 42));
        health["checks"][0]["data"]["bundle_identifier"] = json!("com.trycua.driver");
        assert!(!identity_matches(&p, &health, 42));
    }
}
