//! DEC-053: shared Pi / Claude Code Run preparation and persistence.
//! Native transports are separate; tool effects and approvals stay in Rust.
pub(crate) mod runtime;
pub(crate) mod tools;
pub(crate) mod transport;
pub(crate) mod update;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use super::commands::{
    AgentRunStartArgs, AgentRunStartResult, ChannelTransport, HermesModelOptions,
    HermesModelProvider,
};
use super::engine::{HermesFocusDocument, HermesSessionBinding};
use super::events::{AgentEvent, AgentEventPayload, EventEmitter, EventTransport};
use super::hermes::attached_engine::{self, HostPatchContext, WorkingCopyTarget};
use super::store::{DurableFirstTransport, RunStore, RunStoreTransport};
use super::types::{RunStatus, ThreadStatus};
use crate::commands::ApiResponse;
use crate::model::openai_compat::{configured_provider_snapshots, ProviderSnapshot};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn resolve_request_engine(
    app: &AppHandle,
    request: &AgentRunStartArgs,
) -> Result<String, String> {
    let conn =
        rusqlite::Connection::open(crate::db::get_db_path(app)).map_err(|e| e.to_string())?;
    let thread = match &request.thread_id {
        Some(id) => Some(
            RunStore::new(conn)
                .get_thread(id)
                .map_err(|e| e.to_string())?
                .ok_or("会话不存在")?,
        ),
        None => None,
    };
    resolve_engine(request.engine.as_deref(), thread.as_ref())
}

fn resolve_engine(
    requested: Option<&str>,
    thread: Option<&super::types::AgentThread>,
) -> Result<String, String> {
    let engine = requested.unwrap_or_else(|| thread.map_or("hermes", |t| t.engine.as_str()));
    if !matches!(engine, "hermes" | "pi" | "claude_code") {
        return Err("未知智能体引擎".into());
    }
    if thread.is_some_and(|t| t.latest_run_id.is_some() && t.engine != engine) {
        return Err("已有会话的智能体引擎不能切换，请新建会话".into());
    }
    Ok(engine.into())
}

fn supported(provider: &ProviderSnapshot) -> bool {
    matches!(provider.protocol.as_str(), "openai" | "anthropic")
}

#[tauri::command]
pub async fn agent_pi_models(app: AppHandle) -> ApiResponse<HermesModelOptions> {
    model_options(&app, false)
}

pub(crate) fn model_options(app: &AppHandle, claude: bool) -> ApiResponse<HermesModelOptions> {
    let result = (|| {
        let providers = configured_provider_snapshots(app)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter_map(|p| {
                if claude {
                    super::claude::provider_snapshot(p)
                } else {
                    supported(&p).then_some(p)
                }
            })
            .map(|p| HermesModelProvider {
                slug: p.id.clone(),
                name: p.id,
                models: p.models,
                authenticated: None,
                is_current: None,
            })
            .collect::<Vec<_>>();
        let active = crate::model::openai_compat::resolve_provider(app, None).ok();
        let provider = active
            .as_ref()
            .and_then(|p| providers.iter().find(|row| row.slug == p.id))
            .or(providers.first());
        Ok(HermesModelOptions {
            provider: provider.map(|p| p.slug.clone()),
            model: provider.and_then(|p| p.models.first()).cloned(),
            providers,
        })
    })();
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_pi_status(app: AppHandle) -> ApiResponse<Value> {
    let handle = app.clone();
    let result =
        tokio::task::spawn_blocking(move || update::resolve(&handle).map(|(runtime, _)| runtime))
            .await
            .map_err(|_| "Pi 校验任务失败".to_string())
            .and_then(|r| r);
    ApiResponse::ok(
        json!({"available":result.is_ok(), "version":result.as_ref().map(|r| r.version.as_str()).unwrap_or(runtime::VERSION),
        "error":result.err(), "commands":cfg!(target_os="macos"), "browser":false, "mcp":false}),
    )
}

pub(crate) enum ExecutionRuntime {
    Pi(runtime::Runtime),
    Claude(super::claude::runtime::Runtime),
}
impl ExecutionRuntime {
    fn engine(&self) -> &'static str {
        match self {
            Self::Pi(_) => "pi",
            Self::Claude(_) => "claude_code",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Pi(_) => "Pi",
            Self::Claude(_) => "Claude Code",
        }
    }
    fn version(&self) -> &str {
        match self {
            Self::Pi(r) => &r.version,
            Self::Claude(r) => &r.version,
        }
    }
    fn protected_root(&self) -> PathBuf {
        match self {
            Self::Pi(r) => r.extension.parent().unwrap_or(Path::new(".")).to_path_buf(),
            Self::Claude(r) => r.executable.clone(),
        }
    }
}

pub(crate) struct Prepared {
    pub(crate) runtime: ExecutionRuntime,
    pub(crate) provider: ProviderSnapshot,
    pub(crate) model: String,
    pub(crate) key: String,
    pub(crate) home: PathBuf,
    pub(crate) session: PathBuf,
    pub(crate) context: String,
    pub(crate) prompt: String,
    pub(crate) images: Vec<Value>,
    pub(crate) scope: tools::Scope,
    pub(crate) patch: Option<HostPatchContext>,
    pub(crate) max_turns: usize,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(path) = &self.scope.workcopy {
            let _ = std::fs::remove_file(path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("创建智能体私有目录失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn stable_id(thread_id: &str) -> String {
    format!("{:x}", Sha256::digest(thread_id.as_bytes()))
}

fn prepare(
    app: &AppHandle,
    request: &AgentRunStartArgs,
    binding: &HermesSessionBinding,
    runtime: ExecutionRuntime,
) -> Result<Prepared, String> {
    let engine = runtime.engine();
    let label = runtime.label();
    super::attachments::validate_surface_attachments(&request.message, &request.attachments)?;
    if request.message.len() > 1024 * 1024 {
        return Err("消息超过 1 MiB".into());
    }
    if request.message.trim_start().starts_with('/') {
        return Err(format!(
            "{label} 首版只接受普通消息；请通过界面新建会话、选模型或停止运行"
        ));
    }
    if request.skill.is_some() || request.hermes_command.is_some() {
        return Err(format!(
            "{label} 会话不支持 Hermes 的 Skill 或命令，请移除后重试"
        ));
    }
    if request.include_project_context {
        return Err(format!(
            "{label} 通过本地工作目录处理项目，请移除项目资料范围并选择工作目录"
        ));
    }
    let provider =
        crate::model::openai_compat::resolve_provider(app, request.hermes_provider.as_deref())
            .map_err(|e| e.to_string())?;
    let provider = if engine == "claude_code" {
        super::claude::provider_snapshot(provider)
            .ok_or("Claude Code 需要 Anthropic 协议或官方 DeepSeek 供应商，请检查模型设置")?
    } else {
        provider
    };
    if !supported(&provider) {
        return Err("Pi 当前支持 OpenAI 兼容与 Anthropic 供应商".into());
    }
    let model = request
        .hermes_model
        .clone()
        .unwrap_or_else(|| provider.model.clone());
    if !provider.models.contains(&model) {
        return Err(format!("所选 {label} 模型不在供应商配置白名单中"));
    }
    let key = crate::commands::get_cached_api_key(app, &provider.id)?;
    if provider.requires_key && key.is_empty() {
        return Err("请先在 AI 模型设置中配置此供应商的 API Key".into());
    }
    let layout = crate::storage_layout::StorageLayout::resolve(app)?;
    let home = if engine == "claude_code" {
        layout
            .root
            .join(engine)
            .join("sessions")
            .join(stable_id(&binding.thread_id))
    } else {
        layout.root.join("pi").join("agent")
    };
    let sessions = layout.root.join(engine).join("sessions");
    let staging = layout
        .runtime
        .join(format!("{engine}-workcopies"))
        .join(&binding.run_id);
    for dir in [&home, &sessions, &layout.workspace] {
        private_dir(dir)?;
    }
    let workspace = request
        .workspace_root
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or(layout.workspace.clone())
        .canonicalize()
        .map_err(|_| format!("{label} 工作目录不可访问"))?;
    if !workspace.is_dir() {
        return Err(format!("{label} 工作路径不是目录"));
    }
    let mode = request
        .workspace_permission_mode
        .clone()
        .unwrap_or("ask".into());
    if !matches!(mode.as_str(), "ask" | "autoEdit" | "plan") {
        return Err("工作区权限模式无效".into());
    }
    let protected = [
        layout.notes.clone(),
        layout.database.clone(),
        layout.root.join("sophonote.db-wal"),
        layout.root.join("sophonote.db-shm"),
        layout.logs.clone(),
        layout.runtime.clone(),
        layout.hermes.clone(),
        layout.root.join("pi"),
        layout.root.join("claude_code"),
        layout.version.clone(),
        runtime.protected_root(),
    ]
    .into_iter()
    .map(|p| p.canonicalize().unwrap_or(p))
    .collect::<Vec<_>>();
    if protected.iter().any(|root| workspace.starts_with(root)) {
        return Err(format!("不能将受保护的应用目录作为 {label} 工作区"));
    }
    let mut scope = tools::Scope {
        workspace,
        read_paths: vec![],
        workcopy: None,
        protected,
        mode,
    };
    let mut context = Vec::new();
    let target = if let Some(selection) = &request.selection {
        Some(WorkingCopyTarget::Selection(super::events::RunContext {
            article_id: selection.article_id.clone(),
            title: selection.title.clone(),
            base_version: selection.base_version,
            selected_markdown: selection.selected_markdown.clone(),
            selected_text_hash: selection.selected_text_hash.clone(),
            before_context: selection.before_context.clone(),
            after_context: selection.after_context.clone(),
        }))
    } else {
        request
            .focus_document
            .as_ref()
            .map(|doc| {
                doc.markdown
                    .clone()
                    .map(|markdown| {
                        WorkingCopyTarget::Document(HermesFocusDocument {
                            article_id: doc.article_id.clone(),
                            title: doc.title.clone(),
                            base_version: doc.base_version,
                            markdown,
                        })
                    })
                    .ok_or("当前文档缺少发送时正文快照")
            })
            .transpose()?
    };
    let patch = if let Some(target) = target {
        let body = match &target {
            WorkingCopyTarget::Document(doc) => {
                attached_engine::focus_document_attachment_markdown(doc, None)
            }
            WorkingCopyTarget::Selection(selection) => {
                attached_engine::selection_attachment_markdown(selection)
            }
            _ => unreachable!(),
        };
        if body.len() as u64 > tools::FILE_LIMIT {
            return Err("文档范围超过 1 MiB".into());
        }
        private_dir(&staging)?;
        let path = staging.join("document.md");
        std::fs::write(&path, body).map_err(|_| "无法创建文档工作副本")?;
        let path = path.canonicalize().map_err(|_| "无法解析文档工作副本")?;
        scope.workcopy = Some(path.clone());
        context.push(json!({"kind":"document-working-copy","path":path,"writeback":"修改可编辑标记内正文并保留标记，宿主会生成待审阅的笔记 Diff；原笔记不可直接写入"}));
        attached_engine::host_patch_context_for_attachment(Some(binding), path, target)
            .map(|context| context.for_sidecar(engine))
    } else {
        None
    };
    let mut images = vec![];
    for attachment in &request.attachments {
        use super::attachments::RunAttachmentKind;
        match attachment.kind {
            RunAttachmentKind::Image => {
                let (mime, encoded) = if let Some(url) = &attachment.data_url {
                    let (header, encoded) = url.split_once(',').ok_or("图片 data URL 无效")?;
                    (
                        header
                            .trim_start_matches("data:")
                            .trim_end_matches(";base64")
                            .to_string(),
                        encoded.to_string(),
                    )
                } else {
                    let path = Path::new(attachment.path.as_deref().ok_or("图片路径缺失")?)
                        .canonicalize()
                        .map_err(|_| "图片路径不可访问")?;
                    scope.read_paths.push(path.clone());
                    scope.path(path.to_str().ok_or("图片路径非 UTF-8")?, false)?;
                    let mime = match path
                        .extension()
                        .and_then(|p| p.to_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .as_str()
                    {
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "webp" => "image/webp",
                        _ => "image/png",
                    };
                    use std::io::Read;
                    let mut data = Vec::new();
                    std::fs::File::open(path)
                        .map_err(|_| "读取图片失败")?
                        .take(10 * 1024 * 1024 + 1)
                        .read_to_end(&mut data)
                        .map_err(|_| "读取图片失败")?;
                    if data.len() > 10 * 1024 * 1024 {
                        return Err("图片超过 10 MiB".into());
                    }
                    (
                        mime.to_string(),
                        base64::engine::general_purpose::STANDARD.encode(data),
                    )
                };
                images.push(json!({"type":"image","mimeType":mime,"data":encoded}));
            }
            RunAttachmentKind::File | RunAttachmentKind::Folder => {
                let path = Path::new(attachment.path.as_deref().ok_or("附件路径缺失")?)
                    .canonicalize()
                    .map_err(|_| "附件不可访问")?;
                scope.read_paths.push(path.clone());
                scope.path(path.to_str().ok_or("附件路径非 UTF-8")?, false)?;
                context.push(json!({"kind":"attachment","path":path}));
            }
            RunAttachmentKind::Url => context.push(json!({"kind":"url","url":attachment.url})),
        }
    }
    // The private agent home has no user-installed packages or credentials; these settings are host-owned.
    if engine == "pi" {
        std::fs::write(
            home.join("settings.json"),
            "{\"retry\":{\"enabled\":false},\"quietStartup\":true}\n",
        )
        .map_err(|_| "无法写入 Pi 行为配置")?;
    }
    let session = if engine == "claude_code" {
        super::claude::session_path(&home, &binding.thread_id)
    } else {
        sessions.join(format!("{}.jsonl", stable_id(&binding.thread_id)))
    };
    Ok(Prepared {
        runtime,
        provider,
        model,
        key,
        home,
        session,
        context: if context.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&context).map_err(|e| e.to_string())?
        },
        prompt: request.message.clone(),
        images,
        scope,
        patch,
        max_turns: request.max_turns.unwrap_or(20).clamp(1, 40),
    })
}

fn claim_run(
    conn: &mut rusqlite::Connection,
    request: &AgentRunStartArgs,
    binding: &HermesSessionBinding,
    provider: &str,
    model: &str,
    max_turns: usize,
    runtime: &ExecutionRuntime,
) -> Result<(), String> {
    let engine = runtime.engine();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let existing: Option<(String, Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT engine, latest_run_id, project_id FROM agent_threads WHERE id=?1",
            [&binding.thread_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if request.thread_id.is_some() && existing.is_none() {
        return Err("会话不存在".into());
    }
    if let Some((engine, latest, project)) = &existing {
        if latest.is_some() && engine != runtime.engine() {
            return Err("已有会话不能切换引擎".into());
        }
        if request.project_id.is_some() && request.project_id != *project {
            return Err("会话不属于所选项目".into());
        }
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE thread_id=?1 AND lower(trim(status,'\"')) IN ('queued','running','waiting_approval'))",
            [&binding.thread_id], |row| row.get(0)).map_err(|e| e.to_string())?;
        if active {
            return Err("此会话上一轮尚未结束或需要恢复".into());
        }
    } else {
        tx.execute("INSERT INTO agent_threads (id,title,status,project_id,created_at,updated_at,engine) VALUES (?1,'新会话','\"running\"',?2,?3,?3,?4)",
            rusqlite::params![binding.thread_id, binding.project_id, now_ms(), engine]).map_err(|e| e.to_string())?;
    }
    tx.execute("INSERT INTO agent_runs (id,thread_id,project_id,status,provider,model,prompt_version,max_model_calls,current_model_calls,engine,engine_version,created_at,updated_at) VALUES (?1,?2,?3,'\"running\"',?4,?5,?9,?6,0,?10,?7,?8,?8)",
        rusqlite::params![binding.run_id, binding.thread_id, binding.project_id, provider, model, max_turns, runtime.version(), now_ms(), format!("{engine}-rpc-v1"), engine]).map_err(|e| e.to_string())?;
    tx.execute("UPDATE agent_threads SET engine=?5,latest_run_id=?1,external_session_id=?2,status='\"running\"',updated_at=?3 WHERE id=?4",
        rusqlite::params![binding.run_id,format!("{engine}:{}",stable_id(&binding.thread_id)),now_ms(),binding.thread_id,engine]).map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO agent_messages (id,thread_id,run_id,role,content,source,created_at) VALUES (?1,?2,?3,'user',?4,?6,?5)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(),binding.thread_id,binding.run_id,request.message,now_ms(),engine]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

pub async fn start(
    app: AppHandle,
    request: AgentRunStartArgs,
    channel: tauri::ipc::Channel<AgentEvent>,
) -> ApiResponse<AgentRunStartResult> {
    start_engine(app, request, channel, "pi").await
}

pub(crate) async fn start_engine(
    app: AppHandle,
    request: AgentRunStartArgs,
    channel: tauri::ipc::Channel<AgentEvent>,
    engine: &'static str,
) -> ApiResponse<AgentRunStartResult> {
    let result = async {
        let _update_guard = if engine == "claude_code" {
            Some(
                super::runtime_updates::CLAUDE_GATE
                    .try_lock()
                    .map_err(|_| "Claude Code 正在更新，请完成后重试")?,
            )
        } else {
            None
        };
        let runtime = if engine == "claude_code" {
            ExecutionRuntime::Claude(super::claude::runtime::locate().await?)
        } else {
            let handle = app.clone();
            let runtime = tokio::task::spawn_blocking(move || {
                update::resolve(&handle).map(|(runtime, _)| runtime)
            })
            .await
            .map_err(|_| "Pi 校验任务失败")??;
            ExecutionRuntime::Pi(runtime)
        };
        let label = runtime.label();
        let db_path = crate::db::get_db_path(&app);
        let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
        let project_id = if let Some(id) = &request.thread_id {
            conn.query_row(
                "SELECT project_id FROM agent_threads WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|_| "会话不存在")?
        } else {
            request.project_id.clone()
        };
        let binding = HermesSessionBinding {
            db_path,
            notes_dir: crate::notes::notes_dir(&app),
            project_id,
            thread_id: request
                .thread_id
                .clone()
                .unwrap_or_else(|| format!("thread-{}", uuid::Uuid::new_v4())),
            run_id: format!("run-{}", uuid::Uuid::new_v4()),
        };
        let prepared = prepare(&app, &request, &binding, runtime)?;
        let has_history: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_messages WHERE thread_id=?1 AND role='assistant')",
            [&binding.thread_id], |row| row.get(0)).map_err(|e| e.to_string())?;
        if has_history && !prepared.session.is_file() {
            return Err(format!(
                "{label} 会话文件缺失，无法恢复原历史；请恢复应用数据或新建会话"
            ));
        }
        claim_run(
            &mut conn,
            &request,
            &binding,
            &prepared.provider.id,
            &prepared.model,
            prepared.max_turns,
            &prepared.runtime,
        )?;
        let store = Arc::new(RunStoreTransport::new(
            binding.db_path.to_string_lossy().into_owned(),
        ));
        let transport = Arc::new(DurableFirstTransport::new(
            store,
            vec![Arc::new(ChannelTransport { channel }) as Arc<dyn EventTransport>],
        ));
        let events = Arc::new(EventEmitter::new(
            &binding.thread_id,
            &binding.run_id,
            transport,
        ));
        let context = request
            .selection
            .as_ref()
            .map(|s| super::events::RunContext {
                article_id: s.article_id.clone(),
                title: s.title.clone(),
                base_version: s.base_version,
                selected_markdown: s.selected_markdown.clone(),
                selected_text_hash: s.selected_text_hash.clone(),
                before_context: s.before_context.clone(),
                after_context: s.after_context.clone(),
            })
            .or_else(|| {
                request
                    .focus_document
                    .as_ref()
                    .map(|d| super::events::RunContext {
                        article_id: d.article_id.clone(),
                        title: d.title.clone(),
                        base_version: d.base_version,
                        selected_markdown: String::new(),
                        selected_text_hash: String::new(),
                        before_context: String::new(),
                        after_context: String::new(),
                    })
            });
        let cancel = CancellationToken::new();
        super::commands::global_cancel_registry().register(&binding.run_id, cancel.clone());
        let response = AgentRunStartResult {
            thread_id: binding.thread_id.clone(),
            run_id: binding.run_id.clone(),
        };
        tauri::async_runtime::spawn(async move {
            let outcome = async {
                events
                    .emit(AgentEventPayload::RunStarted {
                        user_message: request.message,
                        max_turns: prepared.max_turns,
                        context,
                        skill: None,
                    })
                    .map_err(|e| e.to_string())?;
                match &prepared.runtime {
                    ExecutionRuntime::Pi(_) => {
                        transport::run(&prepared, &binding, &events, &cancel).await
                    }
                    ExecutionRuntime::Claude(_) => {
                        super::claude::transport::run(&prepared, &binding, &events, &cancel).await
                    }
                }
            }
            .await;
            let (run_status, thread_status) = if cancel.is_cancelled() {
                let _ = events.emit(AgentEventPayload::RunCancelled {
                    reason: "用户取消".into(),
                });
                (RunStatus::Cancelled, ThreadStatus::Cancelled)
            } else {
                match outcome {
                    Ok((answer, turns)) => {
                        if let Some(patch) = &prepared.patch {
                            attached_engine::emit_host_patch(patch, Some(&events));
                        }
                        let _ = events.emit(AgentEventPayload::MessageCompleted {
                            text: answer.clone(),
                        });
                        if let Ok(conn) = rusqlite::Connection::open(&binding.db_path) {
                            let store = RunStore::new(conn);
                            let _ = store.save_message(
                                &format!("msg-{}", uuid::Uuid::new_v4()),
                                &binding.thread_id,
                                &binding.run_id,
                                "assistant",
                                &answer,
                                now_ms(),
                            );
                            let _ = store.set_model_calls(&binding.run_id, turns, now_ms());
                            let _ = store
                                .refresh_thread_title_from_messages(&binding.thread_id, now_ms());
                        }
                        let _ = events.emit(AgentEventPayload::RunCompleted {
                            outcome: "completed".into(),
                            final_answer: answer,
                            model_calls: turns,
                        });
                        (RunStatus::Completed, ThreadStatus::Completed)
                    }
                    Err(error) => {
                        let error = if prepared.key.is_empty() {
                            error
                        } else {
                            error.replace(&prepared.key, "[凭据已隐藏]")
                        };
                        let _ = events.emit(AgentEventPayload::RunFailed {
                            outcome: "failed".into(),
                            error,
                        });
                        (RunStatus::Failed, ThreadStatus::Failed)
                    }
                }
            };
            if let Ok(conn) = rusqlite::Connection::open(&binding.db_path) {
                let store = RunStore::new(conn);
                let _ = store.update_run_status(&binding.run_id, &run_status, now_ms());
                let _ = store.update_thread_status(&binding.thread_id, &thread_status, now_ms());
            }
            super::hermes::gateway_client::unregister_run_control(&binding.run_id);
            super::commands::global_cancel_registry().remove(&binding.run_id);
        });
        Ok(response)
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_threads_are_engine_bound_and_legacy_threads_default_to_hermes() {
        let mut thread = super::super::types::AgentThread::new("t".into(), "".into(), None, 0);
        assert_eq!(resolve_engine(None, Some(&thread)).unwrap(), "hermes");
        assert_eq!(resolve_engine(Some("pi"), Some(&thread)).unwrap(), "pi");
        thread.latest_run_id = Some("r".into());
        assert!(resolve_engine(Some("pi"), Some(&thread)).is_err());
        thread.engine = "pi".into();
        assert_eq!(resolve_engine(None, Some(&thread)).unwrap(), "pi");
        assert!(resolve_engine(Some("hermes"), Some(&thread)).is_err());
        assert!(resolve_engine(Some("claude_code"), Some(&thread)).is_err());
        assert_eq!(
            resolve_engine(Some("claude_code"), None).unwrap(),
            "claude_code"
        );
        thread.engine = "claude_code".into();
        assert_eq!(resolve_engine(None, Some(&thread)).unwrap(), "claude_code");
        assert!(resolve_engine(Some("pi"), Some(&thread)).is_err());
        assert!(resolve_engine(Some("unknown"), None).is_err());
    }

    #[test]
    fn claude_claim_is_atomic_engine_bound_and_records_native_version() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_threads (id TEXT PRIMARY KEY,title TEXT,status TEXT,project_id TEXT,created_at INTEGER,updated_at INTEGER,engine TEXT,latest_run_id TEXT,external_session_id TEXT);
            CREATE TABLE agent_runs (id TEXT PRIMARY KEY,thread_id TEXT,project_id TEXT,status TEXT,provider TEXT,model TEXT,prompt_version TEXT,max_model_calls INTEGER,current_model_calls INTEGER,engine TEXT,engine_version TEXT,created_at INTEGER,updated_at INTEGER);
            CREATE TABLE agent_messages (id TEXT PRIMARY KEY,thread_id TEXT,run_id TEXT,role TEXT,content TEXT,source TEXT,created_at INTEGER);").unwrap();
        let mut request: AgentRunStartArgs =
            serde_json::from_value(json!({"message":"fixture","engine":"claude_code"})).unwrap();
        let mut binding = HermesSessionBinding {
            db_path: PathBuf::new(),
            notes_dir: PathBuf::new(),
            project_id: None,
            thread_id: "thread".into(),
            run_id: "first".into(),
        };
        let runtime = ExecutionRuntime::Claude(super::super::claude::runtime::Runtime {
            executable: PathBuf::from("/unused"),
            version: "2.1.233".into(),
        });
        claim_run(
            &mut conn, &request, &binding, "provider", "model", 6, &runtime,
        )
        .unwrap();
        let row: (String, String, String) = conn.query_row("SELECT t.engine,r.engine,r.engine_version FROM agent_threads t JOIN agent_runs r ON r.thread_id=t.id", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            row,
            ("claude_code".into(), "claude_code".into(), "2.1.233".into())
        );
        request.thread_id = Some(binding.thread_id.clone());
        binding.run_id = "second".into();
        assert!(
            claim_run(&mut conn, &request, &binding, "provider", "model", 6, &runtime).is_err()
        );
        conn.execute("UPDATE agent_runs SET status='completed'", [])
            .unwrap();
        let pi = ExecutionRuntime::Pi(runtime::Runtime {
            version: runtime::VERSION.into(),
            executable: PathBuf::new(),
            extension: PathBuf::new(),
        });
        assert!(claim_run(&mut conn, &request, &binding, "provider", "model", 6, &pi).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        claim_run(
            &mut conn, &request, &binding, "provider", "model", 6, &runtime,
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    #[ignore = "requires pnpm pi:bundle; only uses a local fixture, never a real provider"]
    async fn native_pi_roundtrip_approval_history_and_plan_denial() {
        use std::sync::Mutex;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        struct Capture(Mutex<Vec<AgentEvent>>);
        impl EventTransport for Capture {
            fn send(&self, event: AgentEvent) -> Result<(), String> {
                if matches!(event.payload, AgentEventPayload::ApprovalRequired { .. }) {
                    super::super::hermes::gateway_client::send_run_control(
                        &event.run_id,
                        super::super::hermes::gateway_client::GatewayControl::Approval {
                            choice: "once".into(),
                            all: false,
                        },
                    );
                }
                self.0.lock().unwrap().push(event);
                Ok(())
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("pi-native-{}", uuid::Uuid::new_v4()));
        private_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        private_dir(&root.join("agent")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let (cancel_ready, cancel_wait) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut cancel_ready = Some(cancel_ready);
            let mut requests = vec![];
            for index in 0..7 {
                let (mut stream, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(20), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut data = vec![];
                let mut buffer = [0; 4096];
                let request = loop {
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&buffer[..n]);
                    if let Some(at) = data.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..at]).to_lowercase();
                        assert!(headers
                            .lines()
                            .any(|line| line == "authorization: bearer fixture-secret"));
                        let size: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if data.len() >= at + 4 + size {
                            break serde_json::from_slice::<Value>(&data[at + 4..at + 4 + size])
                                .unwrap();
                        }
                    }
                };
                requests.push(request);
                if index == 5 {
                    let body = r#"{"error":{"message":"fixture provider rejection","type":"invalid_request_error"}}"#;
                    stream.write_all(format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                    continue;
                }
                if index == 6 {
                    cancel_ready.take().unwrap().send(()).unwrap();
                    let mut sink = [0u8; 1];
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        stream.read(&mut sink),
                    )
                    .await;
                    continue;
                }
                let tool = index == 0 || index == 3;
                let delta = if tool {
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("call-{index}"),"type":"function","function":{"name":"write","arguments":json!({"path":if index==0 {"result.txt"} else {"denied.txt"},"content":"Pi fixture"}).to_string()}}]})
                } else {
                    json!({"role":"assistant","content":"Pi fixture complete"})
                };
                let body = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                    json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":if tool {"tool_calls"} else {"stop"}}]})
                );
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        let runtime = runtime::verify(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/pi")
                .join(runtime::target()),
        )
        .unwrap();
        let mut prepared = Prepared {
            runtime: ExecutionRuntime::Pi(runtime),
            provider: ProviderSnapshot {
                id: "fixture".into(),
                protocol: "openai".into(),
                base_url: endpoint,
                model: "fixture".into(),
                models: vec!["fixture".into()],
                requires_key: false,
            },
            model: "fixture".into(),
            key: "fixture-secret".into(),
            home: root.join("agent"),
            session: root.join("session.jsonl"),
            context: String::new(),
            prompt: "First fixture request".into(),
            images: vec![],
            scope: tools::Scope {
                workspace: root.clone(),
                read_paths: vec![],
                workcopy: None,
                protected: vec![root.join("agent")],
                mode: "ask".into(),
            },
            patch: None,
            max_turns: 6,
        };
        let binding = HermesSessionBinding {
            db_path: root.join("unused.db"),
            notes_dir: root.join("notes"),
            project_id: None,
            thread_id: "fixture-thread".into(),
            run_id: format!("fixture-{}", uuid::Uuid::new_v4()),
        };
        let capture = Arc::new(Capture(Mutex::new(vec![])));
        let events = Arc::new(EventEmitter::new(
            &binding.thread_id,
            &binding.run_id,
            capture.clone(),
        ));
        let result = transport::run(&prepared, &binding, &events, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.0, "Pi fixture complete");
        assert_eq!(result.1, 2);
        assert_eq!(
            std::fs::read_to_string(root.join("result.txt")).unwrap(),
            "Pi fixture"
        );
        assert!(capture
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e.payload, AgentEventPayload::ApprovalRequired { .. })));
        assert!(prepared.session.is_file());
        prepared.prompt = "Second fixture request".into();
        transport::run(&prepared, &binding, &events, &CancellationToken::new())
            .await
            .unwrap();
        prepared.scope.mode = "plan".into();
        prepared.prompt = "Plan-only fixture request".into();
        transport::run(&prepared, &binding, &events, &CancellationToken::new())
            .await
            .unwrap();
        assert!(!root.join("denied.txt").exists());
        prepared.prompt = "Provider rejection fixture".into();
        let failure = transport::run(&prepared, &binding, &events, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(failure.contains("fixture provider rejection"), "{failure}");
        prepared.prompt = "Cancellation fixture".into();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let cancellation = tokio::spawn(async move {
            cancel_wait.await.unwrap();
            trigger.cancel();
        });
        let failure = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            transport::run(&prepared, &binding, &events, &cancel),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(failure.contains("已取消"));
        cancellation.await.unwrap();
        let requests = server.await.unwrap();
        assert!(requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "user"
                && m["content"].to_string().contains("First fixture request")));
        assert!(requests[4].to_string().contains("计划模式禁止"));
        for path in [
            prepared.session.clone(),
            prepared.home.join("auth.json"),
            prepared.home.join("models.json"),
        ] {
            if path.is_file() {
                assert!(!std::fs::read_to_string(path)
                    .unwrap()
                    .contains("fixture-secret"));
            }
        }
        super::super::hermes::gateway_client::unregister_run_control(&binding.run_id);
        std::fs::remove_dir_all(root).unwrap();
    }
}
