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

#[cfg(test)]
fn resolve_engine(
    requested: Option<&str>,
    thread: Option<&super::types::AgentThread>,
) -> Result<String, String> {
    super::routing::resolve_binding(
        requested,
        thread.map(|t| t.engine.as_str()),
        thread.is_some_and(|t| t.latest_run_id.is_some()),
    )
    .map(|route| route.engine().as_str().into())
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
    OpenCode(super::opencode::runtime::Runtime),
}
impl ExecutionRuntime {
    fn engine(&self) -> &'static str {
        match self {
            Self::Pi(_) => "pi",
            Self::Claude(_) => "claude_code",
            Self::OpenCode(_) => "opencode",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Pi(_) => "Pi",
            Self::Claude(_) => "Claude Code",
            Self::OpenCode(_) => "OpenCode",
        }
    }
    pub(crate) fn version(&self) -> &str {
        match self {
            Self::Pi(r) => &r.version,
            Self::Claude(r) => &r.version,
            Self::OpenCode(r) => &r.version,
        }
    }
    fn protected_root(&self) -> PathBuf {
        match self {
            Self::Pi(r) => r.extension.parent().unwrap_or(Path::new(".")).to_path_buf(),
            Self::Claude(r) => r.executable.clone(),
            Self::OpenCode(r) => r
                .executable
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf(),
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
    let super::model_preparation::PreparedModel {
        provider,
        model,
        key,
    } = super::model_preparation::prepare(
        app,
        super::registry::EngineId::parse(engine)?,
        request.hermes_provider.as_deref(),
        request.hermes_model.as_deref(),
    )?;
    let layout = crate::storage_layout::StorageLayout::resolve(app)?;
    let home = if matches!(engine, "claude_code" | "opencode") {
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
        layout.root.join("opencode"),
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
        read_lease: None,
        readonly: None,
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
    } else if engine == "opencode" {
        home.join("session-id")
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

struct ThreadClaim {
    engine: String,
    latest_run_id: Option<String>,
    project_id: Option<String>,
    external_session_id: Option<String>,
}

fn claim_run(
    conn: &mut rusqlite::Connection,
    request: &AgentRunStartArgs,
    binding: &HermesSessionBinding,
    model_selection: (&str, &str),
    max_turns: usize,
    runtime: (
        &ExecutionRuntime,
        Option<&super::orchestration::admission::PreparedCollaboration>,
    ),
    initial_route: &super::routing::RouteDecision,
) -> Result<(), String> {
    let (provider, model) = model_selection;
    let (runtime, collaboration) = runtime;
    let engine = runtime.engine();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    initial_route.verify_fence(&tx)?;
    super::submission::live::check_claim(
        &tx,
        &binding.thread_id,
        initial_route.submission.as_ref(),
    )
    .map_err(|e| e.to_string())?;
    let existing: Option<ThreadClaim> = tx
        .query_row(
            "SELECT engine, latest_run_id, project_id, external_session_id FROM agent_threads WHERE id=?1",
            [&binding.thread_id],
            |row| Ok(ThreadClaim {
                engine: row.get(0)?, latest_run_id: row.get(1)?,
                project_id: row.get(2)?, external_session_id: row.get(3)?,
            }),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if request.thread_id.is_some() && existing.is_none() {
        return Err("会话不存在".into());
    }
    if let Some(thread) = &existing {
        if binding.project_id != thread.project_id {
            return Err("任务所属项目已变化，请重新打开任务后再试".into());
        }
        if (thread.latest_run_id.is_some() || thread.external_session_id.is_some())
            && thread.engine != runtime.engine()
        {
            return Err("已有会话不能切换引擎".into());
        }
        if request.project_id.is_some() && request.project_id != thread.project_id {
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
    let mut decision = super::routing::resolve_binding(
        request.engine.as_deref(),
        existing.as_ref().map(|thread| thread.engine.as_str()),
        existing.as_ref().is_some_and(|thread| {
            thread.latest_run_id.is_some() || thread.external_session_id.is_some()
        }),
    )?;
    decision.carry_preflight(initial_route)?;
    if decision.engine().as_str() != engine {
        return Err("会话引擎已变化，请重试".into());
    }
    tx.execute("INSERT INTO agent_runs (id,thread_id,project_id,status,provider,model,prompt_version,max_model_calls,current_model_calls,engine,engine_version,created_at,updated_at) VALUES (?1,?2,?3,'\"queued\"',?4,?5,?9,?6,0,?10,?7,?8,?8)",
        rusqlite::params![binding.run_id, binding.thread_id, binding.project_id, provider, model, max_turns, runtime.version(), now_ms(), format!("{engine}-rpc-v1"), engine]).map_err(|e| e.to_string())?;
    tx.execute("UPDATE agent_threads SET engine=?5,latest_run_id=?1,external_session_id=?2,status='\"running\"',updated_at=?3 WHERE id=?4",
        rusqlite::params![binding.run_id,format!("{engine}:{}",stable_id(&binding.thread_id)),now_ms(),binding.thread_id,engine]).map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO agent_messages (id,thread_id,run_id,role,content,source,created_at) VALUES (?1,?2,?3,'user',?4,?6,?5)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(),binding.thread_id,binding.run_id,request.message,now_ms(),engine]).map_err(|e| e.to_string())?;
    let mut snapshot = super::routing::CapabilitySnapshot::new(
        decision.engine(),
        Some(runtime.version()),
        super::routing::PreparationEvidence::ValidatedRuntime,
        Some(super::capabilities::prepared_host_tools(
            request
                .workspace_permission_mode
                .as_deref()
                .unwrap_or("ask"),
        )),
        now_ms(),
    );
    snapshot.host_delegation = collaboration.is_some();
    super::routing::persist(&tx, &binding.run_id, &decision, &snapshot)?;
    super::submission::live::bind_claim(
        &tx,
        initial_route.submission.as_ref(),
        &binding.run_id,
        now_ms() as i64,
    )
    .map_err(|e| e.to_string())?;
    if let Some(collaboration) = collaboration {
        collaboration.commit(&tx, &binding.run_id, provider, model, now_ms() as i64)?;
    }
    tx.commit().map_err(|e| e.to_string())
}

pub(crate) fn record_runtime_observation(app: &AppHandle, runtime: &ExecutionRuntime) {
    let result = (|| -> Result<Vec<PathBuf>, String> {
        let (executable, pointer, extension) = match runtime {
            ExecutionRuntime::Pi(r) => (
                &r.executable,
                Some(update::root(app)?.join("active.json")),
                Some(r.extension.clone()),
            ),
            ExecutionRuntime::OpenCode(r) => (
                &r.executable,
                Some(super::opencode::update::root(app)?.join("active.json")),
                None,
            ),
            ExecutionRuntime::Claude(r) => (&r.executable, None, None),
        };
        let mut paths = vec![executable.clone()];
        if let Some(pointer) = pointer {
            let root = executable.parent().ok_or("运行时路径无效")?;
            paths.extend([root.to_path_buf(), root.join("manifest.json"), pointer]);
        } else {
            paths.extend(super::claude::runtime::candidate_paths()?);
        }
        if let Some(extension) = extension {
            paths.push(extension);
        }
        Ok(paths)
    })();
    if let Ok(paths) = result {
        if let Ok(engine) = super::registry::EngineId::parse(runtime.engine()) {
            super::runtime_observations::record(
                crate::db::get_db_path(app),
                engine,
                runtime.version(),
                paths,
            );
        }
    }
}

pub(crate) async fn start_engine(
    app: AppHandle,
    request: AgentRunStartArgs,
    channel: tauri::ipc::Channel<AgentEvent>,
    route: super::routing::RouteDecision,
) -> ApiResponse<AgentRunStartResult> {
    let engine = route.engine().as_str();
    let result = async {
        super::orchestration::admission::validate(&request)?;
        let _update_guard = if engine == "claude_code" {
            Some(
                super::runtime_updates::CLAUDE_GATE
                    .try_lock()
                    .map_err(|_| "Claude Code 正在更新，请完成后重试")?,
            )
        } else {
            None
        };
        super::runtime_observations::invalidate(route.engine());
        let runtime = if engine == "claude_code" {
            ExecutionRuntime::Claude(super::claude::runtime::locate().await?)
        } else if engine == "opencode" {
            let handle = app.clone();
            ExecutionRuntime::OpenCode(
                tokio::task::spawn_blocking(move || {
                    super::opencode::update::resolve(&handle).map(|(runtime, _)| runtime)
                })
                .await
                .map_err(|e| e.to_string())??,
            )
        } else if route.engine() == super::registry::EngineId::Pi {
            let handle = app.clone();
            let runtime = tokio::task::spawn_blocking(move || {
                update::resolve(&handle).map(|(runtime, _)| runtime)
            })
            .await
            .map_err(|_| "Pi 校验任务失败")??;
            ExecutionRuntime::Pi(runtime)
        } else {
            return Err("此执行入口不支持所选引擎".into());
        };
        record_runtime_observation(&app, &runtime);
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
        let mut prepared = prepare(&app, &request, &binding, runtime)?;
        let has_history: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_messages WHERE thread_id=?1 AND role='assistant')",
            [&binding.thread_id], |row| row.get(0)).map_err(|e| e.to_string())?;
        if has_history && !prepared.session.is_file() {
            return Err(format!(
                "{label} 会话文件缺失，无法恢复原历史；请恢复应用数据或新建会话"
            ));
        }
        let collaboration = super::orchestration::admission::PreparedCollaboration::capture(
            &request, &prepared.scope, prepared.runtime.engine(), prepared.runtime.version(),
        )?;
        claim_run(
            &mut conn,
            &request,
            &binding,
            (&prepared.provider.id, &prepared.model),
            prepared.max_turns,
            (&prepared.runtime, collaboration.as_ref()),
            &route,
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
        super::submission::commands::apply_cancel_intent(
            &binding.db_path,
            &binding.run_id,
            &cancel,
        );
        let response = AgentRunStartResult {
            thread_id: binding.thread_id.clone(),
            run_id: binding.run_id.clone(),
        };
        tauri::async_runtime::spawn(async move {
            let mut memory = None;
            let mut workflow_memory = None;
            let collaborative = super::orchestration::native_child::parent_context(&binding)
                .ok().flatten().is_some();
            let original_message = request.message.clone();
            let outcome = async {
                let conn =
                    rusqlite::Connection::open(&binding.db_path).map_err(|e| e.to_string())?;
                RunStore::new(conn)
                    .update_run_status(&binding.run_id, &RunStatus::Running, now_ms())
                    .map_err(|e| e.to_string())?;
                events
                    .emit(AgentEventPayload::RunStarted {
                        user_message: request.message,
                        max_turns: prepared.max_turns,
                        context,
                        skill: None,
                    })
                    .map_err(|e| e.to_string())?;
                memory = super::openviking::RunMemory::begin(
                    &app,
                    engine,
                    &original_message,
                    &mut prepared.context,
                    &events,
                    &cancel,
                )
                .await;
                if cancel.is_cancelled() {
                    return Err("用户取消".into());
                }
                match &prepared.runtime {
                    ExecutionRuntime::Pi(_) => {
                        transport::run(&prepared, &binding, &events, &cancel).await
                    }
                    ExecutionRuntime::OpenCode(_) => {
                        super::opencode::transport::run(&prepared, &binding, &events, &cancel).await
                    }
                    ExecutionRuntime::Claude(_) => {
                        super::claude::transport::run(&prepared, &binding, &events, &cancel).await
                    }
                }
            }
            .await;
            let native_stop_unknown = outcome.as_ref().err().is_some_and(|error| error.contains("停止状态待核实"));
            let (run_status, thread_status) = if native_stop_unknown {
                let _ = events.emit(AgentEventPayload::EngineDegraded {
                    reason: "执行进程停止状态待核实，请勿重复发送".into(), reconnecting: false,
                });
                (RunStatus::Interrupted, ThreadStatus::Failed)
            } else if cancel.is_cancelled() {
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
                        if let Some(memory) = memory {
                            if collaborative {
                                workflow_memory = Some((memory, answer.clone()));
                            } else {
                            memory
                                .finish(
                                    &binding.run_id,
                                    &original_message,
                                    &answer,
                                    &events,
                                    &cancel,
                                )
                                .await;
                            }
                        }
                        if cancel.is_cancelled() {
                            let _ = events.emit(AgentEventPayload::RunCancelled {
                                reason: "用户取消".into(),
                            });
                            (RunStatus::Cancelled, ThreadStatus::Cancelled)
                        } else {
                            let _ = events.emit(AgentEventPayload::RunCompleted {
                                outcome: "completed".into(),
                                final_answer: answer,
                                model_calls: turns,
                            });
                            (RunStatus::Completed, ThreadStatus::Completed)
                        }
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
            if let Some((memory, answer)) = workflow_memory {
                // Only an authoritative whole-workflow success may synchronize.
                // Child/intermediate native Runs never create RunMemory hooks.
                let workflow = rusqlite::Connection::open(&binding.db_path).ok().and_then(|conn| {
                    conn.query_row("SELECT id FROM agent_workflows WHERE root_run_id=?1 AND state='succeeded' AND final_result_ref IS NOT NULL", [&binding.run_id], |r| r.get::<_, String>(0)).optional().ok().flatten()
                });
                if let Some(workflow) = workflow {
                    memory.finish(&workflow, &original_message, &answer, &events, &cancel).await;
                }
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
            CREATE TABLE agent_runs (id TEXT PRIMARY KEY,thread_id TEXT,project_id TEXT,status TEXT,provider TEXT,model TEXT,prompt_version TEXT,max_model_calls INTEGER,current_model_calls INTEGER,engine TEXT,engine_version TEXT,route_decision_json TEXT,created_at INTEGER,updated_at INTEGER);
            CREATE TABLE agent_messages (id TEXT PRIMARY KEY,thread_id TEXT,run_id TEXT,role TEXT,content TEXT,source TEXT,created_at INTEGER);").unwrap();
        super::super::submission::create_schema(&conn).unwrap();
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
        let mut route =
            super::super::routing::resolve_binding(Some("claude_code"), None, false).unwrap();
        route.candidates = super::super::candidates::screen(&request, &route, None, |_| {
            super::super::runtime_observations::Observation::Unknown
        });
        route.shadow = Some(
            serde_json::from_value(json!({
                "policyVersion":"shadow-general-v1", "recommendedPreparationEngine":"claude_code",
                "automaticExecution":false, "probes":[], "elapsedMs":1
            }))
            .unwrap(),
        );
        claim_run(
            &mut conn,
            &request,
            &binding,
            ("provider", "model"),
            6,
            (&runtime, None),
            &route,
        )
        .unwrap();
        let row: (String, String, String) = conn.query_row("SELECT t.engine,r.engine,r.engine_version FROM agent_threads t JOIN agent_runs r ON r.thread_id=t.id", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        let audit: String = conn
            .query_row(
                "SELECT route_decision_json FROM agent_runs WHERE id='first'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let audit: Value = serde_json::from_str(&audit).unwrap();
        assert_eq!(audit["decision"]["source"], "explicit");
        assert_eq!(audit["decision"]["candidates"].as_array().unwrap().len(), 4);
        assert_eq!(
            audit["decision"]["candidates"][2]["state"],
            "needs_preparation"
        );
        assert_eq!(
            audit["decision"]["candidates"][0]["exclusions"][0],
            "explicit_selection"
        );
        assert_eq!(audit["decision"]["selectedEngine"], "claude_code");
        assert_eq!(
            audit["decision"]["shadow"]["recommendedPreparationEngine"],
            "claude_code"
        );
        assert_eq!(audit["decision"]["shadow"]["automaticExecution"], false);
        assert_eq!(audit["capabilities"]["runtimeVersion"], "2.1.233");
        assert_eq!(audit["capabilities"]["transport"], "stream-json+mcp");
        assert_eq!(
            row,
            ("claude_code".into(), "claude_code".into(), "2.1.233".into())
        );
        request.thread_id = Some(binding.thread_id.clone());
        binding.run_id = "second".into();
        assert!(claim_run(
            &mut conn,
            &request,
            &binding,
            ("provider", "model"),
            6,
            (&runtime, None),
            &route,
        )
        .is_err());
        conn.execute("UPDATE agent_runs SET status='completed'", [])
            .unwrap();
        let pi = ExecutionRuntime::Pi(runtime::Runtime {
            version: runtime::VERSION.into(),
            executable: PathBuf::new(),
            extension: PathBuf::new(),
        });
        assert!(claim_run(
            &mut conn,
            &request,
            &binding,
            ("provider", "model"),
            6,
            (&pi, None),
            &route,
        )
        .is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        claim_run(
            &mut conn,
            &request,
            &binding,
            ("provider", "model"),
            6,
            (&runtime, None),
            &route,
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn native_claim_consumes_submission_in_the_run_and_message_transaction() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::create_schema(&conn).unwrap();
        let now = super::super::submission::live::now_ms();
        let stamp = format!("{now:012x}");
        let request_id = format!("{}-{}-7000-8000-000000000001", &stamp[..8], &stamp[8..]);
        let request: AgentRunStartArgs =
            serde_json::from_value(json!({"message":"fixture","engine":"claude_code"})).unwrap();
        let intent = serde_json::to_value(&request).unwrap();
        let mut submissions = super::super::submission::SubmissionStore::new(&mut conn);
        let admitted = submissions
            .admit(
                super::super::submission::Request {
                    client_request_id: &request_id,
                    thread_id: None,
                    engine: "claude_code",
                    intent: &intent,
                },
                now,
            )
            .unwrap();
        let preparing = submissions
            .advance(
                &admitted.submission.id,
                0,
                super::super::submission::State::Preparing,
                now,
            )
            .unwrap();
        let mut route =
            super::super::routing::resolve_binding(Some("claude_code"), None, false).unwrap();
        route.submission = Some(super::super::submission::live::Receipt {
            id: preparing.id.clone(),
            generation: preparing.generation,
        });
        let binding = HermesSessionBinding {
            db_path: PathBuf::new(),
            notes_dir: PathBuf::new(),
            project_id: None,
            thread_id: "new".into(),
            run_id: "r".into(),
        };
        let runtime = ExecutionRuntime::Claude(super::super::claude::runtime::Runtime {
            executable: PathBuf::from("/unused"),
            version: "2.1.233".into(),
        });
        claim_run(
            &mut conn,
            &request,
            &binding,
            ("p", "m"),
            6,
            (&runtime, None),
            &route,
        )
        .unwrap();
        let bound = super::super::submission::SubmissionStore::new(&mut conn)
            .get(&preparing.id)
            .unwrap();
        assert_eq!(bound.state, super::super::submission::State::Dispatching);
        assert_eq!(bound.run_id.as_deref(), Some("r"));
        assert_eq!(
            conn.query_row(
                "SELECT lower(trim(status,'\"')) FROM agent_runs WHERE id='r'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "queued"
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM agent_messages WHERE run_id='r'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let encoded = serde_json::to_value(&route).unwrap();
        assert!(encoded.get("submission").is_none());
        assert!(claim_run(
            &mut conn,
            &request,
            &binding,
            ("p", "m"),
            6,
            (&runtime, None),
            &route,
        )
        .is_err());
    }

    #[tokio::test]
    #[ignore = "requires pnpm pi:bundle; only uses a local fixture, never a real provider"]
    async fn native_pi_roundtrip_approval_history_and_plan_denial() {
        native_pi_protocol_contract("openai").await;
    }

    #[tokio::test]
    #[ignore = "requires bundled Pi; exercises Anthropic through a local HTTP fixture"]
    async fn native_pi_anthropic_roundtrip_approval_history_and_plan_denial() {
        native_pi_protocol_contract("anthropic").await;
    }

    async fn native_pi_protocol_contract(protocol: &'static str) {
        use std::sync::Mutex;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        struct Capture(Mutex<Vec<AgentEvent>>);
        impl EventTransport for Capture {
            fn send(&self, event: AgentEvent) -> Result<(), String> {
                if let AgentEventPayload::ApprovalRequired { approval_id, .. } = &event.payload {
                    super::super::hermes::gateway_client::send_run_control(
                        &event.run_id,
                        super::super::hermes::gateway_client::GatewayControl::Approval {
                            approval_id: approval_id.clone(),
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
        let endpoint = format!(
            "http://{}{}",
            listener.local_addr().unwrap(),
            if protocol == "anthropic" { "" } else { "/v1" }
        );
        let web = crate::agent::web::tests::fixture().await;
        let page_url = format!("{}/page", web.0);
        let (cancel_ready, cancel_wait) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut cancel_ready = Some(cancel_ready);
            let mut requests = vec![];
            for index in 0..9 {
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
                        let expected_auth = if protocol == "anthropic" {
                            "x-api-key: fixture-secret"
                        } else {
                            "authorization: bearer fixture-secret"
                        };
                        assert!(headers.lines().any(|line| line == expected_auth));
                        let request_line: Vec<_> =
                            headers.lines().next().unwrap().split_whitespace().collect();
                        assert_eq!(request_line[0], "post");
                        let target: hyper::Uri = request_line[1].parse().unwrap();
                        assert_eq!(
                            target.path(),
                            if protocol == "anthropic" {
                                "/v1/messages"
                            } else {
                                "/v1/chat/completions"
                            }
                        );
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
                if index == 7 {
                    let body = r#"{"error":{"message":"fixture provider rejection","type":"invalid_request_error"}}"#;
                    stream.write_all(format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                    continue;
                }
                if index == 8 {
                    cancel_ready.take().unwrap().send(()).unwrap();
                    let mut sink = [0u8; 1];
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        stream.read(&mut sink),
                    )
                    .await;
                    continue;
                }
                let tool = index == 0 || index == 3 || index == 5;
                let delta = if index == 5 {
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":"call-web","type":"function","function":{"name":"webfetch","arguments":json!({"url":page_url}).to_string()}}]})
                } else if tool {
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("call-{index}"),"type":"function","function":{"name":"write","arguments":json!({"path":if index==0 {"result.txt"} else {"denied.txt"},"content":"Pi fixture"}).to_string()}}]})
                } else {
                    json!({"role":"assistant","content":"Pi fixture complete"})
                };
                let body = if protocol == "anthropic" {
                    let mut frames = vec![
                        json!({"type":"message_start","message":{"id":format!("msg-{index}"),"type":"message","role":"assistant","model":"fixture","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
                    ];
                    if tool {
                        let function = &delta["tool_calls"][0]["function"];
                        frames.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":format!("tool-{index}"),"name":function["name"],"input":{}}}));
                        frames.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":function["arguments"]}}));
                    } else {
                        frames.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}));
                        frames.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Pi fixture complete"}}));
                    }
                    frames.push(json!({"type":"content_block_stop","index":0}));
                    frames.push(json!({"type":"message_delta","delta":{"stop_reason":if tool {"tool_use"} else {"end_turn"},"stop_sequence":null},"usage":{"output_tokens":1}}));
                    frames.push(json!({"type":"message_stop"}));
                    frames
                        .iter()
                        .map(|frame| {
                            format!(
                                "event: {}\ndata: {frame}\n\n",
                                frame["type"].as_str().unwrap()
                            )
                        })
                        .collect::<String>()
                } else {
                    format!(
                        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                        json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                        json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":if tool {"tool_calls"} else {"stop"}}]})
                    )
                };
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
                protocol: protocol.into(),
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
                read_lease: None,
                readonly: None,
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
        capture.0.lock().unwrap().clear();
        prepared.prompt = "Read the webpage in Plan mode".into();
        transport::run(&prepared, &binding, &events, &CancellationToken::new())
            .await
            .unwrap();
        assert!(!capture
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e.payload, AgentEventPayload::ApprovalRequired { .. })));
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
        assert!(requests[6]["messages"]
            .to_string()
            .contains("WEB_FIXTURE_OK"));
        let names: Vec<_> = requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| {
                if protocol == "anthropic" {
                    t["name"].as_str()
                } else {
                    t["function"]["name"].as_str()
                }
            })
            .collect();
        for name in ["webfetch", "websearch", "news_search"] {
            assert!(names.contains(&name));
        }
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
