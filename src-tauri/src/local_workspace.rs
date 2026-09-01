use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use tauri::Manager;

use crate::commands::ApiResponse;

const MAX_PREVIEW_BYTES: usize = 512 * 1024;
const MAX_FULL_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 768 * 1024;

// 与常见代码编辑器一致，仅隐藏版本库内部对象；其余目录由用户按需展开。
const SKIPPED_DIRECTORIES: &[&str] = &[".git"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalWorkspaceEntry {
    path: String,
    name: String,
    kind: String,
    depth: usize,
    size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalWorkspaceSnapshot {
    root: String,
    name: String,
    entries: Vec<LocalWorkspaceEntry>,
    truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFilePreview {
    path: String,
    content: String,
    size: u64,
    truncated: bool,
    fingerprint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGitChange {
    path: String,
    status: String,
    staged: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGitStatus {
    is_repo: bool,
    branch: Option<String>,
    ahead: u32,
    behind: u32,
    changes: Vec<LocalGitChange>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGitDiff {
    path: Option<String>,
    content: String,
    truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCommandResult {
    command: String,
    output: String,
    exit_code: Option<i32>,
    timed_out: bool,
    truncated: bool,
}

pub(crate) fn canonical_root(root: &str) -> Result<PathBuf, String> {
    let path = Path::new(root);
    if root.trim().is_empty() || !path.is_absolute() {
        return Err("请选择有效的本地项目目录".to_string());
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法读取项目目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("项目路径必须是非符号链接目录".to_string());
    }
    path.canonicalize()
        .map_err(|error| format!("无法解析项目目录：{error}"))
}

fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.trim().is_empty() || path.is_absolute() {
        return Err("文件路径无效".to_string());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("文件路径不能离开项目目录".to_string());
        }
    }
    Ok(path.to_path_buf())
}

fn resolve_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = safe_relative_path(relative)?;
    let candidate = root.join(relative);
    let metadata =
        fs::symlink_metadata(&candidate).map_err(|error| format!("无法读取文件：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("仅支持项目目录内的普通文件".to_string());
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("无法解析文件：{error}"))?;
    if !canonical.starts_with(root) {
        return Err("文件路径不能离开项目目录".to_string());
    }
    Ok(canonical)
}

fn relative_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn resolve_directory(root: &Path, relative: Option<&str>) -> Result<PathBuf, String> {
    let candidate = match relative {
        Some(value) => root.join(safe_relative_path(value)?),
        None => root.to_path_buf(),
    };
    let metadata =
        fs::symlink_metadata(&candidate).map_err(|error| format!("无法读取目录：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("仅支持项目目录内的普通目录".to_string());
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("无法解析目录：{error}"))?;
    if !canonical.starts_with(root) {
        return Err("目录路径不能离开项目目录".to_string());
    }
    Ok(canonical)
}

fn list_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
) -> Result<Vec<LocalWorkspaceEntry>, String> {
    let mut children = fs::read_dir(directory)
        .map_err(|error| format!("无法读取目录：{error}"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    children.sort_by_key(|entry| {
        let is_file = entry.file_type().map(|kind| kind.is_file()).unwrap_or(true);
        (is_file, entry.file_name().to_string_lossy().to_lowercase())
    });

    let mut entries = Vec::with_capacity(children.len());
    for child in children {
        let name = child.file_name().to_string_lossy().to_string();
        let file_type = match child.file_type() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if file_type.is_symlink()
            || (file_type.is_dir() && SKIPPED_DIRECTORIES.contains(&name.as_str()))
        {
            continue;
        }
        let path = child.path();
        let is_directory = file_type.is_dir();
        entries.push(LocalWorkspaceEntry {
            path: relative_string(root, &path),
            name,
            kind: if is_directory { "directory" } else { "file" }.to_string(),
            depth,
            size: if file_type.is_file() {
                child.metadata().ok().map(|value| value.len())
            } else {
                None
            },
        });
    }
    Ok(entries)
}

fn truncate_utf8(bytes: Vec<u8>, limit: usize) -> (String, bool) {
    let truncated = bytes.len() > limit;
    let mut slice = &bytes[..bytes.len().min(limit)];
    while std::str::from_utf8(slice).is_err() && !slice.is_empty() {
        slice = &slice[..slice.len() - 1];
    }
    (String::from_utf8_lossy(slice).to_string(), truncated)
}

fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn run_git(root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("无法运行 Git：{error}"))
}

#[tauri::command]
pub fn local_workspace_scan(root: String) -> ApiResponse<LocalWorkspaceSnapshot> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let entries = match list_directory(&root, &root, 0) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let name = root
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());
    ApiResponse::ok(LocalWorkspaceSnapshot {
        root: root.display().to_string(),
        name,
        entries,
        truncated: false,
    })
}

#[tauri::command]
pub fn local_workspace_list_directory(
    root: String,
    relative_path: String,
) -> ApiResponse<Vec<LocalWorkspaceEntry>> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let directory = match resolve_directory(&root, Some(&relative_path)) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let depth = Path::new(&relative_path).components().count();
    match list_directory(&root, &directory, depth) {
        Ok(entries) => ApiResponse::ok(entries),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub fn local_workspace_read(
    root: String,
    relative_path: String,
    full: Option<bool>,
) -> ApiResponse<LocalFilePreview> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let path = match resolve_file(&root, &relative_path) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let bytes = match fs::read(&path) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("无法读取文件：{error}")),
    };
    if bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return ApiResponse::err("二进制文件暂不支持预览".to_string());
    }
    let size = bytes.len() as u64;
    let file_fingerprint = fingerprint(&bytes);
    let load_full = full.unwrap_or(false);
    if load_full && bytes.len() > MAX_FULL_PREVIEW_BYTES {
        return ApiResponse::err("文件超过 8 MB，请使用外部编辑器打开".to_string());
    }
    let (content, truncated) = truncate_utf8(
        bytes,
        if load_full {
            MAX_FULL_PREVIEW_BYTES
        } else {
            MAX_PREVIEW_BYTES
        },
    );
    ApiResponse::ok(LocalFilePreview {
        path: relative_path,
        content,
        size,
        truncated,
        fingerprint: file_fingerprint,
    })
}

#[tauri::command]
pub fn local_workspace_preview_file(
    app: tauri::AppHandle,
    root: String,
    relative_path: String,
) -> ApiResponse<String> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let path = match resolve_file(&root, &relative_path) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    if let Err(error) = app.asset_protocol_scope().allow_file(&path) {
        return ApiResponse::err(format!("无法授权浏览器读取文件：{error}"));
    }
    ApiResponse::ok(path.display().to_string())
}

#[tauri::command]
pub fn local_workspace_write(
    root: String,
    relative_path: String,
    content: String,
    expected_fingerprint: String,
) -> ApiResponse<LocalFilePreview> {
    if content.len() > MAX_PREVIEW_BYTES {
        return ApiResponse::err("文件超过可编辑大小限制".to_string());
    }
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let path = match resolve_file(&root, &relative_path) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let current = match fs::read(&path) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("无法读取文件：{error}")),
    };
    if fingerprint(&current) != expected_fingerprint {
        return ApiResponse::err("文件已在外部发生变化，请刷新后重新编辑".to_string());
    }
    if let Err(error) = fs::write(&path, content.as_bytes()) {
        return ApiResponse::err(format!("保存文件失败：{error}"));
    }
    let bytes = content.into_bytes();
    let size = bytes.len() as u64;
    ApiResponse::ok(LocalFilePreview {
        path: relative_path,
        content: String::from_utf8_lossy(&bytes).to_string(),
        size,
        truncated: false,
        fingerprint: fingerprint(&bytes),
    })
}

#[tauri::command]
pub fn local_workspace_git_status(root: String) -> ApiResponse<LocalGitStatus> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let output = match run_git(
        &root,
        &[
            "status",
            "--porcelain=v1",
            "--branch",
            "--untracked-files=normal",
        ],
    ) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    if !output.status.success() {
        return ApiResponse::ok(LocalGitStatus {
            is_repo: false,
            branch: None,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut branch = None;
    let mut ahead = 0;
    let mut behind = 0;
    let mut changes = Vec::new();
    for line in stdout.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            let head = header.split("...").next().unwrap_or(header);
            branch = Some(head.split_whitespace().next().unwrap_or(head).to_string());
            if let Some(position) = header.find("ahead ") {
                ahead = header[position + 6..]
                    .split(|value: char| !value.is_ascii_digit())
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
            if let Some(position) = header.find("behind ") {
                behind = header[position + 7..]
                    .split(|value: char| !value.is_ascii_digit())
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let code = &line[..2];
        let path = line[3..]
            .split(" -> ")
            .last()
            .unwrap_or(&line[3..])
            .to_string();
        changes.push(LocalGitChange {
            path,
            status: code.trim().to_string(),
            staged: code.as_bytes().first().copied().unwrap_or(b' ') != b' ',
        });
    }
    ApiResponse::ok(LocalGitStatus {
        is_repo: true,
        branch,
        ahead,
        behind,
        changes,
    })
}

#[tauri::command]
pub fn local_workspace_git_diff(
    root: String,
    relative_path: Option<String>,
) -> ApiResponse<LocalGitDiff> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let safe_path = match relative_path.as_deref() {
        Some(value) => match safe_relative_path(value) {
            Ok(_) => Some(value),
            Err(error) => return ApiResponse::err(error),
        },
        None => None,
    };
    let mut unstaged_args = vec!["diff", "--no-ext-diff", "--no-color"];
    let mut staged_args = vec!["diff", "--cached", "--no-ext-diff", "--no-color"];
    if let Some(path) = safe_path {
        unstaged_args.extend(["--", path]);
        staged_args.extend(["--", path]);
    }
    let unstaged = match run_git(&root, &unstaged_args) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let staged = match run_git(&root, &staged_args) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    if !unstaged.status.success() || !staged.status.success() {
        return ApiResponse::err("无法读取 Git 变更".to_string());
    }
    let mut bytes = Vec::new();
    if !staged.stdout.is_empty() {
        bytes.extend_from_slice(b"# Staged changes\n\n");
        bytes.extend_from_slice(&staged.stdout);
    }
    if !unstaged.stdout.is_empty() {
        if !bytes.is_empty() {
            bytes.extend_from_slice(b"\n");
        }
        bytes.extend_from_slice(b"# Working tree changes\n\n");
        bytes.extend_from_slice(&unstaged.stdout);
    }
    let (content, truncated) = truncate_utf8(bytes, MAX_DIFF_BYTES);
    ApiResponse::ok(LocalGitDiff {
        path: relative_path,
        content,
        truncated,
    })
}

fn git_file_action(root: String, relative_path: String, action: &str) -> ApiResponse<()> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    if let Err(error) = safe_relative_path(&relative_path) {
        return ApiResponse::err(error);
    }
    let args = match action {
        "stage" => vec!["add", "--", relative_path.as_str()],
        "unstage" => vec!["restore", "--staged", "--", relative_path.as_str()],
        "discard" => vec!["restore", "--worktree", "--", relative_path.as_str()],
        _ => return ApiResponse::err("未知 Git 操作".to_string()),
    };
    let output = match run_git(&root, &args) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    if output.status.success() {
        ApiResponse::ok(())
    } else {
        ApiResponse::err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[tauri::command]
pub fn local_workspace_git_stage(root: String, relative_path: String) -> ApiResponse<()> {
    git_file_action(root, relative_path, "stage")
}

#[tauri::command]
pub fn local_workspace_git_unstage(root: String, relative_path: String) -> ApiResponse<()> {
    git_file_action(root, relative_path, "unstage")
}

#[tauri::command]
pub fn local_workspace_git_discard(root: String, relative_path: String) -> ApiResponse<()> {
    git_file_action(root, relative_path, "discard")
}

#[tauri::command]
pub async fn local_workspace_run_command(
    root: String,
    command: String,
) -> ApiResponse<LocalCommandResult> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let command = command.trim().to_string();
    if command.is_empty() || command.len() > 4_096 {
        return ApiResponse::err("命令不能为空且不能超过 4,096 个字符".to_string());
    }
    let child = tokio::process::Command::new("/bin/zsh")
        .arg("-lc")
        .arg(&command)
        .current_dir(&root)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(std::time::Duration::from_secs(60), child).await {
        Ok(Ok(output)) => {
            let mut bytes = output.stdout;
            if !output.stderr.is_empty() {
                if !bytes.is_empty() {
                    bytes.extend_from_slice(b"\n");
                }
                bytes.extend_from_slice(&output.stderr);
            }
            let (text, truncated) = truncate_utf8(bytes, MAX_DIFF_BYTES);
            ApiResponse::ok(LocalCommandResult {
                command,
                output: text,
                exit_code: output.status.code(),
                timed_out: false,
                truncated,
            })
        }
        Ok(Err(error)) => ApiResponse::err(format!("命令执行失败：{error}")),
        Err(_) => ApiResponse::ok(LocalCommandResult {
            command,
            output: "命令运行超过 60 秒，已终止。长驻服务请交给 Agent 以后台进程启动。".to_string(),
            exit_code: None,
            timed_out: true,
            truncated: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{list_directory, safe_relative_path};
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn accepts_project_relative_file() {
        assert!(safe_relative_path("src/components/App.tsx").is_ok());
    }

    #[test]
    fn rejects_parent_escape() {
        assert!(safe_relative_path("../secret.txt").is_err());
    }

    #[test]
    fn lists_large_directories_without_a_global_item_cap() {
        let root = std::env::temp_dir().join(format!("sophonote-directory-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp directory");
        for index in 0..1_205 {
            fs::write(root.join(format!("file-{index:04}.txt")), b"").expect("create fixture");
        }
        let entries = list_directory(&root, &root, 0).expect("list directory");
        assert_eq!(entries.len(), 1_205);
        fs::remove_dir_all(root).expect("remove temp directory");
    }
}
