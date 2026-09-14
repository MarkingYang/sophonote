use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct Runtime {
    pub executable: PathBuf,
    pub version: String,
}

fn supported(version: &str) -> bool {
    let parts = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>();
    parts.is_ok_and(|p| p.len() == 3 && (p[0], p[1], p[2]) >= (2, 1, 233))
}

pub(crate) async fn locate() -> Result<Runtime, String> {
    let runtime = inspect().await?;
    if !supported(&runtime.version) {
        return Err("请升级官方 Claude Code CLI 至 2.1.233 或更新版本后重试".into());
    }
    Ok(runtime)
}

pub(crate) async fn inspect() -> Result<Runtime, String> {
    let explicit = std::env::var_os("SOPHONOTE_CLAUDE_BIN").map(PathBuf::from);
    let candidates = if let Some(path) = explicit {
        if !path.is_absolute() {
            return Err("SOPHONOTE_CLAUDE_BIN 必须是绝对路径".into());
        }
        vec![path]
    } else {
        let mut paths = vec![
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ];
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            paths.push(PathBuf::from(home).join(if cfg!(windows) {
                ".local/bin/claude.exe"
            } else {
                ".local/bin/claude"
            }));
        }
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(
                std::env::split_paths(&path)
                    .filter(|p| p.is_absolute())
                    .map(|p| {
                        p.join(if cfg!(windows) {
                            "claude.exe"
                        } else {
                            "claude"
                        })
                    }),
            );
        }
        paths
    };
    let executable = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or("未检测到 Claude Code。请先安装官方 Claude Code CLI（2.1.233 或更新版本）后重试。")?
        .canonicalize()
        .map_err(|_| "Claude Code 路径不可访问")?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&executable)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Claude Code 版本检查超时")?
    .map_err(|_| "Claude Code 无法启动")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().next().unwrap_or_default();
    if !output.status.success() || !text.contains("Claude Code") || version.split('.').count() != 3
    {
        return Err("请升级官方 Claude Code CLI 至 2.1.233 或更新版本后重试".into());
    }
    Ok(Runtime {
        executable,
        version: version.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_isolated_bare_cli_version() {
        assert!(supported("2.1.233"));
        assert!(supported("2.2.0"));
        assert!(!supported("2.1.232"));
        assert!(!supported("invalid"));
    }
}
