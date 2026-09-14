//! Host-owned update operations for optional Agent runtimes.
use crate::commands::ApiResponse;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tauri::{AppHandle, Emitter};

pub(crate) static CLAUDE_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static PI_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static PROGRESS: Mutex<[Option<UpdateProgress>; 2]> = Mutex::new([None, None]);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    engine: String,
    phase: String,
    state: String,
    message: String,
    bytes_downloaded: Option<u64>,
    total_bytes: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub engine: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub source: String,
    pub can_update: bool,
    pub message: Option<String>,
    pub progress: Option<UpdateProgress>,
}
pub(crate) fn report(
    app: &AppHandle,
    engine: &str,
    phase: &str,
    state: &str,
    message: &str,
    bytes: Option<u64>,
    total: Option<u64>,
) {
    let value = UpdateProgress {
        engine: engine.into(),
        phase: phase.into(),
        state: state.into(),
        message: message.into(),
        bytes_downloaded: bytes,
        total_bytes: total,
    };
    if let Ok(mut entries) = PROGRESS.lock() {
        entries[usize::from(engine == "claude_code")] = Some(value.clone());
    }
    let _ = app.emit("sophonote:agent-update-progress", value);
}
fn progress(engine: &str) -> Option<UpdateProgress> {
    PROGRESS
        .lock()
        .ok()
        .and_then(|p| p[usize::from(engine == "claude_code")].clone())
}

#[derive(Debug, PartialEq)]
enum ClaudeInstall {
    Native,
    NodePackage,
    Brew(&'static str, &'static str),
    Manual,
}
fn claude_install(executable: &Path, home: Option<&Path>, explicit: bool) -> ClaudeInstall {
    if explicit {
        return ClaudeInstall::Manual;
    }
    for prefix in ["/opt/homebrew", "/usr/local"] {
        for cask in ["claude-code", "claude-code@latest"] {
            if executable.starts_with(Path::new(prefix).join("Caskroom").join(cask)) {
                return ClaudeInstall::Brew(
                    if prefix == "/opt/homebrew" {
                        "/opt/homebrew/bin/brew"
                    } else {
                        "/usr/local/bin/brew"
                    },
                    cask,
                );
            }
        }
    }
    // The official npm package ships the native binary; its documented updater remains `claude update`.
    if executable
        .ancestors()
        .any(|p| p.ends_with("node_modules/@anthropic-ai/claude-code"))
    {
        return ClaudeInstall::NodePackage;
    }
    if home.is_some_and(|h| executable.starts_with(h.join(".local/share/claude/versions"))) {
        ClaudeInstall::Native
    } else {
        ClaudeInstall::Manual
    }
}
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

async fn status(app: &AppHandle, engine: &str, check: bool) -> Result<UpdateStatus, String> {
    match engine {
        "pi" => {
            let handle = app.clone();
            let resolved = tokio::task::spawn_blocking(move || super::pi::update::resolve(&handle))
                .await
                .map_err(|_| "Pi 状态校验失败")?;
            let (version, message, source) = match resolved {
                Ok((r, warning)) => (
                    Some(r.version),
                    warning,
                    if r.executable.starts_with(super::pi::update::root(app)?) {
                        "应用私有更新槽"
                    } else {
                        "随 SophoNote 分发"
                    },
                ),
                Err(error) => (None, Some(error), "运行时未就绪"),
            };
            let latest = if check {
                Some(super::pi::update::latest().await?.version)
            } else {
                None
            };
            Ok(UpdateStatus {
                engine: engine.into(),
                current_version: version,
                latest_version: latest,
                source: source.into(),
                can_update: true,
                message,
                progress: progress(engine),
            })
        }
        "claude_code" => {
            let runtime = super::claude::runtime::inspect().await;
            let (version, source, can_update, message) = match runtime {
                Ok(r) => {
                    let install = claude_install(
                        &r.executable,
                        home().as_deref(),
                        std::env::var_os("SOPHONOTE_CLAUDE_BIN").is_some(),
                    );
                    let source = match install {
                        ClaudeInstall::Native => "本机官方原生安装",
                        ClaudeInstall::NodePackage => "本机官方 npm / pnpm 安装",
                        ClaudeInstall::Brew(_, cask) => cask,
                        ClaudeInstall::Manual => "本机自定义或包管理器安装",
                    };
                    let manual = install == ClaudeInstall::Manual;
                    (
                        Some(r.version),
                        source,
                        !manual,
                        manual.then(|| {
                            "请按原安装方式更新，然后点击重新检测；自定义路径不会被自动替换。"
                                .into()
                        }),
                    )
                }
                Err(e) => (None, "尚未检测到可用安装", false, Some(e)),
            };
            Ok(UpdateStatus {
                engine: engine.into(),
                current_version: version,
                latest_version: None,
                source: source.into(),
                can_update,
                message,
                progress: progress(engine),
            })
        }
        _ => Err("未知 Agent 更新类型".into()),
    }
}

pub(crate) fn update_command_environment(command: &mut tokio::process::Command) {
    command.env_clear();
    for name in [
        "HOME",
        "USERPROFILE",
        "PATH",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
}
fn ensure_claude_idle(conn: &rusqlite::Connection) -> Result<(), String> {
    let busy: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE engine='claude_code' AND lower(trim(status, '\"')) NOT IN ('completed','failed','cancelled','interrupted'))", [], |r| r.get(0)).map_err(|_| "无法检查在途会话")?;
    if busy {
        return Err("请先结束正在执行的 Claude Code 会话，再更新本机 CLI。".into());
    }
    Ok(())
}
async fn update_claude(app: &AppHandle) -> Result<(), String> {
    let conn =
        rusqlite::Connection::open(crate::db::get_db_path(app)).map_err(|_| "无法检查在途会话")?;
    ensure_claude_idle(&conn)?;
    drop(conn);
    report(
        app,
        "claude_code",
        "checking",
        "running",
        "正在识别本机安装方式…",
        None,
        None,
    );
    let current = super::claude::runtime::inspect().await?;
    let install = claude_install(
        &current.executable,
        home().as_deref(),
        std::env::var_os("SOPHONOTE_CLAUDE_BIN").is_some(),
    );
    let mut command = match install {
        ClaudeInstall::Native | ClaudeInstall::NodePackage => {
            let mut c = tokio::process::Command::new(&current.executable);
            c.arg("update");
            c
        }
        ClaudeInstall::Brew(binary, cask) => {
            let mut c = tokio::process::Command::new(binary);
            c.args(["upgrade", "--cask", cask]);
            c.env("HOMEBREW_NO_AUTO_UPDATE", "1");
            c
        }
        ClaudeInstall::Manual => {
            return Err("此安装方式需手动更新，请使用原安装工具或官方指引。".into())
        }
    };
    update_command_environment(&mut command);
    command.env("HOMEBREW_NO_AUTO_UPDATE", "1");
    if let Some(home) = home() {
        command.current_dir(home);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    report(
        app,
        "claude_code",
        "installing",
        "running",
        "官方更新器正在检查、下载并更新本机 CLI（最长 3 分钟）…",
        None,
        None,
    );
    let mut child = super::pi::tools::ManagedChild::new(
        command
            .spawn()
            .map_err(|_| "无法启动官方更新器，请检查安装权限")?,
    );
    let exit = tokio::time::timeout(Duration::from_secs(180), child.0.wait())
        .await
        .map_err(|_| "官方更新超时，请检查网络并通过原安装方式重试")?
        .map_err(|_| "官方更新进程异常")?;
    if !exit.success() {
        return Err(format!(
            "官方更新器退出（{}），请检查网络、目录写权限或通过原安装方式更新。",
            exit.code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "被终止".into())
        ));
    }
    report(
        app,
        "claude_code",
        "verifying",
        "running",
        "正在复核更新后的 CLI 版本…",
        None,
        None,
    );
    let updated = super::claude::runtime::locate().await?;
    let message = if updated.version == current.version {
        format!(
            "官方更新器已完成检查，当前版本仍为 {}；下一轮使用此版本。",
            updated.version
        )
    } else {
        format!(
            "已更新至 {}，下一轮 Claude Code 会话生效。",
            updated.version
        )
    };
    report(
        app,
        "claude_code",
        "ready",
        "completed",
        &message,
        None,
        None,
    );
    Ok(())
}

#[tauri::command]
pub async fn agent_runtime_update_status(
    app: AppHandle,
    engine: String,
    check: Option<bool>,
) -> ApiResponse<UpdateStatus> {
    match status(&app, &engine, check.unwrap_or(false)).await {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}
#[tauri::command]
pub async fn agent_runtime_update(app: AppHandle, engine: String) -> ApiResponse<UpdateStatus> {
    let result = match engine.as_str() {
        "pi" => match PI_GATE.try_lock() {
            Ok(_guard) => super::pi::update::pull(&app).await,
            Err(_) => return ApiResponse::err("Pi 更新已在进行中".into()),
        },
        "claude_code" => match CLAUDE_GATE.try_lock() {
            Ok(_guard) => update_claude(&app).await,
            Err(_) => return ApiResponse::err("Claude Code 正在启动或更新，请稍后重试".into()),
        },
        _ => Err("未知 Agent 更新类型".into()),
    };
    if let Err(error) = result {
        if matches!(engine.as_str(), "pi" | "claude_code") {
            report(&app, &engine, "failed", "failed", &error, None, None);
        }
        return ApiResponse::err(error);
    }
    match status(&app, &engine, false).await {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_update_rejects_active_runs_but_allows_terminal_and_other_engines() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_runs(engine TEXT,status TEXT);")
            .unwrap();
        for state in [
            "queued",
            "running",
            "waiting_approval",
            "Running",
            "unknown",
        ] {
            conn.execute(
                "INSERT INTO agent_runs VALUES ('claude_code',?1)",
                [format!("\"{state}\"")],
            )
            .unwrap();
            assert!(ensure_claude_idle(&conn).is_err());
            conn.execute("DELETE FROM agent_runs", []).unwrap();
        }
        for state in [
            "completed",
            "failed",
            "cancelled",
            "interrupted",
            "Completed",
        ] {
            conn.execute(
                "INSERT INTO agent_runs VALUES ('claude_code',?1)",
                [format!("\"{state}\"")],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO agent_runs VALUES ('pi','running')", [])
            .unwrap();
        assert!(ensure_claude_idle(&conn).is_ok());
    }

    #[test]
    fn install_detection_preserves_package_manager_and_custom_paths() {
        let h = Path::new("/Users/test");
        assert_eq!(
            claude_install(
                Path::new(
                    "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe"
                ),
                Some(h),
                false
            ),
            ClaudeInstall::NodePackage
        );
        assert_eq!(
            claude_install(
                &h.join(".local/share/claude/versions/2.1.233"),
                Some(h),
                false
            ),
            ClaudeInstall::Native
        );
        assert_eq!(
            claude_install(
                Path::new("/opt/homebrew/Caskroom/claude-code@latest/2/claude"),
                Some(h),
                false
            ),
            ClaudeInstall::Brew("/opt/homebrew/bin/brew", "claude-code@latest")
        );
        assert_eq!(
            claude_install(
                Path::new("/opt/homebrew/Caskroom/claude-code-evil/bin/claude"),
                Some(h),
                false
            ),
            ClaudeInstall::Manual
        );
        assert_eq!(
            claude_install(&h.join(".local/share/claude/versions/2"), Some(h), true),
            ClaudeInstall::Manual
        );
    }
}
