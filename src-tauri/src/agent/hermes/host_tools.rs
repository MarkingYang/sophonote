//! Hermes receives a Run-scoped Host tool surface, never a native write grant.
use crate::agent::{attachments::RunAttachmentKind, commands::AgentRunStartArgs, pi::tools::Scope};
use tauri::Manager;

pub(crate) fn prepare(
    app: &tauri::AppHandle,
    request: &AgentRunStartArgs,
) -> Result<Scope, String> {
    let layout = crate::storage_layout::StorageLayout::resolve(app)?;
    let workspace = request
        .workspace_root
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or(layout.workspace.clone())
        .canonicalize()
        .map_err(|_| "工作区路径不可访问")?;
    if !workspace.is_dir() {
        return Err("工作区路径不是目录".into());
    }
    let mode = request
        .workspace_permission_mode
        .clone()
        .unwrap_or("ask".into());
    if !matches!(mode.as_str(), "ask" | "autoEdit" | "plan") {
        return Err("工作区权限模式无效".into());
    }
    // Native slash/Skill expansion can execute shell hooks before the tool loop.
    if request.hermes_command.is_some()
        || request.skill.is_some()
        || request.message.trim_start().starts_with('/')
    {
        return Err("当前原生命令尚未接入宿主权限检查，请使用普通任务描述".into());
    }
    let mut protected = vec![
        layout.notes,
        layout.database,
        layout.logs,
        layout.runtime,
        layout.hermes,
        layout.version,
        layout.root.join("sophonote.db-wal"),
        layout.root.join("sophonote.db-shm"),
        layout.root.join("pi"),
        layout.root.join("claude_code"),
        layout.root.join("opencode"),
    ];
    if let Ok(resource) = app.path().resource_dir() {
        protected.push(resource);
    }
    let protected: Vec<_> = protected
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    if protected.iter().any(|p| workspace.starts_with(p)) {
        return Err("不能将受保护的应用目录作为工作区".into());
    }
    let mut scope = Scope {
        read_lease: None,
        readonly: None,
        workspace,
        read_paths: vec![],
        workcopy: None,
        protected,
        mode,
    };
    for attachment in &request.attachments {
        if matches!(
            attachment.kind,
            RunAttachmentKind::File | RunAttachmentKind::Folder | RunAttachmentKind::Image
        ) {
            if let Some(path) = &attachment.path {
                let path = std::path::PathBuf::from(path)
                    .canonicalize()
                    .map_err(|_| "附件路径不可访问")?;
                if scope.protected.iter().any(|p| path.starts_with(p)) {
                    return Err("附件位于受保护的应用目录".into());
                }
                scope.read_paths.push(path);
            }
        }
    }
    Ok(scope)
}
