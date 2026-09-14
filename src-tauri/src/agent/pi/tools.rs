//! Pi native tools return over RPC to the Rust host. No model-owned filesystem effects.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub const FILE_LIMIT: u64 = 1024 * 1024;

#[derive(Clone)]
pub struct Scope {
    pub workspace: PathBuf,
    pub read_paths: Vec<PathBuf>,
    pub workcopy: Option<PathBuf>,
    pub protected: Vec<PathBuf>,
    pub mode: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub name: String,
    pub args: Value,
}

fn str_arg<'a>(request: &'a Request, key: &str) -> Result<&'a str, String> {
    request
        .args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("工具缺少 {key}"))
}

fn resolved(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|_| "无法解析路径".into());
    }
    let parent = path.parent().ok_or("无效路径")?;
    // Never create implicit ancestors: parent must already be authorized and real.
    Ok(parent
        .canonicalize()
        .map_err(|_| "父目录不存在")?
        .join(path.file_name().ok_or("无效文件名")?))
}

impl Scope {
    pub fn path(&self, raw: &str, write: bool) -> Result<PathBuf, String> {
        let path = resolved(&self.workspace.join(raw))?;
        let workcopy = self.workcopy.as_ref().is_some_and(|copy| *copy == path);
        if !workcopy && self.protected.iter().any(|root| path.starts_with(root)) {
            return Err("此路径属于受保护的应用数据，请使用显式文档工作副本".into());
        }
        let attached = !write
            && self
                .read_paths
                .iter()
                .any(|root| path == *root || (root.is_dir() && path.starts_with(root)));
        if !workcopy && !path.starts_with(&self.workspace) && !attached {
            return Err("路径不在本轮授权工作区或附件范围内".into());
        }
        if write && self.mode == "plan" {
            return Err("计划模式禁止修改文件".into());
        }
        Ok(path)
    }

    pub fn validate(&self, request: &Request) -> Result<bool, String> {
        match request.name.as_str() {
            "read" | "ls" => {
                self.path(
                    request
                        .args
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("."),
                    false,
                )?;
                Ok(false)
            }
            "write" | "edit" => {
                self.path(str_arg(request, "path")?, true)?;
                Ok(self.mode != "autoEdit")
            }
            "bash" => {
                if self.mode == "plan" {
                    return Err("计划模式禁止执行命令".into());
                }
                if !cfg!(target_os = "macos") {
                    return Err("此平台尚未提供受控命令执行".into());
                }
                let command = str_arg(request, "command")?;
                if command.trim().is_empty() || command.len() > 32 * 1024 {
                    return Err("命令为空或过长".into());
                }
                Ok(true)
            }
            _ => Err("智能体请求了未授权的工具".into()),
        }
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let mut data = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "无法打开文件")?
        .take(FILE_LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|_| "读取文件失败")?;
    if data.len() as u64 > FILE_LIMIT {
        return Err("文件超过 1 MiB，请缩小处理范围".into());
    }
    String::from_utf8(data).map_err(|_| "文件不是 UTF-8 文本".into())
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if text.len() as u64 > FILE_LIMIT {
        return Err("写入超过 1 MiB".into());
    }
    let temporary = path.with_file_name(format!(".sophonote-pi-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::write(&temporary, text).map_err(|_| "写入临时文件失败")?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())
                .map_err(|_| "保留文件权限失败")?;
        }
        std::fs::rename(&temporary, path).map_err(|_| "提交文件修改失败")
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(str::to_string)
}

pub async fn execute(
    scope: &Scope,
    request: &Request,
    cancel: &CancellationToken,
) -> Result<String, String> {
    scope.validate(request)?;
    if cancel.is_cancelled() {
        return Err("操作已取消".into());
    }
    if request.name == "bash" {
        return bash(scope, request, cancel).await;
    }
    let scope = scope.clone();
    let request = request.clone();
    tokio::task::spawn_blocking(move || {
        let raw = request
            .args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".");
        let path = scope.path(raw, matches!(request.name.as_str(), "edit" | "write"))?;
        match request.name.as_str() {
            "ls" => {
                let mut entries = std::fs::read_dir(path)
                    .map_err(|_| "无法读取目录")?
                    .take(1001)
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                entries.sort();
                if entries.len() > 1000 {
                    entries.truncate(1000);
                    entries.push("…目录已截断".into());
                }
                Ok(entries.join("\n"))
            }
            "read" => {
                let text = read_text(&path)?;
                let offset = request.args["offset"]
                    .as_u64()
                    .unwrap_or(1)
                    .clamp(1, 1_000_000) as usize;
                let limit = request.args["limit"].as_u64().unwrap_or(300).clamp(1, 1000) as usize;
                let output = text
                    .lines()
                    .enumerate()
                    .skip(offset - 1)
                    .take(limit)
                    .map(|(i, line)| format!("{}: {}", i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(output.chars().take(64 * 1024).collect())
            }
            "write" => {
                write_text(&path, str_arg(&request, "content")?)?;
                Ok("文件已写入；文档工作副本需在 Diff 中批准后才写回笔记。".into())
            }
            "edit" => {
                let text = read_text(&path)?;
                let old = str_arg(&request, "oldText")?;
                if old.is_empty() || text.matches(old).count() != 1 {
                    return Err("原文不存在或匹配不唯一，请重新读取并提供唯一锚点".into());
                }
                write_text(&path, &text.replacen(old, str_arg(&request, "newText")?, 1))?;
                Ok("修改已保存；文档工作副本仍需 Diff 审批。".into())
            }
            _ => Err("未知工具".into()),
        }
    })
    .await
    .map_err(|_| "工具任务异常")?
}

/// Always reap the child and its process group, including detached descendants retaining pipes.
pub struct ManagedChild(pub tokio::process::Child, #[cfg(unix)] Option<u32>);
impl ManagedChild {
    pub fn new(child: tokio::process::Child) -> Self {
        #[cfg(unix)]
        let pid = child.id();
        Self(
            child,
            #[cfg(unix)]
            pid,
        )
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.1 {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.start_kill();
    }
}

fn sandbox_profile(scope: &Scope) -> String {
    let quoted = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap_or_default();
    let mut profile = format!("(version 1)(allow default)(deny file-write*)(allow file-write* (subpath {}) (subpath {}) (literal \"/dev/null\"))",
        quoted(&scope.workspace), quoted(&std::env::temp_dir()));
    for path in &scope.protected {
        profile.push_str(&format!(
            "(deny file-read* file-write* (subpath {}))",
            quoted(path)
        ));
    }
    profile
}

async fn bash(
    scope: &Scope,
    request: &Request,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let mut command = tokio::process::Command::new("/usr/bin/sandbox-exec");
    command
        .args([
            "-p",
            &sandbox_profile(scope),
            "/bin/bash",
            "--noprofile",
            "--norc",
            "-c",
            str_arg(request, "command")?,
        ])
        .current_dir(&scope.workspace)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        )
        .env("HOME", &scope.workspace)
        .env("LANG", "en_US.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = ManagedChild::new(command.spawn().map_err(|_| "无法启动受控命令")?);
    let stdout = child.0.stdout.take().ok_or("命令缺少 stdout")?;
    let stderr = child.0.stderr.take().ok_or("命令缺少 stderr")?;
    let limit = 64 * 1024;
    let drain = |stream: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
        let mut stream = stream;
        let mut result = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            let keep = n.min(limit - result.len());
            result.extend_from_slice(&buf[..keep]);
        }
        Ok::<_, std::io::Error>(result)
    };
    let timeout = request.args["timeout"].as_u64().unwrap_or(60).clamp(1, 60);
    let result = tokio::select! {
        _ = cancel.cancelled() => return Err("命令已取消".into()),
        result = tokio::time::timeout(std::time::Duration::from_secs(timeout), async {
            tokio::try_join!(child.0.wait(), drain(Box::new(stdout)), drain(Box::new(stderr)))
        }) => result,
    }
    .map_err(|_| "命令超时（最多 60 秒）")?
    .map_err(|_| "读取命令结果失败")?;
    Ok(format!(
        "退出码：{}\n{}{}",
        result.0.code().map_or("信号终止".into(), |n| n.to_string()),
        String::from_utf8_lossy(&result.1),
        String::from_utf8_lossy(&result.2)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn shell_enforces_protected_paths_and_timeout() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("pi-shell-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("protected")).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("protected/secret"), "private-fixture").unwrap();
        let scope = Scope {
            workspace: root.clone(),
            read_paths: vec![],
            workcopy: None,
            protected: vec![root.join("protected")],
            mode: "ask".into(),
        };
        let request = Request {
            name: "bash".into(),
            args: serde_json::json!({"command":"printf safe > allowed.txt; cat protected/secret; printf unsafe > protected/new"}),
        };
        assert!(scope.validate(&request).unwrap());
        let result = execute(&scope, &request, &CancellationToken::new())
            .await
            .unwrap();
        assert!(!result.contains("private-fixture"));
        assert_eq!(
            std::fs::read_to_string(root.join("allowed.txt")).unwrap(),
            "safe"
        );
        assert!(!root.join("protected/new").exists());
        let request = Request {
            name: "bash".into(),
            args: serde_json::json!({"command":"sleep 10", "timeout":1}),
        };
        assert!(execute(&scope, &request, &CancellationToken::new())
            .await
            .unwrap_err()
            .contains("超时"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scope_rejects_traversal_and_protected_data_even_inside_workspace() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("pi-scope-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("notes")).unwrap();
        let root = root.canonicalize().unwrap();
        let scope = Scope {
            workspace: root.clone(),
            read_paths: vec![],
            workcopy: None,
            protected: vec![root.join("notes")],
            mode: "ask".into(),
        };
        assert!(scope.path("../outside.md", true).is_err());
        assert!(scope.path("notes/a.md", true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.parent().unwrap(), root.join("escape")).unwrap();
            assert!(scope.path("escape/outside.md", true).is_err());
        }
        let edit = Request {
            name: "write".into(),
            args: serde_json::json!({"path":"a.md","content":"hello"}),
        };
        assert!(scope.validate(&edit).unwrap());
        let mut scope = scope;
        scope.mode = "autoEdit".into();
        assert!(!scope.validate(&edit).unwrap());
        scope.mode = "plan".into();
        assert!(scope.validate(&edit).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
