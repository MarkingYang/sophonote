// ============================================================
// Track B · 智能体演进（AG-06 追加 / AG-07 扩展 / AG-08 扩展）：RunController 调试命令
// 用途：承载真实模型调用（DeepSeek/Kimi 实跑），完成 Phase 1 Spike 采纳门禁。
//
// 边界：
// - API Key 只在 Rust 侧读取（OpenAiCompatGateway::from_settings），
//   React 永不接触密钥；
// - 本命令是调试面：不走 RunStore（Phase 2），产物一次性返回；
//   AG-07 起 agent_spike_run_stream 额外经 Tauri Channel 实时发事件（门禁⑤）；
//   AG-08 起 agent_spike_mcp_list / agent_spike_mcp_run 接入 MCP stdio 工具（门禁⑥），
//   MCP 连接配置现传不落库，生命周期随命令结束（drop 即关停子进程）；
// - 默认 max_turns=6、上限 20，防止真实供应商下的失控循环烧 token。
// ============================================================
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::commands::ApiResponse;
use crate::model::gateway::{ModelGateway, SharedGateway};
use crate::model::messages::{ModelError, ModelRequest, ModelResponse};
use crate::model::openai_compat::OpenAiCompatGateway;
use crate::tools::builtin::spike_registry;
use crate::tools::mcp::{discover, McpStdioConfig};
use crate::tools::ToolRegistry;

use super::engine::{legacy_spike_engine, AgentEngine, HermesFocusDocument, RunEnvelope};
use super::engine_select::{
    is_engine_unavailable, probe_hermes_production_health, resolve_engine, EngineChoice,
    EngineResolve,
};
use super::events::{AgentEvent, AgentEventPayload, EventEmitter, EventTransport};
use super::run_controller::{SpikeParams, SpikeRunReport};

/// Hermes Surface 的产品路径不会调用 SophoNote ModelGateway。保留该占位实现，
/// 只是为了让历史 Spike 与 Hermes 暂时共享 `RunEnvelope`，并确保产品会话不会
/// 因 SophoNote 本地缺少供应商 API Key 而在连接 Hermes 前失败。
struct HermesOwnedModelGateway;

#[async_trait::async_trait]
impl ModelGateway for HermesOwnedModelGateway {
    async fn complete(
        &self,
        _request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Config(
            "Hermes Surface 不允许调用 SophoNote ModelGateway".into(),
        ))
    }
}

/// AG-19 归属校验（纯函数，独立可测）：
/// - 新建 Thread（thread = None）：项目取请求侧；
/// - 复用 Thread：请求显式带 project 时必须与 Thread 归属一致，否则拒绝；
///   Run 的 project_id 一律继承 Thread（不信任请求侧，杜绝越权挂项目）。
fn resolve_run_scope(
    thread: Option<&crate::agent::types::AgentThread>,
    requested_project_id: Option<&str>,
) -> Result<Option<String>, String> {
    match thread {
        None => Ok(requested_project_id.map(str::to_string)),
        Some(t) => {
            if let Some(req) = requested_project_id {
                if t.project_id.as_deref() != Some(req) {
                    return Err(format!("Thread {} 不属于项目 {}", t.id, req));
                }
            }
            Ok(t.project_id.clone())
        }
    }
}

/// 读取已绑定的 Hermes 持久 Session。新 Thread 在首次 `prompt.submit` 时由
/// Gateway 原生创建；这是 Hermes Desktop 同款的空白草稿语义。
fn hermes_session_for_thread(
    db_path: &std::path::Path,
    thread_id: &str,
) -> Result<Option<String>, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
    crate::agent::store::RunStore::new(conn)
        .external_session_id_for_thread(thread_id)
        .map_err(|e| format!("读取 Hermes Session 映射失败: {e}"))
}

/// 历史 Rig 测试用项目工具注册表。产品 Hermes Surface 不调用此函数，
/// SophoNote 工具未来只能通过可验证、绑定 Session 的 capability channel 暴露。
/// Spike 内置假工具 + 2 个只读工具（AG-19）+ 3 个写工具（AG-24：
/// create_document / propose_document_patch / move_document）。
/// 所有工具实例绑定 project_id（范围隔离）；写工具另绑 run_id（审批归属）。
/// 铁律：模型侧写工具恒 dry-run（propose_document_patch 只产 diff 提案），
/// 落盘必须经用户侧 document_apply_patch 命令批准——Agent 不直碰 notes.rs/SQLite/文件。
#[allow(dead_code)]
pub(crate) fn project_registry(
    db_path: std::path::PathBuf,
    notes_dir: std::path::PathBuf,
    project_id: &str,
    run_id: &str,
) -> ToolRegistry {
    let mut reg = spike_registry();
    reg.register(Arc::new(
        crate::tools::project::ListProjectDocumentsTool::new(
            db_path.clone(),
            project_id.to_string(),
        ),
    ));
    reg.register(Arc::new(crate::tools::project::ReadDocumentTool::new(
        db_path.clone(),
        notes_dir.clone(),
        project_id.to_string(),
    )));
    reg.register(Arc::new(crate::tools::documents::CreateDocumentTool::new(
        db_path.clone(),
        notes_dir.clone(),
        project_id.to_string(),
        run_id.to_string(),
    )));
    reg.register(Arc::new(
        crate::tools::documents::ProposeDocumentPatchTool::new(
            db_path.clone(),
            notes_dir.clone(),
            project_id.to_string(),
            run_id.to_string(),
        ),
    ));
    reg.register(Arc::new(crate::tools::documents::MoveDocumentTool::new(
        db_path,
        project_id.to_string(),
    )));
    reg
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpikeRunArgs {
    /// 用户输入
    pub message: String,
    /// 供应商覆盖（None = settings 默认供应商），用于 DeepSeek/Kimi 双供应商实跑
    #[serde(default)]
    pub provider: Option<String>,
    /// 仅供未注册的历史 Spike 显式传入；不会用于产品会话。
    #[serde(default)]
    pub system: Option<String>,
    /// 模型调用预算覆盖（None = 6；夹紧到 1..=20）
    #[serde(default)]
    pub max_turns: Option<usize>,
}

/// AG-07：Tauri Channel 的 EventTransport 包装——事件经 invoke 时传入的
/// on_event 通道实时送达前端（Channel::send 本身同步、内部排队，无需 await）
struct ChannelTransport {
    channel: tauri::ipc::Channel<AgentEvent>,
}

impl EventTransport for ChannelTransport {
    fn send(&self, event: AgentEvent) -> Result<(), String> {
        self.channel.send(event).map_err(|e| e.to_string())
    }
}

/// 调试命令的共享执行体：仅执行调用者显式传入的 system，不内置产品提示词。
/// （内置假工具 vs 合并 MCP 工具后的注册表），事件流开关按命令区分
async fn run_spike_command(
    app: &AppHandle,
    request: SpikeRunArgs,
    events: Option<Arc<EventEmitter>>,
    registry: Arc<ToolRegistry>,
) -> ApiResponse<SpikeRunReport> {
    let gateway: SharedGateway =
        match OpenAiCompatGateway::from_settings(app, request.provider.as_deref()) {
            Ok(g) => Arc::new(g),
            Err(err) => return ApiResponse::err(err.to_string()),
        };

    if request.message.trim().is_empty() {
        return ApiResponse::err("message 不能为空".into());
    }

    let max_turns = request.max_turns.unwrap_or(6).clamp(1, 20);
    let params = SpikeParams {
        system: request.system,
        history: Vec::new(), // Spike 调试命令无 Thread，不带历史
        user: request.message,
        max_turns,
        temperature: Some(0.0), // Spike 期要可复现；正式运行温度由 PromptRegistry 管
        // AG-22：调试命令无 Run/无版本化提示词，保持 Spike 口径
        run_id: None,
        prompt_version: "spike@v1".into(),
        // AG-26：调试命令无选区上下文
        run_context: None,
        // AG-27：调试命令不激活 Skill（正式 Run 经 agent_run_start）
        run_skill: None,
        max_tool_calls: None,
    };

    // Spike 调试命令不做中途取消（无 UI 挂接）；正式路径经 agent_run_cancel
    let cancel = CancellationToken::new();
    let engine = legacy_spike_engine();
    if let Err(err) = engine.health() {
        return ApiResponse::err(err.to_string());
    }

    match engine
        .run_with_events(RunEnvelope {
            gateway,
            registry,
            params,
            cancel,
            events,
            observer: None,
            context_pack: None,
            model_route: None,
            hermes_session_id: None,
            hermes_memory_scope_key: None,
            hermes_input: None,
            hermes_model: None,
            hermes_provider: None,
            hermes_command: None,
            hermes_workspace_root: None,
            hermes_attachments: Vec::new(),
            hermes_focus_document: None,
            hermes_project_context: false,
            hermes_session_binding: None,
        })
        .await
    {
        Ok(report) => {
            println!(
                "[agent] spike command finished: outcome={} model_calls={} tools={}",
                report.outcome,
                report.model_calls,
                report.tool_executions.len()
            );
            ApiResponse::ok(report)
        }
        Err(err) => ApiResponse::err(err.to_string()),
    }
}

/// AG-06 Spike 调试命令：以自有 ModelGateway + ToolGateway 驱动 rig AgentRun，
/// 真实供应商调用 + 确定性假工具，一次返回完整运行报告（无事件流）。
#[tauri::command]
pub async fn agent_spike_run(app: AppHandle, request: SpikeRunArgs) -> ApiResponse<SpikeRunReport> {
    run_spike_command(&app, request, None, Arc::new(spike_registry())).await
}

/// AG-07 流式调试命令（门禁⑤）：同 agent_spike_run 的入参与报告返回值，
/// 另经 on_event（Tauri Channel）实时推送有序 AgentEvent 流。
/// DevTools 调用示例：
///   window.__TAURI__.core.invoke('agent_spike_run_stream', {
///     request: { message: '查杭州天气再把气温加 3' },
///     onEvent: (e) => console.log(e.seq, e.payload),
///   })
#[tauri::command]
pub async fn agent_spike_run_stream(
    app: AppHandle,
    request: SpikeRunArgs,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> ApiResponse<SpikeRunReport> {
    let run_id = format!("spike-{}", uuid::Uuid::new_v4());
    let emitter = Arc::new(EventEmitter::new(
        "spike",
        run_id,
        Arc::new(ChannelTransport { channel: on_event }),
    ));
    run_spike_command(&app, request, Some(emitter), Arc::new(spike_registry())).await
}

// ---------------- AG-08：MCP stdio 工具接入门禁⑥ ----------------

/// MCP 单个工具的下发视图（前端/console 可读）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// agent_spike_mcp_list 报告：零模型调用即验证「stdio 握手 + tools/list + 注册进同一 ToolRegistry」
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListReport {
    /// 实际注册的 MCP 工具
    pub tools: Vec<McpToolInfo>,
    /// 合并后 ToolRegistry.definitions() 的名称序列（内置假工具 + MCP 工具，按名排序）——
    /// 门禁⑥的直接证据：MCP 工具与内置工具同表下发
    pub merged_definitions: Vec<String>,
}

/// AG-08 门禁⑥ 零成本验证命令：连接 MCP stdio 服务器 → tools/list → 全部注册进
/// spike ToolRegistry → 返回工具清单与合并后的 definitions（不发起任何模型调用）。
/// DevTools 调用示例（首跑 npx 会先下载 server，耗时属正常）：
///   window.__TAURI__.core.invoke('agent_spike_mcp_list', {
///     mcp: { command: 'npx', args: ['-y', '@modelcontextprotocol/server-everything'] },
///   }).then(r => console.log(r))
#[tauri::command]
pub async fn agent_spike_mcp_list(mcp: McpStdioConfig) -> ApiResponse<McpListReport> {
    let (descriptors, instances) = match discover(&mcp).await {
        Ok(found) => found,
        Err(err) => return ApiResponse::err(err),
    };
    let mut registry = spike_registry();
    for tool in instances {
        registry.register(tool);
    }
    let report = McpListReport {
        tools: descriptors
            .into_iter()
            .map(|d| McpToolInfo {
                name: d.name,
                description: d.description,
                input_schema: d.input_schema,
            })
            .collect(),
        merged_definitions: registry.definitions().into_iter().map(|d| d.name).collect(),
    };
    println!(
        "[agent] mcp list: {} mcp tools discovered, merged registry size = {}",
        report.tools.len(),
        report.merged_definitions.len()
    );
    ApiResponse::ok(report)
}

/// AG-08 实跑入参：SpikeRunArgs 全字段 + MCP 服务器配置
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpikeMcpRunArgs {
    #[serde(flatten)]
    pub base: SpikeRunArgs,
    pub mcp: McpStdioConfig,
}

/// AG-08 实跑调试命令（真实模型调用，成本敏感默认不跑）：
/// 合并 MCP 工具进 ToolRegistry 后驱动完整 AgentRun 循环。
/// DevTools 调用示例：
///   window.__TAURI__.core.invoke('agent_spike_mcp_run', {
///     request: {
///       message: '用 echo 工具回显这句话：门禁通过',
///       mcp: { command: 'npx', args: ['-y', '@modelcontextprotocol/server-everything'] },
///     },
///   }).then(r => console.log(r.data))
#[tauri::command]
pub async fn agent_spike_mcp_run(
    app: AppHandle,
    request: SpikeMcpRunArgs,
) -> ApiResponse<SpikeRunReport> {
    let (descriptors, instances) = match discover(&request.mcp).await {
        Ok(found) => found,
        Err(err) => return ApiResponse::err(err),
    };
    let mut registry = spike_registry();
    for tool in instances {
        registry.register(tool);
    }
    let _ = descriptors;
    run_spike_command(&app, request.base, None, Arc::new(registry)).await
}

// ---------------- AG-13：RunStore 命令（Phase 2 持久化层）----------------

/// 创建新 Thread（话题容器）
#[tauri::command]
pub async fn agent_thread_create(
    app: AppHandle,
    title: String,
    project_id: Option<String>,
) -> ApiResponse<String> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let store = crate::agent::store::RunStore::new(conn);
    if let Err(e) = store.create_thread(&thread_id, &title, project_id.as_deref(), now_ms) {
        return ApiResponse::err(e.to_string());
    }
    drop(store);

    // Hermes Desktop 同款草稿语义：空白 Thread 不提前制造持久 Session；首次
    // prompt.submit 在同一 Gateway 连接内创建并绑定，避免孤儿空会话。
    if !crate::agent::hermes::gateway_env_configured() {
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            let _ = crate::agent::store::RunStore::new(conn).delete_thread(&thread_id);
        }
        return ApiResponse::err(format!(
            "Hermes Agent 未连接：请设置 {} 与 {}",
            crate::agent::hermes::ENV_GATEWAY_URL,
            crate::agent::hermes::ENV_GATEWAY_TOKEN
        ));
    }
    ApiResponse::ok(thread_id)
}

/// 列出 Thread（按 project_id + scope 过滤；默认 active）
#[tauri::command]
pub async fn agent_thread_list(
    app: AppHandle,
    project_id: Option<String>,
    scope: Option<String>,
) -> ApiResponse<Vec<crate::agent::types::AgentThread>> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let list_scope = match scope.as_deref() {
        Some("history") => crate::agent::types::ThreadListScope::History,
        _ => crate::agent::types::ThreadListScope::Active,
    };
    let store = crate::agent::store::RunStore::new(conn);
    match store.list_threads(project_id.as_deref(), list_scope) {
        Ok(threads) => ApiResponse::ok(threads),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 关闭会话 → 历史（可恢复）；无对话则硬删不进历史
#[tauri::command]
pub async fn agent_thread_close(app: AppHandle, thread_id: String) -> ApiResponse<bool> {
    let db_path = crate::db::get_db_path(&app);
    // 空 Chat 会被硬删；先删除对应 Hermes 空 Session，确保 1:1 生命周期不留孤儿。
    // 有对话的 Thread 只是进入历史，Hermes Session 与长期记忆均继续保留。
    let empty_external_session = {
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
        };
        let store = crate::agent::store::RunStore::new(conn);
        let has_user_message = match store.get_messages(&thread_id) {
            Ok(messages) => messages
                .iter()
                .any(|message| message.role == "user" && !message.content.trim().is_empty()),
            Err(e) => return ApiResponse::err(e.to_string()),
        };
        if has_user_message {
            None
        } else {
            match store.external_session_id_for_thread(&thread_id) {
                Ok(id) => id,
                Err(e) => return ApiResponse::err(e.to_string()),
            }
        }
    };
    if let Some(session_id) = empty_external_session {
        let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
            return ApiResponse::err(
                "无法删除空 Chat：Hermes 未配置，Session 尚未清理".to_string(),
            );
        };
        let mut gateway =
            match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
                .await
            {
                Ok(gateway) => gateway,
                Err(error) => return ApiResponse::err(error.to_string()),
            };
        if let Err(e) = gateway
            .call(
                "session.delete",
                serde_json::json!({"session_id": session_id}),
            )
            .await
        {
            return ApiResponse::err(format!("删除 Hermes Session 失败: {e}"));
        }
    }

    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let store = crate::agent::store::RunStore::new(conn);
    match store.close_thread(&thread_id, now_ms) {
        Ok(kept_in_history) => ApiResponse::ok(kept_in_history),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 从历史恢复为活跃会话
#[tauri::command]
pub async fn agent_thread_reopen(app: AppHandle, thread_id: String) -> ApiResponse<()> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let store = crate::agent::store::RunStore::new(conn);
    match store.reopen_thread(&thread_id, now_ms) {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 归档会话（UI 不可见，逾 TTL 硬删）
#[tauri::command]
pub async fn agent_thread_archive(app: AppHandle, thread_id: String) -> ApiResponse<()> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let store = crate::agent::store::RunStore::new(conn);
    match store.archive_thread(&thread_id, now_ms) {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 置顶/取消置顶会话（组织性操作，不扰动「最近」时序）
#[tauri::command]
pub async fn agent_thread_pin(app: AppHandle, thread_id: String, pinned: bool) -> ApiResponse<()> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let store = crate::agent::store::RunStore::new(conn);
    match store.set_thread_pinned(&thread_id, pinned, now_ms) {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 收藏夹分类列表（创建时间升序）
#[tauri::command]
pub async fn agent_collection_list(
    app: AppHandle,
) -> ApiResponse<Vec<crate::agent::types::ThreadCollection>> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let store = crate::agent::store::RunStore::new(conn);
    match store.list_collections() {
        Ok(collections) => ApiResponse::ok(collections),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 新建收藏夹分类（名称 trim、1–40 字符、同名拒绝）
#[tauri::command]
pub async fn agent_collection_create(
    app: AppHandle,
    name: String,
) -> ApiResponse<crate::agent::types::ThreadCollection> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let collection_id = format!("col-{}", uuid::Uuid::new_v4());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let store = crate::agent::store::RunStore::new(conn);
    match store.create_collection(&collection_id, &name, now_ms) {
        Ok(collection) => ApiResponse::ok(collection),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 会话加入/移动/移出收藏夹分类（collection_id = null 即移出）
#[tauri::command]
pub async fn agent_thread_set_collection(
    app: AppHandle,
    thread_id: String,
    collection_id: Option<String>,
) -> ApiResponse<()> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let store = crate::agent::store::RunStore::new(conn);
    match store.set_thread_collection(&thread_id, collection_id.as_deref()) {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 按可选 TTL 清理已归档会话（ttl_days=0/None → 不清理；普通与历史会话永久保留）
#[tauri::command]
pub async fn agent_thread_gc(app: AppHandle, ttl_days: Option<u32>) -> ApiResponse<usize> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let days = ttl_days.unwrap_or(0);
    if days == 0 {
        return ApiResponse::ok(0);
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let store = crate::agent::store::RunStore::new(conn);
    match store.gc_expired_threads(days, now_ms) {
        Ok(n) => ApiResponse::ok(n),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 获取 Thread 的所有消息（按创建时间升序）
#[tauri::command]
pub async fn agent_thread_messages(
    app: AppHandle,
    thread_id: String,
) -> ApiResponse<Vec<crate::agent::types::AgentMessage>> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let store = crate::agent::store::RunStore::new(conn);
    match store.get_messages(&thread_id) {
        Ok(msgs) => ApiResponse::ok(msgs),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 重放 Run 的事件（after_seq 重放，用于 seq 缺口恢复）
#[tauri::command]
pub async fn agent_run_events_replay(
    app: AppHandle,
    run_id: String,
    after_seq: u64,
) -> ApiResponse<Vec<String>> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let store = crate::agent::store::RunStore::new(conn);
    match store.replay_after_seq(&run_id, after_seq) {
        Ok(events) => ApiResponse::ok(events),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// AG-20（审计 P0-3 整改项⑤）：获取 Run 的状态快照（真正的 state_snapshot）。
/// 返回可重建 UI 的完整状态：Run 状态（runs 表真相源）、最新 seq、全量事件
///（含 seq=0，前端经同一 handleEvent 链路回灌，eventId 幂等去重）、
/// 消息尾部、工具调用状态与待审批项。
///
/// 与 agent_run_events_replay 的分工：replay 是缺口补齐的第一梯队（排他语义，
/// 取不到 seq=0）；本接口是补齐阶梯的升级项——replay 填不上（事件写丢/
/// seq=0 缺失）时用 Snapshot 全量重同步。Run 不存在返回错误。
#[tauri::command]
pub async fn agent_run_snapshot(
    app: AppHandle,
    run_id: String,
) -> ApiResponse<crate::agent::store::RunSnapshot> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let store = crate::agent::store::RunStore::new(conn);
    match store.state_snapshot(&run_id) {
        Ok(Some(snapshot)) => ApiResponse::ok(snapshot),
        Ok(None) => ApiResponse::err(format!("Run {} 不存在或已删除", run_id)),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// ISSUE-019：重新打开非终态会话时的权威恢复探针。
///
/// Hermes 是执行真相源：本进程没有 CancellationToken 只说明 SophoNote 宿主任务
/// 丢失，不能据此把 Hermes live turn 判成 interrupted。恢复时先通过原生
/// `session.resume` 重绑 transport：仍活跃则后台续接同一回合；已结束则从
/// Hermes transcript 对账完成；只有 Hermes 明确空闲且没有本轮回答才中断。
#[tauri::command]
pub async fn agent_run_reconcile(
    app: AppHandle,
    run_id: String,
) -> ApiResponse<crate::agent::store::RunSnapshot> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let mut store = crate::agent::store::RunStore::new(conn);
    if !global_cancel_registry().contains(&run_id) {
        let snapshot = match store.state_snapshot(&run_id) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return ApiResponse::err(format!("Run {} 不存在或已删除", run_id)),
            Err(e) => return ApiResponse::err(e.to_string()),
        };
        if matches!(
            snapshot.run_status,
            crate::agent::types::RunStatus::Completed
                | crate::agent::types::RunStatus::Failed
                | crate::agent::types::RunStatus::Cancelled
                | crate::agent::types::RunStatus::Interrupted
        ) {
            return ApiResponse::ok(snapshot);
        }
        let stored_session_id = match store.external_session_id_for_thread(&snapshot.thread_id) {
            Ok(Some(session_id)) => session_id,
            Ok(None) => {
                return ApiResponse::err("当前会话尚未绑定 Hermes Session，无法安全恢复".into())
            }
            Err(e) => return ApiResponse::err(e.to_string()),
        };
        let expected_user_message = snapshot
            .messages
            .iter()
            .rev()
            .find(|message| message.run_id == run_id && message.role == "user")
            .map(|message| message.content.as_str());
        let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
            return ApiResponse::err(
                "Hermes Agent 暂不可达；本轮保持恢复中，不会误判 interrupted".into(),
            );
        };
        let recovered = match crate::agent::hermes::attached_engine::resume_turn(
            &endpoint,
            &stored_session_id,
            expected_user_message,
        )
        .await
        {
            Ok(recovered) => recovered,
            Err(e) => {
                return ApiResponse::err(format!(
                    "Hermes 会话恢复失败；本轮保持恢复中，稍后重试：{e}"
                ))
            }
        };

        if recovered.active {
            let next_seq = snapshot.latest_seq.saturating_add(1);
            let transport = Arc::new(crate::agent::store::RunStoreTransport::new(
                db_path.to_string_lossy().to_string(),
            ));
            let emitter = Arc::new(EventEmitter::resume_at(
                &snapshot.thread_id,
                &run_id,
                next_seq,
                transport,
            ));
            let cancel = CancellationToken::new();
            global_cancel_registry().register(&run_id, cancel.clone());
            let run_id_clone = run_id.clone();
            let thread_id_clone = snapshot.thread_id.clone();
            let db_path_clone = db_path.clone();
            tauri::async_runtime::spawn(async move {
                let report = crate::agent::hermes::attached_engine::observe_recovered_turn(
                    recovered, emitter, cancel,
                )
                .await;
                match report {
                    Ok(report) => finalize_recovered_run(
                        &db_path_clone,
                        &run_id_clone,
                        &thread_id_clone,
                        &report,
                    ),
                    Err(error) => eprintln!(
                        "[agent] recovered run {} detached before terminal: {}",
                        run_id_clone, error
                    ),
                }
                global_cancel_registry().remove(&run_id_clone);
            });
            drop(store);
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
            };
            return match crate::agent::store::RunStore::new(conn).state_snapshot(&run_id) {
                Ok(Some(snapshot)) => ApiResponse::ok(snapshot),
                Ok(None) => ApiResponse::err(format!("Run {} 不存在或已删除", run_id)),
                Err(e) => ApiResponse::err(e.to_string()),
            };
        }

        if let Some(final_answer) = recovered.inactive_final_answer {
            let next_seq = snapshot.latest_seq.saturating_add(1);
            let transport = Arc::new(crate::agent::store::RunStoreTransport::new(
                db_path.to_string_lossy().to_string(),
            ));
            let emitter =
                EventEmitter::resume_at(&snapshot.thread_id, &run_id, next_seq, transport);
            let _ = emitter.emit(AgentEventPayload::MessageCompleted {
                text: final_answer.clone(),
            });
            let _ = emitter.emit(AgentEventPayload::RunCompleted {
                outcome: "completed".into(),
                final_answer: final_answer.clone(),
                model_calls: 1,
            });
            let report = SpikeRunReport {
                outcome: "completed".into(),
                final_answer,
                model_calls: 1,
                tool_executions: Vec::new(),
                invalid_resolutions: 0,
                usage: crate::model::messages::TokenUsage::default(),
                transcript: Vec::new(),
                error: None,
            };
            finalize_recovered_run(&db_path, &run_id, &snapshot.thread_id, &report);
        } else {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if let Err(e) = store.interrupt_orphaned_run(
                &run_id,
                "Hermes 已确认会话空闲，且未找到本轮完整回答，本轮已标记为 interrupted",
                now_ms,
            ) {
                return ApiResponse::err(e.to_string());
            }
        }
    }
    match store.state_snapshot(&run_id) {
        Ok(Some(snapshot)) => ApiResponse::ok(snapshot),
        Ok(None) => ApiResponse::err(format!("Run {} 不存在或已删除", run_id)),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

fn finalize_recovered_run(
    db_path: &std::path::Path,
    run_id: &str,
    thread_id: &str,
    report: &SpikeRunReport,
) {
    let Ok(conn) = rusqlite::Connection::open(db_path) else {
        return;
    };
    let store = crate::agent::store::RunStore::new(conn);
    let (run_status, thread_status) = match report.outcome.as_str() {
        "completed" => (
            crate::agent::types::RunStatus::Completed,
            crate::agent::types::ThreadStatus::Completed,
        ),
        "cancelled" => (
            crate::agent::types::RunStatus::Cancelled,
            crate::agent::types::ThreadStatus::Cancelled,
        ),
        _ => (
            crate::agent::types::RunStatus::Failed,
            crate::agent::types::ThreadStatus::Failed,
        ),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let _ = store.update_run_status(run_id, &run_status, now_ms);
    let _ = store.update_thread_status(thread_id, &thread_status, now_ms);
    let _ = store.set_model_calls(run_id, report.model_calls, now_ms);
    if report.outcome == "completed" && !report.final_answer.is_empty() {
        let existing = store
            .get_messages(thread_id)
            .unwrap_or_default()
            .iter()
            .any(|message| message.run_id == run_id && message.role == "assistant");
        if !existing {
            let _ = store.save_message(
                &format!("msg-{}", uuid::Uuid::new_v4()),
                thread_id,
                run_id,
                "assistant",
                &report.final_answer,
                now_ms,
            );
            let _ = store.refresh_thread_title_from_messages(thread_id, now_ms);
        }
    }
}

/// 删除 Run（级联删事件、消息、工具调用、审批）
#[tauri::command]
pub async fn agent_run_delete(app: AppHandle, run_id: String) -> ApiResponse<()> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let store = crate::agent::store::RunStore::new(conn);
    match store.delete_run_cascade(&run_id) {
        Ok(_) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

// ---------------- AG-14：Phase 2 集成命令（RunController + RunStore）----------------

/// AG-14 启动运行入参
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunStartArgs {
    /// 用户输入
    pub message: String,
    /// Thread ID（None = 自动创建新 Thread）
    #[serde(default)]
    pub thread_id: Option<String>,
    /// 项目 ID（创建新 Thread 时使用）
    #[serde(default)]
    pub project_id: Option<String>,
    /// 供应商覆盖（None = settings 默认供应商）
    #[serde(default)]
    pub provider: Option<String>,
    /// 旧 IPC 兼容字段；产品 Hermes Surface 明确忽略。
    #[serde(default)]
    pub system: Option<String>,
    /// 模型调用预算覆盖（None = 6；夹紧到 1..=20）
    #[serde(default)]
    pub max_turns: Option<usize>,
    /// 编辑器显式选区：作为 Surface 展示/审计数据，并通过 Hermes 原生
    /// `file.attach` 上传为用户上下文；绝不拼入 system prompt。
    #[serde(default)]
    pub selection: Option<RunSelectionContext>,
    /// AG-27：激活的 Skill 名（仅已安装且启用的 Skill 可激活；激活需项目归属）。
    /// None = 普通对话；有值时经 Hermes `command.dispatch` 激活原生 Skill。
    #[serde(default)]
    pub skill: Option<String>,
    /// 中栏当前打开文档（无选区 chip 时仍提示 Agent 优先 read 该篇）
    #[serde(default)]
    pub focus_document: Option<FocusDocumentContext>,
    /// 左侧显式“将项目加入会话”。仅在没有选区和当前文档附件时生效。
    #[serde(default)]
    pub include_project_context: bool,
    /// DEC-014：用户显式选择的图片/文件/文件夹/URL；Rust 负责路径与预算校验。
    #[serde(default)]
    pub attachments: Vec<crate::agent::attachments::RunAttachmentInput>,
    /// 当前 Run 的模型选择；None = 当前激活供应商默认模型。
    #[serde(default)]
    pub hermes_model: Option<String>,
    /// Hermes Runtime Provider slug，与 hermes_model 成对透传。
    #[serde(default)]
    pub hermes_provider: Option<String>,
    /// DEC-021：由 Composer 识别、仍保持用户原文的 Hermes `/` 命令。
    #[serde(default)]
    pub hermes_command: Option<String>,
    /// Chat / 项目显式授权的本地工作目录；命令层会 canonicalize 后交给 Hermes。
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// 前端权限模式。当前由 SophoNote 宿主编辑器/终端强制执行，并随会话展示；
    /// Hermes 仍使用自身原生危险操作审批，不能把此字段误当成 Runtime 沙箱。
    #[serde(default)]
    pub workspace_permission_mode: Option<String>,
}

/// 中栏打开文档（非选区）。正文来自发送时编辑器草稿，不从 MCP 回读。
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusDocumentContext {
    pub article_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub base_version: i64,
    #[serde(default)]
    pub markdown: Option<String>,
}

/// AG-26 选区上下文（前端 SelectionSnapshot 的命令层入参子集，camelCase）
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSelectionContext {
    pub article_id: String,
    pub title: String,
    /// 选区捕获时刻的文档版本（patch baseVersion 审计链起点）
    pub base_version: i64,
    pub selected_markdown: String,
    pub selected_text_hash: String,
    #[serde(default)]
    pub before_context: String,
    #[serde(default)]
    pub after_context: String,
}

impl RunSelectionContext {
    /// 映射为事件透传结构（events 层零命令层类型依赖）
    fn to_run_context(&self) -> crate::agent::events::RunContext {
        crate::agent::events::RunContext {
            article_id: self.article_id.clone(),
            title: self.title.clone(),
            base_version: self.base_version,
            selected_markdown: self.selected_markdown.clone(),
            selected_text_hash: self.selected_text_hash.clone(),
            before_context: self.before_context.clone(),
            after_context: self.after_context.clone(),
        }
    }
}

/// AG-14 启动运行返回
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunStartResult {
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesSkillInfo {
    pub name: String,
    pub description: String,
    pub origin: Option<String>,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub usage: usize,
    #[serde(default)]
    pub provenance: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesToolsetInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename(deserialize = "tool_count", serialize = "toolCount"))]
    pub tool_count: usize,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub usage: usize,
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesSkillDocument {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HermesCronJobInfo {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    pub schedule_kind: String,
    pub schedule_spec: serde_json::Value,
    pub status: String,
    pub enabled: bool,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub skills: Vec<String>,
    pub profile: String,
    pub execution_status: Option<String>,
    pub created_at: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCronDraft {
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub start_paused: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCronRunInfo {
    pub session_id: String,
    pub status: String,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub preview: String,
    pub end_reason: Option<String>,
    pub profile: String,
    pub model: Option<String>,
    pub tool_call_count: u64,
    pub model_call_count: u64,
    pub last_activity: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCronRunStep {
    pub index: usize,
    pub phase: String,
    pub title: String,
    pub tool_name: String,
    pub status: String,
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCronRunResult {
    pub session_id: String,
    pub markdown: String,
    pub steps: Vec<HermesCronRunStep>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpServerInfo {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub auth: Option<String>,
    pub tools: Vec<HermesToolInfo>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpServerCreate {
    pub name: String,
    pub transport: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub bearer_token: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpProbe {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub tools: Vec<HermesToolInfo>,
    #[serde(default)]
    pub prompts: usize,
    #[serde(default)]
    pub resources: usize,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct HermesMcpOAuthFlow {
    pub flow_id: String,
    pub server_name: String,
    pub status: String,
    pub authorization_url: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub tools: Vec<HermesToolInfo>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesTerminalBackendInfo {
    pub name: String,
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HermesTerminalBackends {
    #[serde(default)]
    pub active: String,
    #[serde(default)]
    pub backends: Vec<HermesTerminalBackendInfo>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesHubSourceInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub available: Option<bool>,
    #[serde(default)]
    #[serde(rename(deserialize = "rate_limited", serialize = "rateLimited"))]
    pub rate_limited: Option<bool>,
    #[serde(default)]
    pub searchable: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HermesHubSources {
    #[serde(default)]
    pub sources: Vec<HermesHubSourceInfo>,
    #[serde(default)]
    #[serde(rename(deserialize = "index_available", serialize = "indexAvailable"))]
    pub index_available: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HermesHubPreview {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    #[serde(rename(deserialize = "trust_level", serialize = "trustLevel"))]
    pub trust_level: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    #[serde(rename(deserialize = "skill_md", serialize = "skillMd"))]
    pub skill_md: String,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpCatalogEnvInfo {
    pub name: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpCatalogEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    #[serde(rename(deserialize = "auth_type", serialize = "authType"))]
    pub auth_type: String,
    #[serde(default)]
    #[serde(rename(deserialize = "required_env", serialize = "requiredEnv"))]
    pub required_env: Vec<HermesMcpCatalogEnvInfo>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    #[serde(rename(deserialize = "post_install", serialize = "postInstall"))]
    pub post_install: String,
    #[serde(default)]
    #[serde(rename(deserialize = "needs_install", serialize = "needsInstall"))]
    pub needs_install: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpCatalog {
    #[serde(default)]
    pub entries: Vec<HermesMcpCatalogEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesMcpCatalogInstallRequest {
    pub name: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesHubSkillInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub trust: String,
    #[serde(default)]
    pub identifier: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCommandInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesReferenceInfo {
    pub text: String,
    #[serde(default)]
    pub display: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCapabilities {
    pub commands: Vec<HermesCommandInfo>,
    pub skills: Vec<HermesSkillInfo>,
    pub references: Vec<HermesReferenceInfo>,
    pub toolsets: Vec<HermesToolsetInfo>,
    pub tools: Vec<HermesToolInfo>,
    pub mcp_servers: Vec<HermesMcpServerInfo>,
    pub terminal_backends: HermesTerminalBackends,
    pub hub_sources: HermesHubSources,
    pub browser_connected: bool,
    pub browser_url: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesHubPage {
    #[serde(default)]
    pub items: Vec<HermesHubSkillInfo>,
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(
        default = "default_page",
        rename(deserialize = "total_pages", serialize = "totalPages")
    )]
    pub total_pages: usize,
    #[serde(default)]
    pub total: usize,
}

fn default_page() -> usize {
    1
}

fn hermes_query_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (*byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesModelProvider {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub authenticated: Option<bool>,
    #[serde(default, rename(deserialize = "is_current", serialize = "isCurrent"))]
    pub is_current: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesModelOptions {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub providers: Vec<HermesModelProvider>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesUsageDaily {
    #[serde(default)]
    pub day: String,
    #[serde(default, alias = "input_tokens")]
    pub input_tokens: u64,
    #[serde(default, alias = "output_tokens")]
    pub output_tokens: u64,
    #[serde(default, alias = "cache_read_tokens")]
    pub cache_read_tokens: u64,
    #[serde(default, alias = "reasoning_tokens")]
    pub reasoning_tokens: u64,
    #[serde(default, alias = "estimated_cost")]
    pub estimated_cost: f64,
    #[serde(default, alias = "actual_cost")]
    pub actual_cost: f64,
    #[serde(default)]
    pub sessions: u64,
    #[serde(default, alias = "api_calls")]
    pub api_calls: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesUsageModel {
    #[serde(default)]
    pub model: String,
    #[serde(default, alias = "input_tokens")]
    pub input_tokens: u64,
    #[serde(default, alias = "output_tokens")]
    pub output_tokens: u64,
    #[serde(default, alias = "estimated_cost")]
    pub estimated_cost: f64,
    #[serde(default)]
    pub sessions: u64,
    #[serde(default, alias = "api_calls")]
    pub api_calls: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesUsageTotals {
    #[serde(default, alias = "total_input")]
    pub total_input: u64,
    #[serde(default, alias = "total_output")]
    pub total_output: u64,
    #[serde(default, alias = "total_cache_read")]
    pub total_cache_read: u64,
    #[serde(default, alias = "total_reasoning")]
    pub total_reasoning: u64,
    #[serde(default, alias = "total_estimated_cost")]
    pub total_estimated_cost: f64,
    #[serde(default, alias = "total_actual_cost")]
    pub total_actual_cost: f64,
    #[serde(default, alias = "total_sessions")]
    pub total_sessions: u64,
    #[serde(default, alias = "total_api_calls")]
    pub total_api_calls: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesUsageReport {
    #[serde(default)]
    pub daily: Vec<HermesUsageDaily>,
    #[serde(default, alias = "by_model")]
    pub by_model: Vec<HermesUsageModel>,
    #[serde(default)]
    pub totals: HermesUsageTotals,
    #[serde(default, alias = "period_days")]
    pub period_days: u16,
}

fn normalize_hermes_usage_nulls(value: &mut serde_json::Value) {
    const DAILY_NUMBERS: &[&str] = &[
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "reasoning_tokens",
        "estimated_cost",
        "actual_cost",
        "sessions",
        "api_calls",
    ];
    const MODEL_NUMBERS: &[&str] = &[
        "input_tokens",
        "output_tokens",
        "estimated_cost",
        "sessions",
        "api_calls",
    ];
    const TOTAL_NUMBERS: &[&str] = &[
        "total_input",
        "total_output",
        "total_cache_read",
        "total_reasoning",
        "total_estimated_cost",
        "total_actual_cost",
        "total_sessions",
        "total_api_calls",
    ];
    for (section, keys) in [("daily", DAILY_NUMBERS), ("by_model", MODEL_NUMBERS)] {
        if let Some(rows) = value
            .get_mut(section)
            .and_then(serde_json::Value::as_array_mut)
        {
            for row in rows {
                if let Some(object) = row.as_object_mut() {
                    for key in keys {
                        if object.get(*key).is_some_and(serde_json::Value::is_null) {
                            object.insert((*key).to_string(), serde_json::json!(0));
                        }
                    }
                }
            }
        }
    }
    if let Some(totals) = value
        .get_mut("totals")
        .and_then(serde_json::Value::as_object_mut)
    {
        for key in TOTAL_NUMBERS {
            if totals.get(*key).is_some_and(serde_json::Value::is_null) {
                totals.insert((*key).to_string(), serde_json::json!(0));
            }
        }
    }
}

async fn load_hermes_model_options() -> Result<HermesModelOptions, String> {
    let endpoint = crate::agent::hermes::HermesGatewayEndpoint::from_env()
        .ok_or_else(|| "Hermes Agent 未连接".to_string())?;
    let mut gateway =
        crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
            .map_err(|error| error.to_string())?;
    let value = gateway
        // 与 Hermes Desktop 的 ModelPickerDialog 保持同一查询语义。Runtime 会
        // 返回完整 Provider 目录以及每个 Provider 的 authenticated 状态；
        // UI 只允许 authenticated=true 的 Provider。None 表示 Runtime 尚未确认
        // 凭据可用，不能因为它不是 false 就当成可执行模型。
        .call(
            "model.options",
            serde_json::json!({"include_unconfigured": true}),
        )
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_value(value).map_err(|error| format!("Hermes model.options 无效: {error}"))
}

fn settings_provider_candidates(runtime_slug: &str) -> Vec<&str> {
    match runtime_slug.to_ascii_lowercase().as_str() {
        "moonshot" | "moonshotai" | "kimi-coding" => vec![runtime_slug, "kimi"],
        "dashscope" | "aliyun" | "qwen" => vec![runtime_slug, "alibaba"],
        "glm" | "zhipu" | "z-ai" => vec![runtime_slug, "zai"],
        "minimax" => vec![runtime_slug, "minimax-cn"],
        _ => vec![runtime_slug],
    }
}

fn settings_provider_matches_runtime(provider_id: &str, runtime_slug: &str) -> bool {
    settings_provider_candidates(runtime_slug)
        .into_iter()
        .any(|candidate| {
            provider_id == candidate
                || provider_id
                    .strip_prefix(candidate)
                    .and_then(|suffix| suffix.strip_prefix('-'))
                    .is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
                    })
        })
}

fn limit_hermes_model_options(
    mut options: HermesModelOptions,
    allowlist: &HashMap<String, Vec<String>>,
) -> HermesModelOptions {
    options.providers = options
        .providers
        .into_iter()
        .filter_map(|mut provider| {
            let models = allowlist.get(&provider.slug)?;
            // 设置中的模型清单是用户确认后的真相源。Runtime 的发现目录可能滞后，
            // 不应因此让已配置模型从 Chat 选择器消失。
            provider.models = models.clone();
            provider.authenticated = Some(true);
            (!provider.models.is_empty()).then_some(provider)
        })
        .collect();

    let selected_provider = options
        .provider
        .as_deref()
        .and_then(|slug| {
            options
                .providers
                .iter()
                .find(|provider| provider.slug == slug)
        })
        .or_else(|| options.providers.first());
    let selected_model = selected_provider.and_then(|provider| {
        options
            .model
            .as_ref()
            .filter(|model| provider.models.iter().any(|allowed| allowed == *model))
            .cloned()
            .or_else(|| provider.models.first().cloned())
    });
    options.provider = selected_provider.map(|provider| provider.slug.clone());
    options.model = selected_model;
    options
}

fn configured_hermes_model_options(
    app: &AppHandle,
    options: HermesModelOptions,
) -> HermesModelOptions {
    let mut allowlist = HashMap::new();
    let snapshots =
        crate::model::openai_compat::configured_provider_snapshots(app).unwrap_or_default();
    for runtime_provider in &options.providers {
        let mut models = Vec::new();
        for snapshot in snapshots.iter().filter(|snapshot| {
            settings_provider_matches_runtime(&snapshot.id, &runtime_provider.slug)
        }) {
            let credential_ready = !snapshot.requires_key
                || crate::commands::get_cached_api_key(app, &snapshot.id)
                    .is_ok_and(|key| !key.is_empty());
            if !credential_ready {
                continue;
            }
            for model in &snapshot.models {
                if !models.contains(model) {
                    models.push(model.clone());
                }
            }
        }
        if !models.is_empty() {
            allowlist.insert(runtime_provider.slug.clone(), models);
        }
    }
    limit_hermes_model_options(options, &allowlist)
}

async fn load_configured_hermes_model_options(
    app: &AppHandle,
) -> Result<HermesModelOptions, String> {
    load_hermes_model_options()
        .await
        .map(|options| configured_hermes_model_options(app, options))
}

fn resolve_hermes_model(
    options: &HermesModelOptions,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> Result<(String, String), String> {
    let providers = options
        .providers
        .iter()
        .filter(|provider| provider.authenticated == Some(true) && !provider.models.is_empty())
        .collect::<Vec<_>>();
    let provider = if let Some(slug) = requested_provider {
        providers
            .iter()
            .copied()
            .find(|provider| provider.slug == slug)
    } else if let Some(model) = requested_model {
        providers
            .iter()
            .copied()
            .find(|provider| {
                options.provider.as_deref() == Some(provider.slug.as_str())
                    && provider.models.iter().any(|item| item == model)
            })
            .or_else(|| {
                providers
                    .iter()
                    .copied()
                    .find(|provider| provider.models.iter().any(|item| item == model))
            })
    } else {
        options
            .provider
            .as_deref()
            .and_then(|slug| {
                providers
                    .iter()
                    .copied()
                    .find(|provider| provider.slug == slug && provider.authenticated == Some(true))
            })
            .or_else(|| {
                providers
                    .iter()
                    .copied()
                    .find(|provider| provider.authenticated == Some(true))
            })
            .or_else(|| providers.first().copied())
    }
    .ok_or_else(|| "Hermes 没有已配置且可用的模型供应商".to_string())?;

    let model = requested_model
        .map(str::to_string)
        .or_else(|| {
            (options.provider.as_deref() == Some(provider.slug.as_str()))
                .then(|| options.model.clone())
                .flatten()
        })
        .or_else(|| provider.models.first().cloned())
        .ok_or_else(|| format!("Hermes 供应商 {} 没有可用模型", provider.name))?;
    if !provider.models.iter().any(|item| item == &model) {
        return Err(format!(
            "模型 {model} 不在 Hermes 供应商 {} 的已配置模型中",
            provider.name
        ));
    }
    Ok((provider.slug.clone(), model))
}

/// 会话模型是 Hermes 可执行目录与 SophoNote 设置中已配置凭据/模型的交集。
#[tauri::command]
pub async fn agent_hermes_models(app: AppHandle) -> ApiResponse<HermesModelOptions> {
    match load_configured_hermes_model_options(&app).await {
        Ok(options) => ApiResponse::ok(options),
        Err(error) => ApiResponse::err(error),
    }
}

/// 设置页读取 Runtime 完整发现目录；该目录只用于补充配置，不代表模型可执行。
#[tauri::command]
pub async fn agent_hermes_model_catalog() -> ApiResponse<HermesModelOptions> {
    match load_hermes_model_options().await {
        Ok(options) => ApiResponse::ok(options),
        Err(error) => ApiResponse::err(error),
    }
}

/// Hermes Runtime 精确用量账本。SophoNote 不从正文估算 Token，也不复制明细表。
#[tauri::command]
pub async fn agent_hermes_usage(days: u16) -> ApiResponse<HermesUsageReport> {
    if !matches!(days, 7 | 30 | 90) {
        return ApiResponse::err("用量统计时间范围只支持 7、30 或 90 天".into());
    }
    let mut value = match hermes_dashboard_request(
        reqwest::Method::GET,
        &format!("api/analytics/usage?days={days}"),
        None,
        std::time::Duration::from_secs(20),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    normalize_hermes_usage_nulls(&mut value);
    match serde_json::from_value::<HermesUsageReport>(value) {
        Ok(mut report) => {
            report.period_days = days;
            report.by_model.retain(|row| !row.model.trim().is_empty());
            // Runtime 的 by_model 已合并 vision/compression/title 等辅助调用，
            // totals/daily 仍只聚合主 Session。摘要取两者较大值，避免辅助模型
            // 已出现在明细中但总 Token 反而更小；会话数保持主 Session 口径。
            let model_input = report
                .by_model
                .iter()
                .map(|row| row.input_tokens)
                .sum::<u64>();
            let model_output = report
                .by_model
                .iter()
                .map(|row| row.output_tokens)
                .sum::<u64>();
            let model_calls = report.by_model.iter().map(|row| row.api_calls).sum::<u64>();
            let model_cost = report
                .by_model
                .iter()
                .map(|row| row.estimated_cost)
                .sum::<f64>();
            report.totals.total_input = report.totals.total_input.max(model_input);
            report.totals.total_output = report.totals.total_output.max(model_output);
            report.totals.total_api_calls = report.totals.total_api_calls.max(model_calls);
            report.totals.total_estimated_cost = report.totals.total_estimated_cost.max(model_cost);
            ApiResponse::ok(report)
        }
        Err(error) => ApiResponse::err(format!("Hermes 用量响应无效: {error}")),
    }
}

fn hermes_cron_text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

type HermesProjectWorkdirs = HashMap<String, (String, String)>;

fn hermes_project_workspace_root(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("hermes").join("workspaces").join("projects"))
        .map_err(|error| format!("读取 SophoNote 项目工作目录失败: {error}"))
}

fn hermes_project_workdirs(app: &AppHandle) -> Result<HermesProjectWorkdirs, String> {
    let root = hermes_project_workspace_root(app)?;
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("读取项目列表失败: {error}"))?;
    let mut stmt = conn
        .prepare("SELECT id, name FROM projects")
        .map_err(|error| format!("读取项目列表失败: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("读取项目列表失败: {error}"))?;
    let mut result = HashMap::new();
    for row in rows {
        let (id, name) = row.map_err(|error| format!("读取项目列表失败: {error}"))?;
        result.insert(root.join(&id).to_string_lossy().into_owned(), (id, name));
    }
    Ok(result)
}

fn hermes_cron_project_workdir(
    app: &AppHandle,
    project_id: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(project_id) = project_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if project_id.len() > 128
        || !project_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err("项目标识无效".into());
    }
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("读取项目失败: {error}"))?;
    let exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            rusqlite::params![project_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| format!("读取项目失败: {error}"))?;
    if !exists {
        return Err("所属项目不存在".into());
    }
    let path = hermes_project_workspace_root(app)?.join(project_id);
    std::fs::create_dir_all(&path).map_err(|error| format!("创建项目工作目录失败: {error}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

fn project_hermes_cron_job(
    value: &serde_json::Value,
    project_workdirs: &HermesProjectWorkdirs,
) -> Option<HermesCronJobInfo> {
    let id = hermes_cron_text(value, "id")?;
    let enabled = value
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let state = hermes_cron_text(value, "state").unwrap_or_else(|| "scheduled".into());
    let last_status = hermes_cron_text(value, "last_status");
    let execution_status = value
        .get("latest_execution")
        .and_then(|execution| hermes_cron_text(execution, "status"));
    let next_run_at = hermes_cron_text(value, "next_run_at");
    let status =
        if !enabled || state == "paused" || value.get("paused_at").is_some_and(|v| !v.is_null()) {
            "paused"
        } else if execution_status
            .as_deref()
            .is_some_and(|status| matches!(status, "claimed" | "running"))
        {
            "running"
        } else if state == "completed" {
            "completed"
        } else if last_status.as_deref().is_some_and(|status| status != "ok") {
            "error"
        } else if next_run_at.is_none() {
            "completed"
        } else {
            "active"
        }
        .to_string();
    let skills = value
        .get("skills")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect();
    let schedule_spec = value
        .get("schedule")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let schedule_kind = schedule_spec
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("custom")
        .to_string();
    let project = hermes_cron_text(value, "workdir")
        .and_then(|workdir| project_workdirs.get(&workdir).cloned());

    Some(HermesCronJobInfo {
        id,
        name: hermes_cron_text(value, "name").unwrap_or_else(|| "未命名计划任务".into()),
        prompt: hermes_cron_text(value, "prompt").unwrap_or_default(),
        schedule: hermes_cron_text(value, "schedule_display")
            .or_else(|| hermes_cron_text(value, "schedule"))
            .unwrap_or_else(|| "未设置".into()),
        schedule_kind,
        schedule_spec,
        status,
        enabled,
        next_run_at,
        last_run_at: hermes_cron_text(value, "last_run_at"),
        last_status,
        last_error: hermes_cron_text(value, "last_error")
            .or_else(|| hermes_cron_text(value, "last_delivery_error")),
        skills,
        profile: hermes_cron_text(value, "profile").unwrap_or_else(|| "default".into()),
        execution_status,
        created_at: hermes_cron_text(value, "created_at"),
        project_id: project.as_ref().map(|(id, _)| id.clone()),
        project_name: project.map(|(_, name)| name),
        provider: hermes_cron_text(value, "provider"),
        model: hermes_cron_text(value, "model"),
    })
}

/// 计划任务只读取 Hermes Cron 真相源；SophoNote 不落第二份 scheduler 表。
#[tauri::command]
pub async fn agent_hermes_cron_jobs(app: AppHandle) -> ApiResponse<Vec<HermesCronJobInfo>> {
    let project_workdirs = match hermes_project_workdirs(&app) {
        Ok(workdirs) => workdirs,
        Err(error) => return ApiResponse::err(error),
    };
    if let Err(error) = reconcile_hermes_cron_jobs(&app).await {
        // 目录仍可读取；任务行会保留缺失字段/最近错误，避免一次对账失败让整个
        // 计划任务页面消失。触发入口还会再次执行同一对账。
        eprintln!("[hermes] cron reconciliation failed: {error}");
    }
    match hermes_dashboard_request(
        reqwest::Method::GET,
        "api/cron/jobs?profile=all",
        None,
        std::time::Duration::from_secs(15),
    )
    .await
    {
        Ok(value) => {
            let Some(items) = value.as_array() else {
                return ApiResponse::err("Hermes Cron 返回无效：预期任务数组".into());
            };
            let mut jobs = items
                .iter()
                .filter_map(|value| project_hermes_cron_job(value, &project_workdirs))
                .collect::<Vec<_>>();
            jobs.sort_by(|left, right| {
                left.next_run_at
                    .is_none()
                    .cmp(&right.next_run_at.is_none())
                    .then_with(|| left.next_run_at.cmp(&right.next_run_at))
                    .then_with(|| left.name.cmp(&right.name))
            });
            ApiResponse::ok(jobs)
        }
        Err(error) => ApiResponse::err(error),
    }
}

fn validate_hermes_cron_identity(id: &str, profile: &str) -> Result<(String, String), String> {
    let id = id.trim();
    let profile = profile.trim();
    let safe = |value: &str, max: usize| {
        !value.is_empty()
            && value.len() <= max
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    };
    if !safe(id, 160) {
        return Err("计划任务标识无效".into());
    }
    if !safe(profile, 96) {
        return Err("Hermes Profile 无效".into());
    }
    Ok((id.to_string(), profile.to_string()))
}

fn validate_hermes_cron_draft(draft: &HermesCronDraft) -> Result<(), String> {
    let name = draft.name.trim();
    let prompt = draft.prompt.trim();
    let schedule = draft.schedule.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err("任务名称需为 1–80 个可见字符".into());
    }
    if prompt.is_empty() || prompt.chars().count() > 12_000 {
        return Err("任务内容需为 1–12000 个字符".into());
    }
    if schedule.is_empty()
        || schedule.chars().count() > 256
        || schedule.chars().any(char::is_control)
    {
        return Err("执行计划需为 1–256 个可见字符".into());
    }
    if draft.skills.len() > 24
        || draft.skills.iter().any(|skill| {
            let skill = skill.trim();
            skill.is_empty()
                || skill.len() > 128
                || !skill
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
    {
        return Err("关联 Skill 列表无效".into());
    }
    for (label, value) in [("模型供应商", &draft.provider), ("模型", &draft.model)] {
        if value.as_deref().is_some_and(|text| {
            text.trim().is_empty() || text.len() > 256 || text.chars().any(char::is_control)
        }) {
            return Err(format!("{label}无效"));
        }
    }
    Ok(())
}

async fn resolve_hermes_cron_draft_model(
    app: &AppHandle,
    draft: &HermesCronDraft,
) -> Result<(Option<String>, Option<String>), String> {
    let provider = draft
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let model = draft
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (provider, model) {
        (None, None) => Ok((None, None)),
        (Some(_), None) | (None, Some(_)) => Err("计划任务的模型供应商与模型必须同时配置".into()),
        (Some(provider), Some(model)) => {
            let options = load_configured_hermes_model_options(app).await?;
            let (provider, model) = resolve_hermes_model(&options, Some(provider), Some(model))?;
            Ok((Some(provider), Some(model)))
        }
    }
}

fn hermes_cron_model_is_configured(
    options: &HermesModelOptions,
    value: &serde_json::Value,
) -> bool {
    let Some(provider) = hermes_cron_text(value, "provider") else {
        return false;
    };
    let Some(model) = hermes_cron_text(value, "model") else {
        return false;
    };
    resolve_hermes_model(options, Some(&provider), Some(&model)).is_ok()
}

const AI_RADAR_DAILY_INTRO: &str =
    "请使用「sophonote-ai-radar」Skill 完成每日高质量发现，并严格按照该 Skill 的 Markdown 说明执行。";
const OPENROUTER_RANKINGS_PROMPT: &str = "请使用「sophonote-openrouter-rankings」Skill 更新完整的 OpenRouter 模型榜快照，并严格按照该 Skill 的 Markdown 说明执行。\n\n仅在榜单发生变化时通知。";

fn cron_prompt_value<'a>(prompt: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("{key}=");
    let start = prompt.find(&marker)? + marker.len();
    let rest = &prompt[start..];
    if rest.starts_with('[') {
        let end = rest.find(']')? + 1;
        return Some(&rest[..end]);
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

fn cron_prompt_list(prompt: &str, key: &str) -> Vec<String> {
    cron_prompt_value(prompt, key)
        .map(|value| value.trim_matches(['[', ']']))
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn chinese_list(values: &[String]) -> String {
    values.join("、")
}

fn source_label(value: &str) -> String {
    match value {
        "github" => "GitHub".into(),
        "arxiv" => "arXiv".into(),
        "hackernews" => "Hacker News".into(),
        "producthunt" => "Product Hunt".into(),
        "huggingface" => "Hugging Face".into(),
        "aihot" => "AI 热榜".into(),
        other => other.to_string(),
    }
}

fn lane_label(value: &str) -> String {
    match value {
        "github" => "GitHub 项目".into(),
        "model" => "模型与研究".into(),
        "product" => "AI 产品".into(),
        other => other.to_string(),
    }
}

fn natural_daily_prompt(prompt: &str) -> String {
    let expanded = expand_daily_sources_with_aihot(prompt);
    let sources = cron_prompt_list(&expanded, "sources")
        .iter()
        .map(|value| source_label(value))
        .collect::<Vec<_>>();
    let lanes = cron_prompt_list(&expanded, "lanes")
        .iter()
        .map(|value| lane_label(value))
        .collect::<Vec<_>>();
    let limit = cron_prompt_value(&expanded, "prefilterLimitPerSource")
        .or_else(|| cron_prompt_value(&expanded, "countPerLane"))
        .unwrap_or("4");
    let language = match cron_prompt_value(&expanded, "language") {
        Some("zh-CN" | "zh") | None => "使用中文生成结果。".to_string(),
        Some(value) => format!("使用 {value} 生成结果。"),
    };
    let notification = match cron_prompt_value(&expanded, "notification") {
        Some("qualified-only") | None => "仅在发现合格内容时通知。".to_string(),
        Some(value) => format!("按 {value} 规则通知。"),
    };
    let source_requirement = if sources.is_empty() {
        "使用 Skill 中定义的默认信源采集候选内容。".to_string()
    } else {
        format!("从 {} 采集候选内容。", chinese_list(&sources))
    };
    let lane_requirement = if lanes.is_empty() {
        "按照 Skill 中定义的默认视角完成筛选。".to_string()
    } else {
        format!("按 {} 视角完成筛选。", chinese_list(&lanes))
    };
    format!(
        "{AI_RADAR_DAILY_INTRO}\n\n具体要求：\n- {source_requirement}\n- {lane_requirement}\n- 每个信源最多预筛 {limit} 条候选内容。\n- {language}\n- {notification}"
    )
}

fn natural_report_prompt(prompt: &str) -> String {
    let (period, label) = match cron_prompt_value(prompt, "period") {
        Some("weekly") => ("本周", "AI 周报"),
        Some("monthly") => ("本月", "AI 月报"),
        _ => ("当日", "AI 日报"),
    };
    let date = cron_prompt_value(prompt, "date")
        .map(|value| format!("\n\n报告日期指定为 {value}。"))
        .unwrap_or_default();
    format!(
        "请使用「sophonote-ai-radar」Skill 生成并保存{period} {label}，并严格按照该 Skill 的 Markdown 说明执行。\n\n使用中文生成报告。{date}"
    )
}

fn migrate_cron_prompt_to_natural_language(prompt: &str, skills: &[String]) -> Option<String> {
    let branded_prompt = prompt
        .replace("mindbox-ai-radar", "sophonote-ai-radar")
        .replace(
            "mindbox-openrouter-rankings",
            "sophonote-openrouter-rankings",
        );
    let migrated = if skills.iter().any(|skill| {
        matches!(
            skill.as_str(),
            "sophonote-ai-radar" | "sophonote-discovery-subscriptions" | "mindbox-ai-radar"
        )
    }) {
        if branded_prompt.contains("action=daily") {
            Some(natural_daily_prompt(&branded_prompt))
        } else if branded_prompt.contains("action=report") {
            Some(natural_report_prompt(&branded_prompt))
        } else {
            Some(branded_prompt.clone())
        }
    } else if skills.iter().any(|skill| {
        matches!(
            skill.as_str(),
            "sophonote-openrouter-rankings" | "mindbox-openrouter-rankings"
        )
    })
        && (branded_prompt.contains("action=refresh")
            || branded_prompt.contains("action=model-board"))
    {
        Some(OPENROUTER_RANKINGS_PROMPT.into())
    } else {
        Some(branded_prompt.clone())
    };
    migrated.filter(|value| value != prompt)
}

/// 在 `sources=[...]` 列表中追加缺失的 `aihot`（幂等；无 sources 声明则不动）。
/// 用户显式选择保留——这里只做新信源上线的一次性能力扩展；引号风格随原列表。
fn expand_daily_sources_with_aihot(prompt: &str) -> String {
    let Some(start) = prompt.find("sources=[") else {
        return prompt.to_string();
    };
    let list_start = start + "sources=[".len();
    let Some(relative_end) = prompt[list_start..].find(']') else {
        return prompt.to_string();
    };
    let list_end = list_start + relative_end;
    let list = &prompt[list_start..list_end];
    let has_aihot = list
        .split(',')
        .any(|key| key.trim().trim_matches('"') == "aihot");
    if has_aihot {
        return prompt.to_string();
    }
    let (separator, new_key) = if list.trim().is_empty() {
        ("", "aihot")
    } else if list.contains('"') {
        (",", "\"aihot\"")
    } else {
        (",", "aihot")
    };
    format!(
        "{}sources=[{list}{separator}{new_key}]{}",
        &prompt[..start],
        &prompt[list_end + 1..]
    )
}

const DISCOVERY_DEEP_BACKFILL_JOB_NAME: &str = "AI 动态深度解读补全";
const OPENROUTER_RANKINGS_JOB_NAME: &str = "OpenRouter 模型榜更新";
const OPENROUTER_RANKINGS_SCHEDULE: &str = "30 8 * * *";

fn is_discovery_deep_backfill_job(item: &serde_json::Value) -> bool {
    hermes_cron_text(item, "name").as_deref() == Some(DISCOVERY_DEEP_BACKFILL_JOB_NAME)
        || hermes_cron_text(item, "prompt")
            .is_some_and(|prompt| prompt.contains("action=backfill-deep"))
}

fn is_openrouter_rankings_job(item: &serde_json::Value) -> bool {
    let has_skill = item
        .get("skills")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .any(|skill| {
            matches!(
                skill,
                "sophonote-openrouter-rankings" | "mindbox-openrouter-rankings"
            )
        });
    let legacy_model_board = hermes_cron_text(item, "prompt")
        .is_some_and(|prompt| prompt.contains("action=model-board"));
    has_skill || legacy_model_board
}

/// 默认模型目录可能暂时把 moa/default 投影为当前项；发现域已有可执行 Cron 时，
/// 新补全任务优先继承它的真实 provider/model，避免把虚拟占位写进新任务。
fn resolve_discovery_cron_model(
    options: &HermesModelOptions,
    items: &[serde_json::Value],
) -> Result<(String, String), String> {
    let configured = items.iter().find_map(|item| {
        let has_radar_skill = item
            .get("skills")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .any(|skill| skill == "sophonote-ai-radar");
        let provider = hermes_cron_text(item, "provider")?;
        let model = hermes_cron_text(item, "model")?;
        (has_radar_skill && !(provider == "moa" && model == "default")).then_some((provider, model))
    });
    match configured {
        Some((provider, model)) => resolve_hermes_model(options, Some(&provider), Some(&model)),
        None => resolve_hermes_model(options, None, None),
    }
}

/// 深度补全已并入每日发现 Run。清理旧的独立高频任务，避免空队列空转及与
/// daily 同时更新同一条目；Hermes Cron 仍是任务删除与审计的唯一真相源。
async fn remove_discovery_deep_backfill_jobs(items: &[serde_json::Value]) -> Result<usize, String> {
    let matching_jobs = items
        .iter()
        .filter(|item| is_discovery_deep_backfill_job(item))
        .collect::<Vec<_>>();
    let mut removed = 0usize;
    for job in matching_jobs {
        let Some(id) = hermes_cron_text(job, "id") else {
            continue;
        };
        let profile = hermes_cron_text(job, "profile").unwrap_or_else(|| "default".into());
        let (id, profile) = validate_hermes_cron_identity(&id, &profile)?;
        let path = hermes_cron_job_path(&id, &profile, "");
        hermes_dashboard_request(
            reqwest::Method::DELETE,
            &path,
            None,
            std::time::Duration::from_secs(20),
        )
        .await?;
        removed += 1;
    }
    Ok(removed)
}

async fn ensure_openrouter_rankings_job(
    app: &AppHandle,
    options: &HermesModelOptions,
    items: &[serde_json::Value],
) -> Result<bool, String> {
    if items.iter().any(is_openrouter_rankings_job) {
        return Ok(false);
    }
    let Ok((provider, model)) = resolve_discovery_cron_model(options, items) else {
        // 没有已配置模型时不替用户创建一个会立即失败的后台任务。已有任务由
        // reconcile_hermes_cron_jobs 原位保留并暂停，配置模型后可在管理面恢复。
        return Ok(false);
    };
    let workdir = hermes_cron_project_workdir(app, None)?;
    hermes_dashboard_request(
        reqwest::Method::POST,
        "api/cron/jobs",
        Some(serde_json::json!({
            "name": OPENROUTER_RANKINGS_JOB_NAME,
            "prompt": OPENROUTER_RANKINGS_PROMPT,
            "schedule": OPENROUTER_RANKINGS_SCHEDULE,
            "deliver": "local",
            "skills": ["sophonote-openrouter-rankings"],
            "workdir": workdir,
            "provider": provider,
            "model": model,
        })),
        std::time::Duration::from_secs(20),
    )
    .await?;
    Ok(true)
}

/// 修复旧 Hermes Cron 任务的 Skill/Prompt，并清理已并入 daily 的独立 deep
/// 补全任务。未绑定可执行模型的任务只原位暂停，不删除、不补猜默认模型。
///
/// 包内 Hermes Home 由 SophoNote 私有持有，Cron 又是唯一任务真相源；因此这里
/// 直接补写/创建原生任务，不建立迁移表，也不依赖 config.yaml 的第二份默认模型。
pub(crate) async fn reconcile_hermes_cron_jobs(app: &AppHandle) -> Result<usize, String> {
    // 启动补全与计划任务页读取可能同时发生；上游 Cron create 没有幂等 key，
    // 所以必须把「列出→修复→必要时创建」序列化在本进程内。
    static RECONCILIATION_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _guard = RECONCILIATION_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let options = load_configured_hermes_model_options(app).await?;
    let value = hermes_dashboard_request(
        reqwest::Method::GET,
        "api/cron/jobs?profile=all",
        None,
        std::time::Duration::from_secs(15),
    )
    .await?;
    let items = value
        .as_array()
        .ok_or_else(|| "Hermes Cron 返回无效：预期任务数组".to_string())?;
    let mut repaired = 0usize;
    for item in items {
        let job_skills: Vec<String> = item
            .get("skills")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect();
        let is_openrouter_job = is_openrouter_rankings_job(item);
        let needs_openrouter_repair = is_openrouter_job
            && (hermes_cron_text(item, "name").as_deref() != Some(OPENROUTER_RANKINGS_JOB_NAME)
                || hermes_cron_text(item, "prompt").as_deref() != Some(OPENROUTER_RANKINGS_PROMPT)
                || hermes_cron_text(item, "schedule").as_deref()
                    != Some(OPENROUTER_RANKINGS_SCHEDULE)
                || job_skills.len() != 1
                || job_skills.first().map(String::as_str) != Some("sophonote-openrouter-rankings"));
        // 发现域 Skill 已融合更名为 sophonote-ai-radar；旧任务自动改指，
        // 用户无需手动重建计划任务。
        let needs_skill_repoint = job_skills.iter().any(|skill| {
            matches!(
                skill.as_str(),
                "sophonote-discovery-subscriptions" | "mindbox-ai-radar"
            )
        });
        let migrated_prompt = hermes_cron_text(item, "prompt")
            .and_then(|prompt| migrate_cron_prompt_to_natural_language(&prompt, &job_skills));
        let model_configured = hermes_cron_model_is_configured(&options, item);
        let already_paused = !item
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
            || hermes_cron_text(item, "state").as_deref() == Some("paused")
            || item.get("paused_at").is_some_and(|value| !value.is_null());
        let needs_model_pause = !model_configured && !already_paused;
        if !needs_model_pause
            && !needs_skill_repoint
            && migrated_prompt.is_none()
            && !needs_openrouter_repair
        {
            continue;
        }
        let Some(id) = hermes_cron_text(item, "id") else {
            continue;
        };
        let profile = hermes_cron_text(item, "profile").unwrap_or_else(|| "default".into());
        let (id, profile) = validate_hermes_cron_identity(&id, &profile)?;
        let mut updates = serde_json::Map::new();
        if let Some(prompt) = migrated_prompt {
            updates.insert("prompt".into(), serde_json::Value::String(prompt));
        }
        if needs_openrouter_repair {
            updates.insert(
                "name".into(),
                serde_json::Value::String(OPENROUTER_RANKINGS_JOB_NAME.into()),
            );
            updates.insert(
                "prompt".into(),
                serde_json::Value::String(OPENROUTER_RANKINGS_PROMPT.into()),
            );
            updates.insert(
                "schedule".into(),
                serde_json::Value::String(OPENROUTER_RANKINGS_SCHEDULE.into()),
            );
            updates.insert(
                "skills".into(),
                serde_json::json!(["sophonote-openrouter-rankings"]),
            );
        } else if needs_skill_repoint {
            updates.insert("skills".into(), serde_json::json!(["sophonote-ai-radar"]));
        }
        let path = hermes_cron_job_path(&id, &profile, "");
        if !updates.is_empty() {
            hermes_dashboard_request(
                reqwest::Method::PUT,
                &path,
                Some(serde_json::json!({
                    "updates": updates
                })),
                std::time::Duration::from_secs(20),
            )
            .await?;
            repaired += 1;
        }
        if needs_model_pause {
            let pause_path = hermes_cron_job_path(&id, &profile, "/pause");
            hermes_dashboard_request(
                reqwest::Method::POST,
                &pause_path,
                None,
                std::time::Duration::from_secs(20),
            )
            .await?;
            repaired += 1;
        }
    }
    repaired += remove_discovery_deep_backfill_jobs(items).await?;
    if ensure_openrouter_rankings_job(app, &options, items).await? {
        repaired += 1;
    }
    Ok(repaired)
}

fn hermes_cron_job_path(id: &str, profile: &str, suffix: &str) -> String {
    format!(
        "api/cron/jobs/{}{}?profile={}",
        hermes_query_encode(id),
        suffix,
        hermes_query_encode(profile)
    )
}

async fn project_hermes_cron_response(
    app: &AppHandle,
    value: serde_json::Value,
) -> Result<HermesCronJobInfo, String> {
    let workdirs = hermes_project_workdirs(app)?;
    project_hermes_cron_job(&value, &workdirs).ok_or_else(|| "Hermes Cron 返回无效任务".to_string())
}

/// 表单只写 Hermes Cron；固定本地交付，不创建 SophoNote scheduler 记录。
#[tauri::command]
pub async fn agent_hermes_cron_create(
    app: AppHandle,
    draft: HermesCronDraft,
) -> ApiResponse<HermesCronJobInfo> {
    if let Err(error) = validate_hermes_cron_draft(&draft) {
        return ApiResponse::err(error);
    }
    let workdir = match hermes_cron_project_workdir(&app, draft.project_id.as_deref()) {
        Ok(workdir) => workdir,
        Err(error) => return ApiResponse::err(error),
    };
    let (provider, model) = match resolve_hermes_cron_draft_model(&app, &draft).await {
        Ok(selection) => selection,
        Err(error) => return ApiResponse::err(error),
    };
    let pause_after_create = draft.start_paused || provider.is_none() || model.is_none();
    let value = hermes_dashboard_request(
        reqwest::Method::POST,
        "api/cron/jobs",
        Some(serde_json::json!({
            "name": draft.name.trim(),
            "prompt": draft.prompt.trim(),
            "schedule": draft.schedule.trim(),
            "deliver": "local",
            "skills": draft.skills,
            "workdir": workdir,
            "provider": provider,
            "model": model,
        })),
        std::time::Duration::from_secs(20),
    )
    .await;
    match value {
        Ok(value) => match project_hermes_cron_response(&app, value).await {
            Ok(job) if pause_after_create => {
                match agent_hermes_cron_action(&app, &job.id, &job.profile, "/pause").await {
                    Ok(paused) => ApiResponse::ok(paused),
                    Err(error) => {
                        ApiResponse::err(format!("计划任务已创建，但按暂停策略处理失败: {error}"))
                    }
                }
            }
            Ok(job) => ApiResponse::ok(job),
            Err(error) => ApiResponse::err(error),
        },
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_hermes_cron_update(
    app: AppHandle,
    id: String,
    profile: String,
    draft: HermesCronDraft,
) -> ApiResponse<HermesCronJobInfo> {
    if let Err(error) = validate_hermes_cron_draft(&draft) {
        return ApiResponse::err(error);
    }
    let (id, profile) = match validate_hermes_cron_identity(&id, &profile) {
        Ok(identity) => identity,
        Err(error) => return ApiResponse::err(error),
    };
    let workdir = match hermes_cron_project_workdir(&app, draft.project_id.as_deref()) {
        Ok(workdir) => workdir,
        Err(error) => return ApiResponse::err(error),
    };
    let (provider, model) = match resolve_hermes_cron_draft_model(&app, &draft).await {
        Ok(selection) => selection,
        Err(error) => return ApiResponse::err(error),
    };
    let pause_without_model = provider.is_none() || model.is_none();
    let path = hermes_cron_job_path(&id, &profile, "");
    let value = hermes_dashboard_request(
        reqwest::Method::PUT,
        &path,
        Some(serde_json::json!({
            "updates": {
                "name": draft.name.trim(),
                "prompt": draft.prompt.trim(),
                "schedule": draft.schedule.trim(),
                "skills": draft.skills,
                "workdir": workdir,
                "deliver": "local",
                "provider": provider,
                "model": model,
            }
        })),
        std::time::Duration::from_secs(20),
    )
    .await;
    match value {
        Ok(value) => match project_hermes_cron_response(&app, value).await {
            Ok(job) if pause_without_model => {
                match agent_hermes_cron_action(&app, &job.id, &job.profile, "/pause").await {
                    Ok(paused) => ApiResponse::ok(paused),
                    Err(error) => ApiResponse::err(format!(
                        "计划任务已更新，但未配置模型且自动暂停失败: {error}"
                    )),
                }
            }
            Ok(job) => ApiResponse::ok(job),
            Err(error) => ApiResponse::err(error),
        },
        Err(error) => ApiResponse::err(error),
    }
}

async fn agent_hermes_cron_action(
    app: &AppHandle,
    id: &str,
    profile: &str,
    suffix: &str,
) -> Result<HermesCronJobInfo, String> {
    let (id, profile) = validate_hermes_cron_identity(id, profile)?;
    let path = hermes_cron_job_path(&id, &profile, suffix);
    let value = hermes_dashboard_request(
        reqwest::Method::POST,
        &path,
        None,
        std::time::Duration::from_secs(20),
    )
    .await?;
    project_hermes_cron_response(app, value).await
}

async fn hermes_cron_has_active_run(id: &str, profile: &str) -> Result<bool, String> {
    let (id, profile) = validate_hermes_cron_identity(id, profile)?;
    let path = format!(
        "api/cron/jobs/{}/runs?profile={}&limit=1",
        hermes_query_encode(&id),
        hermes_query_encode(&profile)
    );
    let value = hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(20),
    )
    .await?;
    Ok(value
        .get("runs")
        .and_then(serde_json::Value::as_array)
        .and_then(|runs| runs.first())
        .and_then(|run| run.get("is_active"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

async fn ensure_hermes_cron_model_configured(
    app: &AppHandle,
    id: &str,
    profile: &str,
) -> Result<(), String> {
    let (id, profile) = validate_hermes_cron_identity(id, profile)?;
    let path = hermes_cron_job_path(&id, &profile, "");
    let value = hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(20),
    )
    .await?;
    let options = load_configured_hermes_model_options(app).await?;
    if hermes_cron_model_is_configured(&options, &value) {
        Ok(())
    } else {
        Err("该计划任务尚未配置可执行模型，请先在运行设置中选择模型。".into())
    }
}

#[tauri::command]
pub async fn agent_hermes_cron_set_enabled(
    app: AppHandle,
    id: String,
    profile: String,
    enabled: bool,
) -> ApiResponse<HermesCronJobInfo> {
    if enabled {
        if let Err(error) = ensure_hermes_cron_model_configured(&app, &id, &profile).await {
            return ApiResponse::err(error);
        }
    }
    match agent_hermes_cron_action(
        &app,
        &id,
        &profile,
        if enabled { "/resume" } else { "/pause" },
    )
    .await
    {
        Ok(job) => ApiResponse::ok(job),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_hermes_cron_trigger(
    app: AppHandle,
    id: String,
    profile: String,
) -> ApiResponse<HermesCronJobInfo> {
    if let Err(error) = reconcile_hermes_cron_jobs(&app).await {
        return ApiResponse::err(format!("计划任务对账失败: {error}"));
    }
    if let Err(error) = ensure_hermes_cron_model_configured(&app, &id, &profile).await {
        return ApiResponse::err(error);
    }
    match hermes_cron_has_active_run(&id, &profile).await {
        Ok(true) => return ApiResponse::err("该计划任务正在执行，请等待本轮完成后再运行。".into()),
        Ok(false) => {}
        Err(error) => return ApiResponse::err(format!("无法确认计划任务运行状态: {error}")),
    }
    match agent_hermes_cron_action(&app, &id, &profile, "/trigger").await {
        Ok(job) => ApiResponse::ok(job),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_hermes_cron_delete(id: String, profile: String) -> ApiResponse<String> {
    let (id, profile) = match validate_hermes_cron_identity(&id, &profile) {
        Ok(identity) => identity,
        Err(error) => return ApiResponse::err(error),
    };
    let path = hermes_cron_job_path(&id, &profile, "");
    match hermes_dashboard_request(
        reqwest::Method::DELETE,
        &path,
        None,
        std::time::Duration::from_secs(20),
    )
    .await
    {
        Ok(_) => ApiResponse::ok(id),
        Err(error) => ApiResponse::err(error),
    }
}

/// 孤儿收口投影：无终态且 started_at 早于本进程 Runtime 就绪时刻（留 1 秒
/// 容差）→ worker 必已随上一进程死亡，上游 is_active 不再可信。
/// 无启动时间戳（外部附着调试网关）时不收口，维持上游投影。
fn cron_run_is_corpse(
    boot_epoch: Option<f64>,
    started_at: Option<f64>,
    has_ended_at: bool,
) -> bool {
    !has_ended_at
        && boot_epoch.is_some_and(|boot| started_at.is_some_and(|started| started < boot - 1.0))
}

fn hermes_cron_run_status(
    is_active: bool,
    end_reason: Option<&str>,
    has_ended_at: bool,
) -> &'static str {
    if is_active {
        "running"
    } else if end_reason.is_some_and(|reason| {
        matches!(
            reason,
            "completed" | "success" | "ok" | "cron_complete" | "text_response"
        )
    }) {
        "completed"
    } else if has_ended_at {
        "error"
    } else {
        // Hermes 的 runs API 已将「5 分钟内仍有活动且未终态」投影为
        // is_active=true。走到这里表示会话既没有终态，也已不再活跃，通常是
        // Runtime 重启或执行进程退出；继续展示 pending 会让 UI 永久等待。
        "error"
    }
}

#[tauri::command]
pub async fn agent_hermes_cron_runs(
    id: String,
    profile: String,
) -> ApiResponse<Vec<HermesCronRunInfo>> {
    let (id, profile) = match validate_hermes_cron_identity(&id, &profile) {
        Ok(identity) => identity,
        Err(error) => return ApiResponse::err(error),
    };
    let path = format!(
        "api/cron/jobs/{}/runs?profile={}&limit=30",
        hermes_query_encode(&id),
        hermes_query_encode(&profile)
    );
    match hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(20),
    )
    .await
    {
        Ok(value) => {
            let Some(items) = value.get("runs").and_then(serde_json::Value::as_array) else {
                return ApiResponse::err("Hermes Cron 返回无效运行历史".into());
            };
            let runs = items
                .iter()
                .filter_map(|item| {
                    let session_id = hermes_cron_text(item, "id")?;
                    let upstream_active = item
                        .get("is_active")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    let mut end_reason = hermes_cron_text(item, "end_reason");
                    let has_ended_at = item.get("ended_at").is_some_and(|value| !value.is_null());
                    // 孤儿收口：started_at 早于本进程 Runtime 启动且无终态的会话，
                    // worker 必随上一进程死亡；上游 300 秒活跃窗会误报 running，
                    // 并把「立即运行」卡死最多 5 分钟。这里即时投影为中断。
                    let corpse = cron_run_is_corpse(
                        crate::agent::hermes::bundled_runtime::bundled_gateway_boot_epoch(),
                        item.get("started_at").and_then(serde_json::Value::as_f64),
                        has_ended_at,
                    );
                    if corpse && end_reason.is_none() {
                        end_reason = Some(
                            "该运行随上一次 Runtime 进程重启而结束，未写入终态；SophoNote 已收口为中断。"
                                .into(),
                        );
                    }
                    let status = hermes_cron_run_status(
                        upstream_active && !corpse,
                        end_reason.as_deref(),
                        has_ended_at,
                    );
                    if status == "error" && end_reason.is_none() {
                        end_reason = Some(
                            "Hermes 本轮已停止活动但未写入终态，通常由 Runtime 重启或执行进程中断造成。"
                                .into(),
                        );
                    }
                    Some(HermesCronRunInfo {
                        session_id,
                        status: status.into(),
                        started_at: item.get("started_at").and_then(serde_json::Value::as_f64),
                        ended_at: item.get("ended_at").and_then(serde_json::Value::as_f64),
                        preview: hermes_cron_text(item, "preview").unwrap_or_default(),
                        end_reason,
                        profile: hermes_cron_text(item, "profile")
                            .unwrap_or_else(|| profile.clone()),
                        model: hermes_cron_text(item, "model"),
                        tool_call_count: item
                            .get("tool_call_count")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        model_call_count: item
                            .get("api_call_count")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        last_activity: hermes_cron_text(item, "last_activity_description"),
                    })
                })
                .collect();
            ApiResponse::ok(runs)
        }
        Err(error) => ApiResponse::err(error),
    }
}

fn hermes_cron_message_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.trim().to_string();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| {
            part.get("text")
                .and_then(serde_json::Value::as_str)
                .or_else(|| part.get("content").and_then(serde_json::Value::as_str))
        })
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn hermes_cron_source_label(value: &str) -> &str {
    match value {
        "github" => "GitHub",
        "arxiv" => "arXiv",
        "hackernews" => "Hacker News",
        "producthunt" => "Product Hunt",
        "huggingface" => "Hugging Face",
        "aihot" => "AIHOT",
        _ => value,
    }
}

fn hermes_cron_string_list(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(hermes_cron_source_label)
        .collect::<Vec<_>>()
        .join("、")
}

fn hermes_cron_tool_identity(name: &str) -> (&'static str, &'static str) {
    match name {
        "mcp__sophonote_bridge__refresh_discovery_sources" => ("抓取", "刷新发现数据源"),
        "mcp__sophonote_bridge__list_discovery_candidates" => ("预筛", "按元数据缩小候选范围"),
        "mcp__sophonote_bridge__read_discovery_item" => ("取证", "读取条目证据"),
        "mcp__sophonote_bridge__save_discovery_analysis" => ("生成", "保存发现解读"),
        "mcp__sophonote_bridge__save_discovery_pick" => ("发布", "发布到今日发现"),
        "mcp__sophonote_bridge__save_discovery_scores" => ("打分", "全量评分与标注落库"),
        "mcp__sophonote_bridge__read_discovery_feed" => ("读取", "读取已存发现数据"),
        "mcp__sophonote_bridge__save_discovery_report" => ("报告", "保存 AI 周期报告"),
        "mcp__sophonote_bridge__refresh_openrouter_rankings" => ("榜单", "刷新 OpenRouter 模型榜"),
        "mcp__sophonote_bridge__read_openrouter_rankings" => ("读取", "读取 OpenRouter 模型榜"),
        _ => ("工具", "执行工具"),
    }
}

fn is_discovery_business_tool(name: &str) -> bool {
    matches!(
        name,
        "mcp__sophonote_bridge__refresh_discovery_sources"
            | "mcp__sophonote_bridge__list_discovery_candidates"
            | "mcp__sophonote_bridge__read_discovery_item"
            | "mcp__sophonote_bridge__save_discovery_analysis"
            | "mcp__sophonote_bridge__save_discovery_pick"
            | "mcp__sophonote_bridge__save_discovery_scores"
            | "mcp__sophonote_bridge__read_discovery_feed"
            | "mcp__sophonote_bridge__save_discovery_report"
            | "mcp__sophonote_bridge__refresh_openrouter_rankings"
            | "mcp__sophonote_bridge__read_openrouter_rankings"
    )
}

fn canonical_hermes_tool_name(name: &str) -> String {
    if name.starts_with("mcp__sophonote_bridge__") {
        return name.to_string();
    }
    for prefix in ["sophonote-bridge__", "sophonote-bridge_", "mcp_sophonote-bridge_"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return format!("mcp__sophonote_bridge__{rest}");
        }
    }
    name.to_string()
}

/// Hermes 会按 Provider 能力保存两种 tool_calls：
///
/// - 旧版延迟工具：function.name=tool_call，真实名称位于 arguments 信封；
/// - 原生函数调用：function.name 就是真实工具名，arguments 是业务参数。
///
/// 运行链路必须兼容二者，否则实际执行成功也会显示为 0 个业务步骤。
fn hermes_cron_function_call(function: &serde_json::Value) -> Option<(String, serde_json::Value)> {
    let function_name = hermes_cron_text(function, "name")?;
    let raw_arguments = function.get("arguments").cloned().unwrap_or_default();
    let parsed_arguments = match raw_arguments {
        serde_json::Value::String(raw) => serde_json::from_str(&raw).ok()?,
        serde_json::Value::Object(_) => raw_arguments,
        _ => serde_json::json!({}),
    };
    if function_name == "tool_call" {
        let tool_name = parsed_arguments.get("name")?.as_str()?;
        let arguments = parsed_arguments
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        Some((canonical_hermes_tool_name(tool_name), arguments))
    } else {
        Some((canonical_hermes_tool_name(&function_name), parsed_arguments))
    }
}

fn hermes_cron_tool_input(name: &str, arguments: &serde_json::Value) -> String {
    let sources = hermes_cron_string_list(arguments.get("sources"));
    match name {
        "mcp__sophonote_bridge__refresh_discovery_sources" => {
            format!(
                "数据源：{}",
                if sources.is_empty() {
                    "按任务配置"
                } else {
                    &sources
                }
            )
        }
        "mcp__sophonote_bridge__list_discovery_candidates" => {
            let limit = arguments
                .get("limitPerSource")
                .or_else(|| arguments.get("limit_per_source"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(4);
            format!(
                "数据源：{}；生成前每个来源最多保留 {limit} 条候选",
                if sources.is_empty() {
                    "按任务配置"
                } else {
                    &sources
                }
            )
        }
        "mcp__sophonote_bridge__read_discovery_item" => "读取候选条目的元数据与证据材料".into(),
        "mcp__sophonote_bridge__save_discovery_analysis" => {
            let mode = arguments
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if mode == "deep" {
                "生成并保存深度解读".into()
            } else {
                "生成并保存卡片速览".into()
            }
        }
        "mcp__sophonote_bridge__save_discovery_pick" => {
            let lane = arguments
                .get("lane")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("未分类");
            let score = arguments
                .get("score")
                .and_then(serde_json::Value::as_f64)
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "未提供".into());
            format!("发现分区：{lane}；质量评分：{score}")
        }
        "mcp__sophonote_bridge__save_discovery_scores" => {
            let count = arguments
                .get("scores")
                .and_then(serde_json::Value::as_array)
                .map(|scores| scores.len())
                .unwrap_or(0);
            format!("批量持久化本轮评分与 aspect/主题标注：{count} 条")
        }
        "mcp__sophonote_bridge__read_discovery_feed" => "读取已存发现评分与分析，不重复抓取。".into(),
        "mcp__sophonote_bridge__save_discovery_report" => "保存 AI 日报、周报或月报。".into(),
        "mcp__sophonote_bridge__refresh_openrouter_rankings" => {
            "从 OpenRouter 官方 API 原子刷新完整模型榜快照。".into()
        }
        "mcp__sophonote_bridge__read_openrouter_rankings" => {
            "读取最近一次成功的 OpenRouter 模型榜快照。".into()
        }
        _ => {
            let text = arguments.to_string();
            if text.chars().count() > 240 {
                format!("{}…", text.chars().take(240).collect::<String>())
            } else {
                text
            }
        }
    }
}

fn hermes_cron_tool_payload(content: &str) -> Option<serde_json::Value> {
    serde_json::from_str(content).ok().or_else(|| {
        let start = content.find('{')?;
        let end = content.rfind('}')?;
        serde_json::from_str(&content[start..=end]).ok()
    })
}

fn hermes_cron_tool_error(value: &serde_json::Value) -> Option<String> {
    for key in ["error", "message", "detail"] {
        if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(240).collect());
            }
        }
    }
    None
}

fn hermes_cron_tool_output(name: &str, content: &str) -> (String, String) {
    let payload = hermes_cron_tool_payload(content);
    let compact = content.split_whitespace().collect::<String>();
    let failed = payload
        .as_ref()
        .and_then(|value| value.get("success"))
        .and_then(serde_json::Value::as_bool)
        == Some(false)
        || compact.contains("\"success\":false")
        || compact.contains("\"isError\":true");
    let error = payload.as_ref().and_then(hermes_cron_tool_error);
    let summary = match name {
        "mcp__sophonote_bridge__refresh_discovery_sources" => {
            "已完成数据源刷新，抓取数量与异常见最终结果。"
        }
        "mcp__sophonote_bridge__list_discovery_candidates" => {
            "已按元数据完成预筛，未入选条目不会读取正文或生成内容。"
        }
        "mcp__sophonote_bridge__read_discovery_item" => "已读取条目元数据与证据材料。",
        "mcp__sophonote_bridge__save_discovery_analysis" => "已保存发现卡片或深度解读。",
        "mcp__sophonote_bridge__save_discovery_pick" => "已将合格条目发布到今日发现。",
        "mcp__sophonote_bridge__save_discovery_scores" => "已持久化本轮评分与 aspect/主题标注。",
        "mcp__sophonote_bridge__read_discovery_feed" => "已读取周期内的发现数据。",
        "mcp__sophonote_bridge__save_discovery_report" => "已保存 AI 周期报告。",
        "mcp__sophonote_bridge__refresh_openrouter_rankings" => "已刷新 OpenRouter 模型榜。",
        "mcp__sophonote_bridge__read_openrouter_rankings" => "已读取 OpenRouter 模型榜。",
        _ => "工具调用已返回。",
    };
    if failed {
        (
            "error".into(),
            format!(
                "执行未成功：{}",
                error.unwrap_or_else(|| "请查看最终结果中的错误说明".into())
            ),
        )
    } else {
        ("completed".into(), summary.into())
    }
}

fn hermes_cron_run_steps(messages: &[serde_json::Value]) -> Vec<HermesCronRunStep> {
    let tool_results = messages
        .iter()
        .filter_map(|message| {
            (hermes_cron_text(message, "role").as_deref() == Some("tool"))
                .then(|| {
                    let id = hermes_cron_text(message, "tool_call_id")?;
                    let content = hermes_cron_message_text(message.get("content")?);
                    Some((id, content))
                })
                .flatten()
        })
        .collect::<HashMap<_, _>>();

    let mut steps = Vec::new();
    for message in messages {
        if hermes_cron_text(message, "role").as_deref() != Some("assistant") {
            continue;
        }
        let Some(calls) = message
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for call in calls {
            let Some(function) = call.get("function") else {
                continue;
            };
            let Some((tool_name, arguments)) = hermes_cron_function_call(function) else {
                continue;
            };
            // 执行链路只展示 SophoNote 业务步骤。terminal、read_file 等运行时
            // 工具既不是发现协议的一部分，也不应伪装成业务进度。
            if !is_discovery_business_tool(&tool_name) {
                continue;
            }
            let call_id = hermes_cron_text(call, "id")
                .or_else(|| hermes_cron_text(call, "call_id"))
                .unwrap_or_default();
            let (phase, default_title) = hermes_cron_tool_identity(&tool_name);
            let title = if tool_name == "mcp__sophonote_bridge__save_discovery_analysis" {
                if arguments.get("mode").and_then(serde_json::Value::as_str) == Some("deep") {
                    "保存深度解读"
                } else {
                    "保存卡片速览"
                }
            } else {
                default_title
            };
            let (status, output) = tool_results
                .get(&call_id)
                .map(|content| hermes_cron_tool_output(&tool_name, content))
                .unwrap_or_else(|| ("running".into(), "等待 Hermes 返回工具结果。".into()));
            steps.push(HermesCronRunStep {
                index: steps.len() + 1,
                phase: phase.into(),
                title: title.into(),
                tool_name: tool_name.trim_start_matches("mcp__sophonote_bridge__").into(),
                status,
                input: hermes_cron_tool_input(&tool_name, &arguments),
                output,
            });
        }
    }
    steps
}

#[tauri::command]
pub async fn agent_hermes_cron_run_result(
    session_id: String,
    profile: String,
) -> ApiResponse<HermesCronRunResult> {
    let (session_id, profile) = match validate_hermes_cron_identity(&session_id, &profile) {
        Ok(identity) => identity,
        Err(error) => return ApiResponse::err(error),
    };
    let path = format!(
        "api/sessions/{}/messages?profile={}&limit=100&order=latest",
        hermes_query_encode(&session_id),
        hermes_query_encode(&profile)
    );
    match hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(20),
    )
    .await
    {
        Ok(value) => {
            let messages = value
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let markdown = messages
                .iter()
                .rev()
                .find_map(|message| {
                    let role = hermes_cron_text(message, "role")?;
                    if role != "assistant" {
                        return None;
                    }
                    let text = hermes_cron_message_text(message.get("content")?);
                    (!text.is_empty()).then_some(text)
                })
                .unwrap_or_else(|| "本次运行尚未生成结果。".into());
            let steps = hermes_cron_run_steps(&messages);
            ApiResponse::ok(HermesCronRunResult {
                session_id,
                markdown,
                steps,
            })
        }
        Err(error) => ApiResponse::err(error),
    }
}

/// 从 Hermes Runtime 的 `commands.catalog` 获取原生 Skill。SophoNote 不再解析、
/// 启停或导出 Skill 正文，只把 Runtime 当前可用项展示给用户。
#[tauri::command]
pub async fn agent_hermes_skills() -> ApiResponse<Vec<HermesSkillInfo>> {
    let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
        return ApiResponse::err("Hermes Agent 未连接".into());
    };
    let mut gateway =
        match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
        {
            Ok(gateway) => gateway,
            Err(error) => return ApiResponse::err(error.to_string()),
        };
    let catalog = match gateway
        .call("commands.catalog", serde_json::json!({}))
        .await
    {
        Ok(catalog) => catalog,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    ApiResponse::ok(parse_hermes_skills(&catalog))
}

fn parse_hermes_skills(catalog: &serde_json::Value) -> Vec<HermesSkillInfo> {
    let skill_meta = catalog.get("skills").and_then(serde_json::Value::as_object);
    let mut skills = catalog
        .get("pairs")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let pair = pair.as_array()?;
            let command = pair.first()?.as_str()?;
            let meta = skill_meta?.get(command)?;
            Some(HermesSkillInfo {
                name: command.trim_start_matches('/').to_string(),
                description: pair
                    .get(1)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                origin: meta
                    .get("origin")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                category: String::new(),
                enabled: true,
                usage: 0,
                provenance: String::new(),
            })
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

fn parse_hermes_commands(catalog: &serde_json::Value) -> Vec<HermesCommandInfo> {
    catalog
        .get("categories")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|category| {
            let category_name = category
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Commands")
                .to_string();
            category
                .get("pairs")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |pair| {
                    let pair = pair.as_array()?;
                    Some(HermesCommandInfo {
                        name: pair.first()?.as_str()?.to_string(),
                        description: pair
                            .get(1)
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        category: category_name.clone(),
                    })
                })
        })
        .collect()
}

fn parse_hermes_references(value: &serde_json::Value) -> Vec<HermesReferenceInfo> {
    value
        .get("items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let text = item.get("text")?.as_str()?.to_string();
            Some(HermesReferenceInfo {
                display: item
                    .get("display")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&text)
                    .to_string(),
                description: item
                    .get("meta")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                text,
            })
        })
        .collect()
}

/// 手动重启 Hermes Runtime。前端在检测到"未连接"后调用此命令触发重连。
/// 健康监督器会在重启完成后自动推送 `sophonote:hermes-status-changed` 事件。
#[tauri::command]
pub async fn restart_hermes_runtime(app: AppHandle) -> ApiResponse<()> {
    match crate::restart_bundled_hermes(&app).await {
        Ok(()) => {
            // 手动重启成功即清零自动重启预算，让监督器重新获得自动兜底能力；
            // 否则预算耗尽后一次手动失败就永远只剩人工路径。
            if let Ok(guard) = app.state::<crate::AppState>().hermes_health.lock() {
                if let Some(supervisor) = guard.as_ref() {
                    supervisor.reset_restart_budget();
                }
            }
            // 重启成功后推送 connected 状态，前端无需轮询
            let _ = app.emit("sophonote:hermes-status-changed", "connected");
            ApiResponse::ok(())
        }
        Err(error) => ApiResponse::err(error),
    }
}

/// 当前 Thread 绑定的 Hermes Session 占用与本轮 YOLO。无 Session 时明确失败。
#[tauri::command]
pub async fn agent_hermes_session_surface(
    app: AppHandle,
    thread_id: String,
) -> ApiResponse<crate::agent::hermes::session_surface::HermesSessionSurface> {
    let db_path = crate::db::get_db_path(&app);
    match crate::agent::hermes::session_surface::load_session_surface(&db_path, &thread_id).await {
        Ok(surface) => ApiResponse::ok(surface),
        Err(error) => ApiResponse::err(error),
    }
}

/// 本轮 YOLO：只改当前 Hermes Session 的审批旁路，不写全局 config.yaml。
#[tauri::command]
pub async fn agent_hermes_session_set_yolo(
    app: AppHandle,
    thread_id: String,
    enabled: bool,
) -> ApiResponse<bool> {
    let db_path = crate::db::get_db_path(&app);
    match crate::agent::hermes::session_surface::set_session_yolo(&db_path, &thread_id, enabled)
        .await
    {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

/// 不建 Run 的 slash 控制面。`/undo` 走 Hermes 原生命令并裁掉对应 UI 审计 Run。
#[tauri::command]
pub async fn agent_hermes_session_slash(
    app: AppHandle,
    thread_id: String,
    command: String,
) -> ApiResponse<crate::agent::hermes::session_surface::HermesSlashSurfaceResult> {
    let db_path = crate::db::get_db_path(&app);
    match crate::agent::hermes::session_surface::exec_session_slash(&db_path, &thread_id, &command)
        .await
    {
        Ok(result) => ApiResponse::ok(result),
        Err(error) => ApiResponse::err(error),
    }
}

/// Hermes Runtime 能力快照。Skill、Toolset、Tool、MCP 与 Browser 状态均从
/// 同一个 Gateway 连接读取；SophoNote 不再把自己的本地 MCP 管理器冒充为 Agent 能力。
#[tauri::command]
pub async fn agent_hermes_capabilities() -> ApiResponse<HermesCapabilities> {
    let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
        return ApiResponse::err("Hermes Agent 未连接".into());
    };
    let mut gateway =
        match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
        {
            Ok(gateway) => gateway,
            Err(error) => return ApiResponse::err(error.to_string()),
        };

    let catalog = match gateway
        .call("commands.catalog", serde_json::json!({}))
        .await
    {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let toolsets_value = match gateway.call("tools.list", serde_json::json!({})).await {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let tools_value = match gateway.call("tools.show", serde_json::json!({})).await {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let browser = match gateway
        .call("browser.manage", serde_json::json!({"action": "status"}))
        .await
    {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let config = match gateway
        .call("config.get", serde_json::json!({"key": "full"}))
        .await
    {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    // `@` 根入口由 Hermes Runtime 返回；SophoNote 只负责将显式选择转成原生附件。
    let references = gateway
        .call("complete.path", serde_json::json!({"word": "@"}))
        .await
        .map(|value| parse_hermes_references(&value))
        .unwrap_or_default();

    // Desktop 的能力页使用同一 Hermes 实例的 Dashboard 管理 API 补充启停、
    // 使用量和健康探测。失败时保留 Gateway 目录，不让管理面瞬时失败拖垮 Chat。
    // Desktop 的各个页签独立查询。SophoNote 的弹层需要一次快照，因此并发读取，
    // 避免 Skills、Terminal、Hub 和 analytics 的超时被串行累加。
    let (dashboard_skills_result, terminal_backends_result, hub_sources_result, analytics_result) = tokio::join!(
        hermes_dashboard_request(
            reqwest::Method::GET,
            "api/skills",
            None,
            std::time::Duration::from_secs(15),
        ),
        hermes_dashboard_request(
            reqwest::Method::GET,
            "api/tools/terminal/backends",
            None,
            std::time::Duration::from_secs(15),
        ),
        hermes_dashboard_request(
            reqwest::Method::GET,
            "api/skills/hub/sources",
            None,
            std::time::Duration::from_secs(20),
        ),
        hermes_dashboard_request(
            reqwest::Method::GET,
            "api/analytics/usage?days=365",
            None,
            std::time::Duration::from_secs(20),
        ),
    );
    let dashboard_skills = dashboard_skills_result
        .ok()
        .and_then(|value| serde_json::from_value::<Vec<HermesSkillInfo>>(value).ok());
    let terminal_backends = terminal_backends_result
        .ok()
        .and_then(|value| serde_json::from_value::<HermesTerminalBackends>(value).ok())
        .unwrap_or_default();
    let hub_sources = hub_sources_result
        .ok()
        .and_then(|value| serde_json::from_value::<HermesHubSources>(value).ok())
        .unwrap_or_default();
    let usage_by_tool = analytics_result
        .ok()
        .and_then(|value| value.get("tools").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            Some((
                entry.get("tool")?.as_str()?.to_string(),
                entry.get("count")?.as_u64()? as usize,
            ))
        })
        .collect::<HashMap<_, _>>();

    let mut toolsets = serde_json::from_value::<Vec<HermesToolsetInfo>>(
        toolsets_value.get("toolsets").cloned().unwrap_or_default(),
    )
    .unwrap_or_default();
    for toolset in &mut toolsets {
        toolset.usage = toolset
            .tools
            .iter()
            .map(|tool| usage_by_tool.get(tool).copied().unwrap_or_default())
            .sum();
    }
    let mut tools = Vec::new();
    let mut tools_by_section: HashMap<String, Vec<HermesToolInfo>> = HashMap::new();
    for section in tools_value
        .get("sections")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = section
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let section_tools = serde_json::from_value::<Vec<HermesToolInfo>>(
            section.get("tools").cloned().unwrap_or_default(),
        )
        .unwrap_or_default();
        tools.extend(section_tools.iter().cloned());
        tools_by_section.insert(name.to_string(), section_tools);
    }
    let mcp_config = config
        .get("config")
        .and_then(|value| value.get("mcp_servers"))
        .and_then(serde_json::Value::as_object);
    let mut mcp_servers = mcp_config
        .into_iter()
        .flatten()
        .map(|(name, value)| {
            let section_name = format!("mcp-{}", name.replace('_', "-"));
            HermesMcpServerInfo {
                name: name.clone(),
                transport: if value
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
                {
                    "http".to_string()
                } else {
                    "stdio".to_string()
                },
                enabled: value
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                url: value
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                command: value
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                args: value
                    .get("args")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect(),
                auth: value
                    .get("auth")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("headers")
                            .and_then(serde_json::Value::as_object)
                            .is_some_and(|headers| {
                                headers
                                    .keys()
                                    .any(|key| key.eq_ignore_ascii_case("authorization"))
                            })
                            .then(|| "header".to_string())
                    }),
                tools: tools_by_section.remove(&section_name).unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();
    mcp_servers.sort_by(|left, right| left.name.cmp(&right.name));
    tools.sort_by(|left, right| left.name.cmp(&right.name));

    ApiResponse::ok(HermesCapabilities {
        commands: parse_hermes_commands(&catalog),
        skills: dashboard_skills.unwrap_or_else(|| parse_hermes_skills(&catalog)),
        references,
        toolsets,
        tools,
        mcp_servers,
        terminal_backends,
        hub_sources,
        browser_connected: browser
            .get("connected")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        browser_url: browser
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

/// 使用 Hermes 自己的 tools.configure；配置由 Runtime 持久化。
#[tauri::command]
pub async fn agent_hermes_toolset_set_enabled(name: String, enabled: bool) -> ApiResponse<()> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.contains(':') {
        return ApiResponse::err("Hermes Toolset 名称无效".into());
    }
    let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
        return ApiResponse::err("Hermes Agent 未连接".into());
    };
    let mut gateway =
        match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
        {
            Ok(gateway) => gateway,
            Err(error) => return ApiResponse::err(error.to_string()),
        };
    let action = if enabled { "enable" } else { "disable" };
    match gateway
        .call(
            "tools.configure",
            serde_json::json!({"action": action, "names": [name.clone()]}),
        )
        .await
    {
        Ok(value) => {
            let unknown = value
                .get("unknown")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            if unknown.iter().any(|item| *item == name) {
                ApiResponse::err(format!("Hermes Runtime 不支持配置 Toolset {name}"))
            } else {
                ApiResponse::ok(())
            }
        }
        Err(error) => ApiResponse::err(error.to_string()),
    }
}

/// 启停 Hermes Skill。目录与配置都属于 Hermes；SophoNote 只转发用户选择。
#[tauri::command]
pub async fn agent_hermes_skill_set_enabled(name: String, enabled: bool) -> ApiResponse<()> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return ApiResponse::err("Hermes Skill 名称无效".into());
    }
    match hermes_dashboard_request(
        reqwest::Method::PUT,
        "api/skills/toggle",
        Some(serde_json::json!({"name": name, "enabled": enabled})),
        std::time::Duration::from_secs(15),
    )
    .await
    {
        Ok(_) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(error),
    }
}

/// 读取 Hermes 学习/本地 Skill 的 SKILL.md。SophoNote 只承载编辑 surface，
/// 文件定位、权限、校验和持久化仍由 Hermes Runtime 负责。
#[tauri::command]
pub async fn agent_hermes_skill_document(name: String) -> ApiResponse<HermesSkillDocument> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return ApiResponse::err("Hermes Skill 名称无效".into());
    }
    let path = format!("api/learning/node?id={}", hermes_query_encode(&name));
    match hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(15),
    )
    .await
    {
        Ok(value) => ApiResponse::ok(HermesSkillDocument {
            name,
            content: value
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        Err(error) => ApiResponse::err(error),
    }
}

/// 保存 Hermes 学习/本地 Skill；完整正文直接送入 Hermes 的正式学习节点接口。
#[tauri::command]
pub async fn agent_hermes_skill_document_save(name: String, content: String) -> ApiResponse<()> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return ApiResponse::err("Hermes Skill 名称无效".into());
    }
    if content.is_empty() || content.len() > 512 * 1024 {
        return ApiResponse::err("SKILL.md 正文为空或超过 512 KiB".into());
    }
    match hermes_dashboard_request(
        reqwest::Method::PUT,
        "api/learning/node",
        Some(serde_json::json!({"id": name, "content": content})),
        std::time::Duration::from_secs(20),
    )
    .await
    {
        Ok(_) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(error),
    }
}

/// 归档 Hermes 学习/本地 Skill。Hermes 自身提供可恢复语义，SophoNote 不直接删文件。
#[tauri::command]
pub async fn agent_hermes_skill_archive(name: String) -> ApiResponse<()> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return ApiResponse::err("Hermes Skill 名称无效".into());
    }
    match hermes_dashboard_request(
        reqwest::Method::DELETE,
        "api/learning/node",
        Some(serde_json::json!({"id": name})),
        std::time::Duration::from_secs(20),
    )
    .await
    {
        Ok(_) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(error),
    }
}

/// Hermes Terminal Toolset 的执行后端选择与 Desktop 使用同一正式接口。
#[tauri::command]
pub async fn agent_hermes_terminal_backend_select(backend: String) -> ApiResponse<()> {
    let backend = backend.trim().to_ascii_lowercase();
    if !matches!(
        backend.as_str(),
        "local" | "docker" | "singularity" | "modal" | "daytona" | "ssh"
    ) {
        return ApiResponse::err("Hermes Terminal 执行后端无效".into());
    }
    match hermes_dashboard_request(
        reqwest::Method::PUT,
        "api/tools/terminal/backend",
        Some(serde_json::json!({"backend": backend})),
        std::time::Duration::from_secs(15),
    )
    .await
    {
        Ok(_) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(error),
    }
}

/// Hermes Skills Hub 浏览/搜索；安装仍由 Runtime 执行，客户端不复制 Skill 内容。
#[tauri::command]
pub async fn agent_hermes_skills_hub(
    query: Option<String>,
    page: Option<usize>,
) -> ApiResponse<HermesHubPage> {
    let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
        return ApiResponse::err("Hermes Agent 未连接".into());
    };
    let mut gateway =
        match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
        {
            Ok(gateway) => gateway,
            Err(error) => return ApiResponse::err(error.to_string()),
        };
    let query = query.unwrap_or_default().trim().to_string();
    let (action, params) = if query.is_empty() {
        (
            "browse",
            serde_json::json!({"action": "browse", "page": page.unwrap_or(1).max(1), "page_size": 24}),
        )
    } else {
        (
            "search",
            serde_json::json!({"action": "search", "query": query}),
        )
    };
    match gateway.call("skills.manage", params).await {
        Ok(value) if action == "browse" => match serde_json::from_value(value) {
            Ok(page) => ApiResponse::ok(page),
            Err(error) => ApiResponse::err(format!("Hermes Skills Hub 返回无效: {error}")),
        },
        Ok(value) => {
            let items = value.get("results").cloned().unwrap_or_default();
            let items =
                serde_json::from_value::<Vec<HermesHubSkillInfo>>(items).unwrap_or_default();
            let total = items.len();
            ApiResponse::ok(HermesHubPage {
                items,
                page: 1,
                total_pages: 1,
                total,
            })
        }
        Err(error) => ApiResponse::err(error.to_string()),
    }
}

/// 预览 Hub Skill 的真实 SKILL.md；不安装、不复制到 SophoNote。
#[tauri::command]
pub async fn agent_hermes_skill_hub_preview(identifier: String) -> ApiResponse<HermesHubPreview> {
    let identifier = identifier.trim().to_string();
    if identifier.is_empty() || identifier.len() > 512 || identifier.chars().any(char::is_control) {
        return ApiResponse::err("Hermes Skill 标识无效".into());
    }
    let path = format!(
        "api/skills/hub/preview?identifier={}",
        hermes_query_encode(&identifier)
    );
    match hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(45),
    )
    .await
    {
        Ok(value) => match serde_json::from_value(value) {
            Ok(preview) => ApiResponse::ok(preview),
            Err(error) => ApiResponse::err(format!("Hermes Skill 预览返回无效: {error}")),
        },
        Err(error) => ApiResponse::err(error),
    }
}

/// Nous 审核的 MCP Catalog；只返回清单与变量名，不返回任何密钥值。
#[tauri::command]
pub async fn agent_hermes_mcp_catalog() -> ApiResponse<HermesMcpCatalog> {
    match hermes_dashboard_request(
        reqwest::Method::GET,
        "api/mcp/catalog",
        None,
        std::time::Duration::from_secs(30),
    )
    .await
    {
        Ok(value) => match serde_json::from_value(value) {
            Ok(catalog) => ApiResponse::ok(catalog),
            Err(error) => ApiResponse::err(format!("Hermes MCP Catalog 返回无效: {error}")),
        },
        Err(error) => ApiResponse::err(error),
    }
}

/// 安装 Nous MCP Catalog 条目；环境变量值只在本次请求中送达 Hermes。
#[tauri::command]
pub async fn agent_hermes_mcp_catalog_install(
    request: HermesMcpCatalogInstallRequest,
) -> ApiResponse<()> {
    let name = match hermes_mcp_name(&request.name) {
        Ok(name) => name,
        Err(error) => return ApiResponse::err(error),
    };
    if request.env.keys().any(|key| {
        key.is_empty()
            || key.len() > 128
            || !key.chars().enumerate().all(|(index, ch)| {
                ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || ch.is_ascii_alphabetic())
            })
    }) {
        return ApiResponse::err("MCP Catalog 环境变量名称无效".into());
    }
    match hermes_dashboard_request(
        reqwest::Method::POST,
        "api/mcp/catalog/install",
        Some(serde_json::json!({"name": name, "env": request.env})),
        std::time::Duration::from_secs(60),
    )
    .await
    {
        Ok(value) if value.get("background").and_then(serde_json::Value::as_bool) == Some(true) => {
            // 需要 npm/pip 等安装步骤的 Catalog 项由 Hermes 后台 action 收口；此时
            // Runtime 尚未落盘完成，立即 reload 会制造一个虚假的失败。
            ApiResponse::ok(())
        }
        Ok(_) => match reload_hermes_mcp().await {
            Ok(()) => ApiResponse::ok(()),
            Err(error) => ApiResponse::err(format!("MCP 已安装，但 Hermes 重载失败: {error}")),
        },
        Err(error) => ApiResponse::err(error),
    }
}

/// 由 Hermes Runtime 安装 Hub Skill 并热重载目录。SophoNote 只提交用户选择的
/// identifier/name，不下载、解析或复制 Skill 内容。
#[tauri::command]
pub async fn agent_hermes_skill_install(identifier: String) -> ApiResponse<()> {
    let identifier = identifier.trim().to_string();
    if identifier.is_empty() || identifier.len() > 512 || identifier.chars().any(char::is_control) {
        return ApiResponse::err("Hermes Skill 标识无效".into());
    }
    let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
        return ApiResponse::err("Hermes Agent 未连接".into());
    };
    let mut gateway =
        match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
        {
            Ok(gateway) => gateway,
            Err(error) => return ApiResponse::err(error.to_string()),
        };
    let installed = match gateway
        .call(
            "skills.manage",
            serde_json::json!({"action": "install", "query": identifier}),
        )
        .await
    {
        Ok(value) => value
            .get("installed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    if !installed {
        return ApiResponse::err("Hermes Runtime 未确认 Skill 安装成功".into());
    }
    match gateway.call("skills.reload", serde_json::json!({})).await {
        Ok(_) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(format!("Skill 已安装，但 Hermes 热重载失败: {error}")),
    }
}

/// 透传 Hermes Browser 管理动作。真正的浏览器发现、启动与连接状态均由
/// Runtime 管理，SophoNote 不维护第二份 Browser 会话。
#[tauri::command]
pub async fn agent_hermes_browser_manage(action: String) -> ApiResponse<HermesCapabilities> {
    let action = action.trim().to_ascii_lowercase();
    if !matches!(action.as_str(), "connect" | "disconnect") {
        return ApiResponse::err("Hermes Browser 动作无效".into());
    }
    let Some(endpoint) = crate::agent::hermes::HermesGatewayEndpoint::from_env() else {
        return ApiResponse::err("Hermes Agent 未连接".into());
    };
    let mut gateway =
        match crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
        {
            Ok(gateway) => gateway,
            Err(error) => return ApiResponse::err(error.to_string()),
        };
    if let Err(error) = gateway
        .call("browser.manage", serde_json::json!({"action": action}))
        .await
    {
        return ApiResponse::err(error.to_string());
    }
    drop(gateway);
    agent_hermes_capabilities().await
}

fn hermes_mcp_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 96
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err("MCP Server 名称仅允许字母、数字、连字符和下划线".into());
    }
    Ok(name.to_string())
}

fn hermes_mcp_path(name: &str, suffix: &str) -> Result<String, String> {
    let name = hermes_mcp_name(name)?;
    Ok(format!("api/mcp/servers/{name}{suffix}"))
}

async fn hermes_dashboard_request(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, String> {
    let endpoint = crate::agent::hermes::HermesGatewayEndpoint::from_env()
        .ok_or_else(|| "Hermes Agent 未连接".to_string())?;
    let url = endpoint
        .dashboard_base_url()
        .map_err(|error| error.to_string())?
        .join(path.trim_start_matches('/'))
        .map_err(|error| format!("Hermes MCP 管理地址无效: {error}"))?;
    let client = reqwest::Client::builder()
        // Dashboard 地址来自已附着的 loopback Gateway，不能经过系统代理。
        .no_proxy()
        .timeout(timeout)
        .build()
        .map_err(|error| format!("创建 Hermes MCP 管理连接失败: {error}"))?;
    let mut request = client
        .request(method, url)
        .header("X-Hermes-Session-Token", &endpoint.token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Hermes MCP 管理请求失败: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取 Hermes MCP 管理响应失败: {error}"))?;
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .unwrap_or_else(|_| serde_json::json!({"detail": text}));
    if !status.is_success() {
        let detail = value
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Hermes MCP 管理失败");
        return Err(format!("Hermes MCP 管理失败 ({status}): {detail}"));
    }
    Ok(value)
}

async fn reload_hermes_mcp() -> Result<(), String> {
    let endpoint = crate::agent::hermes::HermesGatewayEndpoint::from_env()
        .ok_or_else(|| "Hermes Agent 未连接".to_string())?;
    let mut gateway =
        crate::agent::hermes::gateway_client::HermesGatewayConnection::connect(&endpoint)
            .await
            .map_err(|error| error.to_string())?;
    let value = gateway
        .call("reload.mcp", serde_json::json!({"confirm": true}))
        .await
        .map_err(|error| error.to_string())?;
    if value.get("status").and_then(serde_json::Value::as_str) != Some("reloaded") {
        return Err(format!("Hermes 未确认 MCP 重载完成: {value}"));
    }
    Ok(())
}

/// 在 Hermes Runtime 中新增 MCP Server。配置、环境变量和 Bearer 密钥均由
/// Hermes Dashboard 管理面落盘；SophoNote 不保存第二份配置，也不向前端回传密钥。
#[tauri::command]
pub async fn agent_hermes_mcp_add(request: HermesMcpServerCreate) -> ApiResponse<HermesMcpProbe> {
    let name = match hermes_mcp_name(&request.name) {
        Ok(name) => name,
        Err(error) => return ApiResponse::err(error),
    };
    let transport = request.transport.trim().to_ascii_lowercase();
    if !matches!(transport.as_str(), "http" | "stdio") {
        return ApiResponse::err("MCP 传输必须为 HTTP 或 stdio".into());
    }
    let auth = request.auth.trim().to_ascii_lowercase();
    if !matches!(auth.as_str(), "none" | "header" | "oauth") {
        return ApiResponse::err("MCP 认证方式无效".into());
    }
    if request.args.len() > 64
        || request.args.iter().any(|value| {
            value.is_empty() || value.len() > 4 * 1024 || value.chars().any(char::is_control)
        })
    {
        return ApiResponse::err("MCP 参数最多 64 项，且每项必须为不超过 4 KB 的单行文本".into());
    }
    if request.env.len() > 64
        || request.env.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 128
                || !key.chars().enumerate().all(|(index, ch)| {
                    ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
                })
                || value.len() > 64 * 1024
        })
    {
        return ApiResponse::err(
            "MCP 环境变量无效：最多 64 项，名称须为环境变量格式，单值不超过 64 KB".into(),
        );
    }
    if request.bearer_token.len() > 64 * 1024 {
        return ApiResponse::err("MCP Bearer Token 过长".into());
    }
    if transport == "http" {
        let url = match reqwest::Url::parse(request.url.trim()) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => url,
            _ => return ApiResponse::err("远程 MCP URL 必须是有效的 http/https 地址".into()),
        };
        if auth == "header" && request.bearer_token.trim().is_empty() {
            return ApiResponse::err("Bearer Token 不能为空".into());
        }
        if auth != "header" && !request.bearer_token.trim().is_empty() {
            return ApiResponse::err("只有 Bearer Token 认证可提交 bearerToken".into());
        }
        if !request.command.trim().is_empty() || !request.args.is_empty() || !request.env.is_empty()
        {
            return ApiResponse::err(
                "远程 HTTP MCP 不能同时提交 stdio 命令、参数或环境变量".into(),
            );
        }
        if url.as_str().len() > 8 * 1024 {
            return ApiResponse::err("远程 MCP URL 过长".into());
        }
    } else {
        if request.command.trim().is_empty()
            || request.command.len() > 4 * 1024
            || request.command.chars().any(char::is_control)
        {
            return ApiResponse::err("stdio MCP 必须提供不超过 4 KB 的单行命令".into());
        }
        if auth != "none" || !request.url.trim().is_empty() || !request.bearer_token.is_empty() {
            return ApiResponse::err("stdio MCP 不接受 HTTP URL 或远程认证配置".into());
        }
    }
    let mut body = serde_json::json!({
        "name": name,
        "args": request.args,
        "env": request.env,
        "auth": auth,
    });
    if transport == "http" {
        body["url"] = serde_json::Value::String(request.url.trim().to_string());
    } else {
        body["command"] = serde_json::Value::String(request.command.trim().to_string());
    }
    if !request.bearer_token.trim().is_empty() {
        body["bearer_token"] = serde_json::Value::String(request.bearer_token.trim().to_string());
    }
    if let Err(error) = hermes_dashboard_request(
        reqwest::Method::POST,
        "api/mcp/servers",
        Some(body),
        std::time::Duration::from_secs(30),
    )
    .await
    {
        return ApiResponse::err(error);
    }
    if let Err(error) = reload_hermes_mcp().await {
        return ApiResponse::err(format!("MCP 已保存，但 Runtime 重载失败: {error}"));
    }
    agent_hermes_mcp_test(name).await
}

/// 启用或停用 Hermes MCP Server，并重载 Runtime 工具目录。
#[tauri::command]
pub async fn agent_hermes_mcp_set_enabled(name: String, enabled: bool) -> ApiResponse<()> {
    let path = match hermes_mcp_path(&name, "/enabled") {
        Ok(path) => path,
        Err(error) => return ApiResponse::err(error),
    };
    if let Err(error) = hermes_dashboard_request(
        reqwest::Method::PUT,
        &path,
        Some(serde_json::json!({"enabled": enabled})),
        std::time::Duration::from_secs(30),
    )
    .await
    {
        return ApiResponse::err(error);
    }
    match reload_hermes_mcp().await {
        Ok(()) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(format!("MCP 状态已保存，但 Runtime 重载失败: {error}")),
    }
}

/// 真实连接一次 MCP Server、读取工具/Prompt/Resource 后断开。
#[tauri::command]
pub async fn agent_hermes_mcp_test(name: String) -> ApiResponse<HermesMcpProbe> {
    let path = match hermes_mcp_path(&name, "/test") {
        Ok(path) => path,
        Err(error) => return ApiResponse::err(error),
    };
    match hermes_dashboard_request(
        reqwest::Method::POST,
        &path,
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    {
        Ok(value) => match serde_json::from_value(value) {
            Ok(probe) => ApiResponse::ok(probe),
            Err(error) => ApiResponse::err(format!("Hermes MCP 探测响应无效: {error}")),
        },
        Err(error) => ApiResponse::err(error),
    }
}

/// 从 Hermes Runtime 移除 MCP Server，并刷新动态工具目录。
#[tauri::command]
pub async fn agent_hermes_mcp_remove(name: String) -> ApiResponse<()> {
    let path = match hermes_mcp_path(&name, "") {
        Ok(path) => path,
        Err(error) => return ApiResponse::err(error),
    };
    if let Err(error) = hermes_dashboard_request(
        reqwest::Method::DELETE,
        &path,
        None,
        std::time::Duration::from_secs(30),
    )
    .await
    {
        return ApiResponse::err(error);
    }
    match reload_hermes_mcp().await {
        Ok(()) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(format!("MCP 已移除，但 Runtime 重载失败: {error}")),
    }
}

/// 发起/轮询 Hermes MCP OAuth。授权 URL 由前端交给系统浏览器打开，Token
/// 始终保存在 Hermes Home，不经过 SophoNote 数据库或 localStorage。
#[tauri::command]
pub async fn agent_hermes_mcp_oauth_start(name: String) -> ApiResponse<HermesMcpOAuthFlow> {
    let path = match hermes_mcp_path(&name, "/auth") {
        Ok(path) => path,
        Err(error) => return ApiResponse::err(error),
    };
    match hermes_dashboard_request(
        reqwest::Method::POST,
        &path,
        None,
        std::time::Duration::from_secs(60),
    )
    .await
    {
        Ok(value) => match serde_json::from_value(value) {
            Ok(flow) => ApiResponse::ok(flow),
            Err(error) => ApiResponse::err(format!("Hermes MCP OAuth 响应无效: {error}")),
        },
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_hermes_mcp_oauth_status(flow_id: String) -> ApiResponse<HermesMcpOAuthFlow> {
    let flow_id = flow_id.trim();
    if flow_id.is_empty()
        || flow_id.len() > 256
        || !flow_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return ApiResponse::err("MCP OAuth flow 标识无效".into());
    }
    let path = format!("api/mcp/oauth/flows/{flow_id}");
    match hermes_dashboard_request(
        reqwest::Method::GET,
        &path,
        None,
        std::time::Duration::from_secs(30),
    )
    .await
    {
        Ok(value) => match serde_json::from_value(value) {
            Ok(flow) => ApiResponse::ok(flow),
            Err(error) => ApiResponse::err(format!("Hermes MCP OAuth 状态无效: {error}")),
        },
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_hermes_mcp_reload() -> ApiResponse<()> {
    match reload_hermes_mcp().await {
        Ok(()) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(error),
    }
}

/// AG-14：启动一次 Agent 运行（创建 Thread+Run，事件同时写入 RunStore 和 Tauri Channel）。
/// 前端用返回的 run_id 订阅 on_event 通道，断线后用 agent_run_events_replay 补全。
/// DevTools 调用示例：
///   window.__TAURI__.core.invoke('agent_run_start', {
///     request: { message: '查杭州天气', threadId: null, projectId: null },
///     onEvent: (e) => console.log(e.seq, e.payload),
///   }).then(r => console.log(r))
#[tauri::command]
pub async fn agent_run_start(
    app: AppHandle,
    request: AgentRunStartArgs,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> ApiResponse<AgentRunStartResult> {
    let db_path = crate::db::get_db_path(&app);

    let workspace_permission_mode = request
        .workspace_permission_mode
        .as_deref()
        .unwrap_or("ask");
    if !matches!(workspace_permission_mode, "ask" | "autoEdit" | "plan") {
        return ApiResponse::err("工作区权限模式无效".into());
    }
    let hermes_workspace_root = match request
        .workspace_root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(root) => match std::fs::canonicalize(root) {
            Ok(path) if path.is_dir() => Some(path.to_string_lossy().to_string()),
            Ok(_) => return ApiResponse::err("工作区路径不是目录".into()),
            Err(error) => return ApiResponse::err(format!("工作区路径不可访问: {error}")),
        },
        None => None,
    };

    // Hermes Surface 原生附件预检：不读取正文、不拼 XML、不注入行为指令。
    if let Err(error) = crate::agent::attachments::validate_surface_attachments(
        &request.message,
        &request.attachments,
    ) {
        return ApiResponse::err(error);
    }
    let requested_hermes_model = request
        .hermes_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if requested_hermes_model
        .as_ref()
        .is_some_and(|model| model.len() > 256)
    {
        return ApiResponse::err("Hermes 模型标识过长".into());
    }
    let requested_hermes_provider = request
        .hermes_provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if requested_hermes_provider.is_some_and(|provider| provider.len() > 128) {
        return ApiResponse::err("Hermes 供应商标识过长".into());
    }
    // 保留用户原始意图。附件引用由 Hermes 原生 attach RPC 返回并在 Surface
    // 协议层关联，不用“请处理附件”等客户端补写文字替用户发言。
    let visible_user_message = request.message.trim().to_string();

    // 0. 只允许 Hermes Runtime 与 SophoNote 设置共同确认可用的模型。
    // Runtime 的可发现目录不能绕过设置中的供应商、凭据与模型白名单。
    let model_options = match load_configured_hermes_model_options(&app).await {
        Ok(options) => options,
        Err(error) => return ApiResponse::err(error),
    };
    let (selected_hermes_provider, selected_hermes_model) = match resolve_hermes_model(
        &model_options,
        requested_hermes_provider,
        requested_hermes_model.as_deref(),
    ) {
        Ok(selection) => selection,
        Err(error) => return ApiResponse::err(error),
    };
    let gateway: SharedGateway = Arc::new(HermesOwnedModelGateway);
    // DEC-020：产品 Chat 不再拥有 PromptRegistry；记录 Surface 协议版本用于审计。
    let prompt_version = "hermes-surface@v1".to_string();

    // 1. 创建或复用 Thread（AG-19：复用时取回 Thread 记录，供归属校验与历史加载）
    let (thread_id, existing_thread) = match request.thread_id {
        Some(tid) => {
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
            };
            let store = crate::agent::store::RunStore::new(conn);
            match store.get_thread(&tid) {
                Ok(Some(t)) => (tid, Some(t)),
                Ok(None) => return ApiResponse::err(format!("Thread {} 不存在或已删除", tid)),
                Err(e) => return ApiResponse::err(format!("查询 Thread 失败: {}", e)),
            }
        }
        None => {
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
            };
            let store = crate::agent::store::RunStore::new(conn);
            let tid = format!("thread-{}", uuid::Uuid::new_v4());
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if let Err(e) =
                store.create_thread(&tid, "新会话", request.project_id.as_deref(), now_ms)
            {
                return ApiResponse::err(format!("创建 Thread 失败: {}", e));
            }
            (tid, None)
        }
    };

    // 1.5 AG-19 归属校验：复用 Thread 时请求的 project 必须与归属一致；
    // Run 的 project_id 一律继承 Thread（见 resolve_run_scope 注释）
    let effective_project =
        match resolve_run_scope(existing_thread.as_ref(), request.project_id.as_deref()) {
            Ok(p) => p,
            Err(e) => return ApiResponse::err(e),
        };

    let hermes_session_id = match hermes_session_for_thread(&db_path, &thread_id) {
        Ok(session_id) => session_id,
        Err(error) => return ApiResponse::err(error),
    };

    // Skill 是 Hermes 原生命令引用，正文、工具交集和 Memory 均由 Runtime 维护。
    let run_skill = request
        .skill
        .as_deref()
        .map(|name| crate::agent::events::RunSkillRef {
            name: name.trim_start_matches('/').to_string(),
            version: 0,
            source: "hermes".into(),
        });

    // DEC-019：产品 Agent 固定 Hermes；缺配置明确失败，不存在 Rig 回退。
    let hermes_ready = probe_hermes_production_health();
    let engine_resolve = resolve_engine(hermes_ready);
    let engine_will_fail = is_engine_unavailable(&engine_resolve);
    let (selected_engine_id, selected_engine_version) = match &engine_resolve {
        EngineResolve::Use(EngineChoice::Hermes) | EngineResolve::Unavailable { .. } => (
            crate::agent::hermes::ENGINE_ID,
            crate::agent::hermes::STUB_PROTOCOL_VERSION,
        ),
    };

    // 2. 创建 Run
    let run_id = format!("run-{}", uuid::Uuid::new_v4());
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let store = crate::agent::store::RunStore::new(conn);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Gateway 自己拥有 Agent 循环预算；该字段只保留本地 Run 元数据兼容。
    let max_turns = request.max_turns.unwrap_or(6).clamp(1, 20);

    // provider/model 直接记录 Hermes Runtime 的真实选择。
    if let Err(e) = store.create_run(
        &run_id,
        &thread_id,
        effective_project.as_deref(),
        &selected_hermes_provider,
        &selected_hermes_model,
        Some(prompt_version.as_str()),
        max_turns,
        now_ms,
    ) {
        return ApiResponse::err(format!("创建 Run 失败: {}", e));
    }
    // H8：覆盖默认 rig 引擎字段为本次选型结果
    if let Err(e) = store.set_run_engine(&run_id, selected_engine_id, selected_engine_version) {
        eprintln!("[agent] 写入 run.engine 失败（不阻断）: {e}");
    }
    if let Some(session_id) = hermes_session_id.as_deref() {
        if let Err(e) = store.update_run_external_meta(
            &run_id,
            Some("jsonrpc+websocket"),
            None,
            Some(session_id),
            Some(crate::agent::hermes::STUB_PROTOCOL_VERSION),
            None,
            now_ms,
        ) {
            eprintln!("[agent] 写入 Hermes Session 对账信息失败（不阻断）: {e}");
        }
    }

    // 2.5 回写 Thread.latest_run_id（AG-15 契约收口）：窗口重挂载后前端凭
    // Thread.latestRunId 定位事件重放入口，不回写则恢复链路断裂。
    // 非致命：失败仅记录，不阻断运行启动。AG-22：updated_at 用真实时间戳。
    if let Err(e) = store.set_latest_run_id(&thread_id, &run_id, now_ms) {
        eprintln!("[agent] 更新 thread.latest_run_id 失败（不阻断运行）: {e}");
    }

    // 2.55 AG-22（审计 P1-2 整改③）：复用 Thread 时显式恢复 running 状态——
    // 上一轮终态后 Thread 停在 completed/cancelled/failed，新 Run 启动若不回
    // running，状态与现实背离（「统一真实时间戳与状态迁移」）。新建 Thread
    // 本就 running，此调用对其幂等。非致命。
    if let Err(e) = store.update_thread_status(
        &thread_id,
        &crate::agent::types::ThreadStatus::Running,
        now_ms,
    ) {
        eprintln!("[agent] 恢复 thread.running 状态失败（不阻断运行）: {e}");
    }

    // 历史只由 Hermes Session 恢复；SophoNote 本地消息表是展示/审计副本，不回灌模型。

    // 2.7 持久化用户消息（AG-18 b）：agent_messages 是 agent_thread_messages 命令
    // 与未来「对话历史」菜单的数据源。先保存再 move 进 SpikeParams；
    // 失败非致命——事件流（seq=0 run_started）仍是渲染主路径。
    let user_msg_id = format!("msg-{}", uuid::Uuid::new_v4());
    if let Err(e) = store.save_message(
        &user_msg_id,
        &thread_id,
        &run_id,
        "user",
        &visible_user_message,
        now_ms,
    ) {
        eprintln!("[agent] 持久化用户消息失败（不阻断运行）: {e}");
    }
    // 标题等助手回复落地后再生成（refresh 仅在 completed + final_answer）

    // 3. 创建传输层（AG-20 RunStore-first，审计 P0-3 整改项①②）：
    //    主路 RunStore 写成功 = 事件已提交，随后才推送 Channel；
    //    DB 写失败 → 事件不广播（杜绝仅屏幕可见事件）；
    //    Channel 失败 → 实时流缺口，前端 seq 缺口检测 + replay 补齐（DB 已有）
    let channel_transport = Arc::new(ChannelTransport { channel: on_event });
    let store_transport = Arc::new(crate::agent::store::RunStoreTransport::new(
        db_path.to_string_lossy().to_string(),
    ));
    let durable = Arc::new(crate::agent::store::DurableFirstTransport::new(
        store_transport as Arc<dyn EventTransport>,
        vec![channel_transport as Arc<dyn EventTransport>],
    ));

    // 4. 创建 EventEmitter
    let emitter = Arc::new(EventEmitter::new(&thread_id, &run_id, durable));

    // 5. 启动 RunController（后台异步任务）
    // AG-22：网关已在步骤 0 由同一 ProviderSnapshot 构造（配置缺失早失败）

    // Context 先进入 run_started 的可见审计 chip；显式选区或当前文档稍后由
    // Hermes Adapter 经原生 file.attach 上传。选区优先，禁止静默扩大到全文。
    let focus_context = request.focus_document.as_ref().and_then(|f| {
        if f.article_id.trim().is_empty() {
            None
        } else {
            Some(crate::agent::events::RunContext {
                article_id: f.article_id.clone(),
                title: f.title.clone(),
                base_version: f.base_version,
                selected_markdown: String::new(),
                selected_text_hash: String::new(),
                before_context: String::new(),
                after_context: String::new(),
            })
        }
    });
    let run_context = request
        .selection
        .as_ref()
        .map(|s| s.to_run_context())
        .or(focus_context);
    let params = SpikeParams {
        system: None,
        history: Vec::new(),
        user: visible_user_message,
        max_turns,
        temperature: Some(0.0),
        // AG-22（整改②）：run_id/prompt_version 注入每次模型请求
        run_id: Some(run_id.clone()),
        prompt_version: prompt_version.clone(),
        // AG-26：选区随 run_started 审计，并由 Hermes Adapter 作为原生附件提交。
        run_context,
        // AG-27：激活 Skill 随 run_started 事件透传（预算夹紧见 max_turns 步骤）
        run_skill,
        // AG-27：工具调用预算（manifest max_tool_calls；None = 不限制）
        max_tool_calls: None,
    };

    let cancel = CancellationToken::new();
    // AG-18 c)：登记全局取消令牌，agent_run_cancel 凭 run_id 查找触发；
    // 终态后在 spawn 闭包尾部注销防泄漏
    global_cancel_registry().register(&run_id, cancel.clone());
    // Hermes 拥有工具状态机。产品 Run 传空注册表，避免把天气/计算器等历史
    // Spike 工具误认为 Surface 能力；该字段仅为旧 RunEnvelope 兼容存在。
    let registry = Arc::new(ToolRegistry::new());

    let app_clone = app.clone();
    let run_id_clone = run_id.clone();
    let thread_id_clone = thread_id.clone();
    // AG-22：spawn 闭包内 queued→running 状态迁移需要的 db 路径
    let db_path_clone = db_path.clone();
    let session_binding = crate::agent::engine::HermesSessionBinding {
        db_path: db_path.clone(),
        notes_dir: crate::notes::notes_dir(&app),
        project_id: effective_project.clone(),
        thread_id: thread_id.clone(),
        run_id: run_id.clone(),
    };
    let hermes_attachments = request.attachments.clone();
    let hermes_focus_document = if request.selection.is_none() {
        request.focus_document.as_ref().and_then(|document| {
            let article_id = document.article_id.trim();
            let markdown = document.markdown.as_deref()?;
            if article_id.is_empty() {
                return None;
            }
            Some(HermesFocusDocument {
                article_id: article_id.to_string(),
                title: document.title.clone(),
                base_version: document.base_version,
                markdown: markdown.to_string(),
            })
        })
    } else {
        None
    };
    let hermes_command = request.hermes_command.clone();
    let hermes_project_context = request.include_project_context
        && request.selection.is_none()
        && hermes_focus_document.is_none()
        && session_binding.project_id.is_some();

    tauri::async_runtime::spawn(async move {
        // AG-22（审计 P1-2 整改③）：queued→running 状态迁移持久化（真实时间戳；
        // 与终态对账同口径，Run 状态轨迹完整可审计）。非致命：失败不阻断运行。
        if let Ok(conn) = rusqlite::Connection::open(&db_path_clone) {
            let store = crate::agent::store::RunStore::new(conn);
            let start_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            if let Err(e) = store.update_run_status(
                &run_id_clone,
                &crate::agent::types::RunStatus::Running,
                start_ms,
            ) {
                eprintln!("[agent] run 状态置 running 失败（不阻断运行）: {e}");
            }
        }

        // Hermes 未配置 → 可见失败；无其他引擎回退。
        if engine_will_fail {
            let reason = match &engine_resolve {
                EngineResolve::Unavailable { reason } => reason.clone(),
                _ => "Hermes 不可用".into(),
            };
            let _ = emitter.emit(AgentEventPayload::EngineDegraded {
                reason: reason.clone(),
                reconnecting: false,
            });
            let _ = emitter.emit(AgentEventPayload::RunFailed {
                outcome: "failed".into(),
                error: reason.clone(),
            });
            let db_path = crate::db::get_db_path(&app_clone);
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                let store = crate::agent::store::RunStore::new(conn);
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let _ = store.update_run_status(
                    &run_id_clone,
                    &crate::agent::types::RunStatus::Failed,
                    now_ms,
                );
                let _ = store.update_thread_status(
                    &thread_id_clone,
                    &crate::agent::types::ThreadStatus::Failed,
                    now_ms,
                );
            }
            global_cancel_registry().remove(&run_id_clone);
            return;
        }

        let envelope = RunEnvelope {
            gateway,
            registry,
            params,
            cancel,
            events: Some(emitter),
            observer: None,
            context_pack: None,
            model_route: None,
            hermes_session_id,
            hermes_memory_scope_key: None,
            hermes_input: None,
            hermes_model: Some(selected_hermes_model),
            hermes_provider: Some(selected_hermes_provider),
            hermes_command,
            hermes_workspace_root,
            hermes_attachments,
            hermes_focus_document,
            hermes_project_context,
            hermes_session_binding: Some(session_binding),
        };

        eprintln!(
            "[agent] run {} engine=hermes resolve={:?} runtime_ready={} surface=gateway",
            run_id_clone, engine_resolve, hermes_ready,
        );

        let events_for_fail = envelope.events.clone();
        let report_result = match &engine_resolve {
            EngineResolve::Use(EngineChoice::Hermes) => {
                match crate::agent::hermes::AttachedHermesEngine::try_from_env() {
                    Ok(eng) => eng.run_with_events(envelope).await,
                    // 终态统一由下方 Err 分支推 RunFailed，避免双发
                    Err(e) => Err(e),
                }
            }
            EngineResolve::Unavailable { .. } => unreachable!("handled above"),
        };

        match report_result {
            Ok(report) => {
                println!(
                    "[agent] run {} finished: outcome={} model_calls={} tools={}",
                    run_id_clone,
                    report.outcome,
                    report.model_calls,
                    report.tool_executions.len()
                );
                // 更新 Run/Thread 状态 + 持久化最终回答（AG-18 b）
                let db_path = crate::db::get_db_path(&app_clone);
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    let store = crate::agent::store::RunStore::new(conn);
                    // Run 与 Thread 终态同源映射（AG-15 契约收口：Thread 状态
                    // 不再永远停在 running，前端 thread.status 才可消费）
                    let (run_status, thread_status) = match report.outcome.as_str() {
                        "completed" => (
                            crate::agent::types::RunStatus::Completed,
                            crate::agent::types::ThreadStatus::Completed,
                        ),
                        "cancelled" => (
                            crate::agent::types::RunStatus::Cancelled,
                            crate::agent::types::ThreadStatus::Cancelled,
                        ),
                        _ => (
                            crate::agent::types::RunStatus::Failed,
                            crate::agent::types::ThreadStatus::Failed,
                        ),
                    };
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let _ = store.update_run_status(&run_id_clone, &run_status, now_ms);
                    let _ = store.update_thread_status(&thread_id_clone, &thread_status, now_ms);
                    // AG-22：终态对账模型调用数（rig turn() 口径，含重试）——
                    // current_model_calls 不再恒 0，Run 记录与真实请求一致
                    let _ = store.set_model_calls(&run_id_clone, report.model_calls, now_ms);
                    // AG-18 b)：最终回答落 agent_messages（assistant 消息）。仅 completed
                    // 且非空才落——cancelled/failed 无最终回答，其前端可见性由
                    // run_failed/run_cancelled 事件归约承担（AG-15 契约收口）。
                    if report.outcome == "completed" && !report.final_answer.is_empty() {
                        let msg_id = format!("msg-{}", uuid::Uuid::new_v4());
                        if let Err(e) = store.save_message(
                            &msg_id,
                            &thread_id_clone,
                            &run_id_clone,
                            "assistant",
                            &report.final_answer,
                            now_ms,
                        ) {
                            eprintln!("[agent] 持久化 final_answer 失败（不阻断运行）: {e}");
                        } else {
                            let _ =
                                store.refresh_thread_title_from_messages(&thread_id_clone, now_ms);
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("[agent] run {} failed: {}", run_id_clone, err);
                // 引擎在 RunStarted 之前失败时也必须推终态，否则前端一直「正在处理」
                if let Some(em) = &events_for_fail {
                    let msg = err.to_string();
                    let user_facing = if msg.contains("502")
                        || msg.contains("Connection")
                        || msg.contains("connect")
                        || msg.contains("非预期状态")
                    {
                        format!(
                            "Hermes 引擎暂时不可用（{msg}）。请确认本机已执行 `hermes serve --host 127.0.0.1 --port 9119 --skip-build` 后重试。"
                        )
                    } else {
                        msg
                    };
                    let _ = em.emit(AgentEventPayload::RunFailed {
                        outcome: "failed".into(),
                        error: user_facing,
                    });
                }
                let db_path = crate::db::get_db_path(&app_clone);
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    let store = crate::agent::store::RunStore::new(conn);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let _ = store.update_run_status(
                        &run_id_clone,
                        &crate::agent::types::RunStatus::Failed,
                        now_ms,
                    );
                    let _ = store.update_thread_status(
                        &thread_id_clone,
                        &crate::agent::types::ThreadStatus::Failed,
                        now_ms,
                    );
                }
            }
        }
        // AG-18 c)：终态注销令牌防注册表泄漏（completed/cancelled/failed 均经此收口）
        global_cancel_registry().remove(&run_id_clone);
    });

    // 6. 立即返回 thread_id 和 run_id（前端用 run_id 订阅事件流）
    ApiResponse::ok(AgentRunStartResult { thread_id, run_id })
}

// ---------------- AG-17：窗口重挂载恢复（Thread 全量事件史）----------------

/// AG-17：获取 Thread 的完整事件历史（全部 Run 按创建时间升序 × 每 Run 内事件
/// 按 seq 升序，**含 seq=0**）。用于窗口重挂载/项目切换后恢复消息列表。
///
/// 为什么不复用 agent_run_events_replay：replay_after_seq 是排他语义
///（seq > after_seq），seq=0 的 run_started（= 用户消息）永远取不回；
/// 且恢复需要跨 Run 串联，单 Run 接口要前端 N 次往返。
///
/// 返回形态与 agent_run_events_replay 一致（JSON 字符串数组），
/// 前端复用同一 JSON.parse → handleEvent 链路（eventId 幂等去重）。
#[tauri::command]
pub async fn agent_thread_history(app: AppHandle, thread_id: String) -> ApiResponse<Vec<String>> {
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };

    let store = crate::agent::store::RunStore::new(conn);
    // list_runs_by_thread 按创建时间降序 → 逆转为升序（恢复顺序 = 创建顺序）
    let mut runs = match store.list_runs_by_thread(&thread_id) {
        Ok(r) => r,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    runs.reverse();

    let mut all: Vec<String> = Vec::new();
    for run in runs {
        match store.all_events_of_run(&run.id) {
            Ok(mut events) => all.append(&mut events),
            Err(e) => return ApiResponse::err(e.to_string()),
        }
    }
    ApiResponse::ok(all)
}

// ---------------- AG-18：运行取消（全局 CancellationToken 注册表）----------------

/// Run 取消令牌注册表：run_id → CancellationToken。
/// agent_run_start 启动时 register、终态 remove；agent_run_cancel 凭 run_id 触发。
/// 取消链路：token.cancel() → RunController 循环顶/Gateway Cancelled 双收口（AG-06）
/// → run.cancelled 事件（AG-07）→ Run/Thread 终态映射 cancelled（AG-15）
/// → 前端 run_cancelled 归约为可见消息。
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<HashMap<String, CancellationToken>>,
}

impl CancelRegistry {
    pub fn register(&self, run_id: &str, token: CancellationToken) {
        self.inner.lock().unwrap().insert(run_id.to_string(), token);
    }

    /// 触发取消。true = 信号已发出；false = 未注册（Run 已终态或未知）。
    pub fn cancel(&self, run_id: &str) -> bool {
        let token = self.inner.lock().unwrap().get(run_id).cloned();
        match token {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    pub fn remove(&self, run_id: &str) {
        self.inner.lock().unwrap().remove(run_id);
    }

    pub fn contains(&self, run_id: &str) -> bool {
        self.inner.lock().unwrap().contains_key(run_id)
    }
}

/// 进程级单例注册表。OnceLock::get_or_init（1.70 稳定）；
/// 不用 get_or_try_init 等更新 API（宿主 rustc 版本约束）。
fn global_cancel_registry() -> &'static CancelRegistry {
    static REGISTRY: OnceLock<CancelRegistry> = OnceLock::new();
    REGISTRY.get_or_init(CancelRegistry::default)
}

/// 取消一个 Run（AG-18 c）。data=true = 取消信号已发出；
/// data=false = 该 run 不在注册表（已终态/未知）——终态后到达的取消按 no-op 处理不报错。
#[tauri::command]
pub async fn agent_run_cancel(run_id: String) -> ApiResponse<bool> {
    let dispatched = global_cancel_registry().cancel(&run_id);
    ApiResponse::ok(dispatched)
}

/// 将 Hermes 原生审批选择回传到当前 Run 所绑定的 Gateway Session。
#[tauri::command]
pub async fn agent_run_approval_respond(
    run_id: String,
    choice: String,
    all: Option<bool>,
) -> ApiResponse<bool> {
    let choice = choice.trim().to_ascii_lowercase();
    if !matches!(choice.as_str(), "once" | "session" | "always" | "deny") {
        return ApiResponse::err("无效审批选择".into());
    }
    ApiResponse::ok(crate::agent::hermes::gateway_client::send_run_control(
        &run_id,
        crate::agent::hermes::gateway_client::GatewayControl::Approval {
            choice,
            all: all.unwrap_or(false),
        },
    ))
}

/// 将 Hermes `clarify.request` 的用户回答回传；SophoNote 不把澄清改写成新一轮提示词。
#[tauri::command]
pub async fn agent_run_clarify_respond(
    run_id: String,
    request_id: String,
    answer: String,
) -> ApiResponse<bool> {
    if request_id.trim().is_empty() {
        return ApiResponse::err("requestId 不能为空".into());
    }
    if answer.len() > 32 * 1024 {
        return ApiResponse::err("回答过长".into());
    }
    ApiResponse::ok(crate::agent::hermes::gateway_client::send_run_control(
        &run_id,
        crate::agent::hermes::gateway_client::GatewayControl::Clarify { request_id, answer },
    ))
}

#[cfg(test)]
mod cancel_registry_tests {
    use super::*;

    #[test]
    fn register_then_cancel_dispatches_and_marks_token() {
        let reg = CancelRegistry::default();
        let token = CancellationToken::new();
        reg.register("r-cancel-1", token.clone());
        assert!(reg.contains("r-cancel-1"));
        assert!(reg.cancel("r-cancel-1"));
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_unknown_or_removed_run_is_noop() {
        let reg = CancelRegistry::default();
        assert!(!reg.cancel("r-cancel-missing"));
        let token = CancellationToken::new();
        reg.register("r-cancel-2", token.clone());
        reg.remove("r-cancel-2");
        assert!(!reg.contains("r-cancel-2"));
        assert!(!reg.cancel("r-cancel-2"));
        assert!(!token.is_cancelled());
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::agent::types::{AgentThread, ThreadStatus};

    fn thread(id: &str, project: Option<&str>) -> AgentThread {
        AgentThread {
            id: id.into(),
            title: "t".into(),
            status: ThreadStatus::Completed,
            project_id: project.map(str::to_string),
            latest_run_id: None,
            created_at: 0,
            updated_at: 0,
            closed_at: None,
            archived_at: None,
            pinned_at: None,
            collection_id: None,
        }
    }

    #[test]
    fn new_thread_takes_requested_project() {
        assert_eq!(
            resolve_run_scope(None, Some("p1")).unwrap(),
            Some("p1".into())
        );
        assert_eq!(resolve_run_scope(None, None).unwrap(), None);
    }

    #[test]
    fn reused_thread_inherits_own_project_when_request_silent() {
        let t = thread("thread-1", Some("p1"));
        assert_eq!(
            resolve_run_scope(Some(&t), None).unwrap(),
            Some("p1".into()),
            "Run 的 project 一律继承 Thread"
        );
    }

    #[test]
    fn reused_thread_matching_project_ok() {
        let t = thread("thread-1", Some("p1"));
        assert_eq!(
            resolve_run_scope(Some(&t), Some("p1")).unwrap(),
            Some("p1".into())
        );
    }

    #[test]
    fn reused_thread_cross_project_rejected() {
        let t = thread("thread-1", Some("p1"));
        let err = resolve_run_scope(Some(&t), Some("p2")).unwrap_err();
        assert!(err.contains("不属于项目"));
    }

    #[test]
    fn projectless_thread_rejects_any_project_request() {
        let t = thread("thread-1", None);
        assert!(resolve_run_scope(Some(&t), Some("p1")).is_err());
        assert_eq!(resolve_run_scope(Some(&t), None).unwrap(), None);
    }

    #[test]
    fn hermes_model_selection_uses_runtime_catalog() {
        let options = HermesModelOptions {
            model: Some("deepseek-v4-flash".into()),
            provider: Some("deepseek".into()),
            providers: vec![
                HermesModelProvider {
                    slug: "deepseek".into(),
                    name: "DeepSeek".into(),
                    models: vec!["deepseek-v4-flash".into(), "deepseek-v4-pro".into()],
                    authenticated: Some(true),
                    is_current: Some(true),
                },
                HermesModelProvider {
                    slug: "kimi".into(),
                    name: "Kimi".into(),
                    models: vec!["k3".into()],
                    authenticated: Some(true),
                    is_current: Some(false),
                },
            ],
        };
        assert_eq!(
            resolve_hermes_model(&options, None, None).unwrap(),
            ("deepseek".into(), "deepseek-v4-flash".into())
        );
        assert_eq!(
            resolve_hermes_model(&options, Some("kimi"), Some("k3")).unwrap(),
            ("kimi".into(), "k3".into())
        );
        assert!(resolve_hermes_model(&options, Some("deepseek"), Some("k3")).is_err());
    }

    #[test]
    fn hermes_default_prefers_a_confirmed_authenticated_provider() {
        let options = HermesModelOptions {
            model: Some("default".into()),
            provider: Some("moa".into()),
            providers: vec![
                HermesModelProvider {
                    slug: "moa".into(),
                    name: "Mixture of Agents".into(),
                    models: vec!["default".into()],
                    authenticated: None,
                    is_current: Some(false),
                },
                HermesModelProvider {
                    slug: "deepseek".into(),
                    name: "DeepSeek".into(),
                    models: vec!["deepseek-v4-pro".into()],
                    authenticated: Some(true),
                    is_current: Some(false),
                },
            ],
        };
        assert_eq!(
            resolve_hermes_model(&options, None, None).unwrap(),
            ("deepseek".into(), "deepseek-v4-pro".into())
        );
        assert!(resolve_hermes_model(&options, Some("moa"), Some("default")).is_err());
    }

    #[test]
    fn cron_model_gate_requires_an_explicit_configured_pair() {
        let options = HermesModelOptions {
            model: Some("deepseek-v4-flash".into()),
            provider: Some("deepseek".into()),
            providers: vec![HermesModelProvider {
                slug: "deepseek".into(),
                name: "DeepSeek".into(),
                models: vec!["deepseek-v4-flash".into()],
                authenticated: Some(true),
                is_current: Some(true),
            }],
        };
        assert!(!hermes_cron_model_is_configured(
            &options,
            &serde_json::json!({})
        ));
        assert!(!hermes_cron_model_is_configured(
            &options,
            &serde_json::json!({"provider": "deepseek"})
        ));
        assert!(!hermes_cron_model_is_configured(
            &options,
            &serde_json::json!({"provider": "moa", "model": "default"})
        ));
        assert!(hermes_cron_model_is_configured(
            &options,
            &serde_json::json!({"provider": "deepseek", "model": "deepseek-v4-flash"})
        ));
    }

    #[test]
    fn hermes_picker_is_limited_to_models_configured_in_sophonote() {
        let options = HermesModelOptions {
            model: Some("anthropic/claude-opus-5".into()),
            provider: Some("openrouter".into()),
            providers: vec![
                HermesModelProvider {
                    slug: "openrouter".into(),
                    name: "OpenRouter".into(),
                    models: vec!["anthropic/claude-opus-5".into()],
                    authenticated: Some(true),
                    is_current: Some(true),
                },
                HermesModelProvider {
                    slug: "deepseek".into(),
                    name: "DeepSeek".into(),
                    models: vec![
                        "deepseek-v4-pro".into(),
                        "deepseek-v4-flash".into(),
                        "deepseek-v4-unknown".into(),
                    ],
                    authenticated: Some(true),
                    is_current: Some(false),
                },
            ],
        };
        let allowlist = HashMap::from([(
            "deepseek".into(),
            vec!["deepseek-v4-pro".into(), "deepseek-v4-flash".into()],
        )]);

        let filtered = limit_hermes_model_options(options, &allowlist);

        assert_eq!(filtered.provider.as_deref(), Some("deepseek"));
        assert_eq!(filtered.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(filtered.providers.len(), 1);
        assert_eq!(
            filtered.providers[0].models,
            vec!["deepseek-v4-pro", "deepseek-v4-flash"]
        );
        assert_eq!(filtered.providers[0].authenticated, Some(true));
    }

    #[test]
    fn hermes_provider_matching_accepts_independent_family_instances() {
        assert!(settings_provider_matches_runtime("ollama", "ollama"));
        assert!(settings_provider_matches_runtime("ollama-2", "ollama"));
        assert!(settings_provider_matches_runtime("kimi-3", "moonshot"));
        assert!(settings_provider_matches_runtime("alibaba-2", "dashscope"));
        assert!(!settings_provider_matches_runtime("ollama-cloud", "ollama"));
    }

    #[test]
    fn configured_models_override_stale_runtime_catalog_and_mark_keyless_ready() {
        let options = HermesModelOptions {
            model: None,
            provider: None,
            providers: vec![HermesModelProvider {
                slug: "ollama".into(),
                name: "Ollama".into(),
                models: vec!["runtime-old".into()],
                authenticated: Some(false),
                is_current: Some(false),
            }],
        };
        let allowlist =
            HashMap::from([("ollama".into(), vec!["qwen3:8b".into(), "qwen3:32b".into()])]);
        let filtered = limit_hermes_model_options(options, &allowlist);
        assert_eq!(filtered.providers[0].models, vec!["qwen3:8b", "qwen3:32b"]);
        assert_eq!(filtered.providers[0].authenticated, Some(true));
    }
}

#[cfg(test)]
mod hermes_surface_catalog_tests {
    use super::*;

    #[test]
    fn parses_only_runtime_skill_entries_from_command_catalog() {
        let catalog = serde_json::json!({
            "pairs": [
                ["/new", "new session"],
                ["/sophonote", "Use SophoNote documents"]
            ],
            "categories": [
                {"name": "Session", "pairs": [["/new", "new session"]]}
            ],
            "skills": {
                "/sophonote": {"origin": "/tmp/skills/sophonote/SKILL.md"}
            }
        });
        let skills = parse_hermes_skills(&catalog);
        let commands = parse_hermes_commands(&catalog);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "sophonote");
        assert_eq!(skills[0].description, "Use SophoNote documents");
        assert_eq!(commands[0].name, "/new");
        assert_eq!(commands[0].category, "Session");
    }

    #[test]
    fn parses_runtime_reference_starters_without_scanning_paths() {
        let references = parse_hermes_references(&serde_json::json!({
            "items": [
                {"text": "@file:", "display": "@file:", "meta": "attach file"},
                {"text": "@url:", "display": "@url:", "meta": "fetch url"}
            ]
        }));
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].text, "@file:");
        assert_eq!(references[0].description, "attach file");
    }

    #[test]
    fn deserializes_gateway_snake_case_counts_for_surface_contract() {
        let toolset: HermesToolsetInfo = serde_json::from_value(serde_json::json!({
            "name": "browser",
            "description": "Browser tools",
            "tool_count": 14,
            "enabled": true,
            "tools": ["browser_navigate"]
        }))
        .unwrap();
        let hub: HermesHubPage = serde_json::from_value(serde_json::json!({
            "items": [], "page": 2, "total_pages": 7, "total": 140
        }))
        .unwrap();
        assert_eq!(toolset.tool_count, 14);
        assert_eq!(hub.total_pages, 7);
    }

    #[test]
    fn validates_mcp_server_names_before_building_rest_paths() {
        assert_eq!(hermes_mcp_name("github-mcp_1").unwrap(), "github-mcp_1");
        assert!(hermes_mcp_name("../oauth").is_err());
        assert!(hermes_mcp_name("含中文").is_err());
        assert!(hermes_mcp_path("server", "/test")
            .unwrap()
            .ends_with("/server/test"));
    }

    #[test]
    fn encodes_hub_identifiers_without_adding_a_url_dependency() {
        assert_eq!(
            hermes_query_encode("github:NousResearch/a skill"),
            "github%3ANousResearch%2Fa%20skill"
        );
        assert_eq!(hermes_query_encode("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn capability_contract_deserializes_dashboard_health_and_redacted_catalog() {
        let backends: HermesTerminalBackends = serde_json::from_value(serde_json::json!({
            "active": "local",
            "backends": [{
                "name": "docker", "label": "Docker", "description": "container",
                "active": false, "status": "needs_setup", "detail": "start Docker"
            }]
        }))
        .unwrap();
        let catalog: HermesMcpCatalog = serde_json::from_value(serde_json::json!({
            "entries": [{
                "name": "github", "description": "GitHub", "transport": "http",
                "auth_type": "env", "required_env": [{"name": "GITHUB_TOKEN", "required": true}]
            }],
            "diagnostics": []
        }))
        .unwrap();
        assert_eq!(backends.backends[0].status, "needs_setup");
        assert_eq!(catalog.entries[0].required_env[0].name, "GITHUB_TOKEN");
    }

    #[test]
    fn projects_hermes_cron_state_without_copying_runtime_storage() {
        let workdirs = HashMap::from([(
            "/tmp/sophonote-projects/project-1".to_string(),
            ("project-1".to_string(), "AI 学习".to_string()),
        )]);
        let running = project_hermes_cron_job(
            &serde_json::json!({
                "id": "job-1",
                "name": "每日论文",
                "prompt": "refresh",
                "schedule_display": "Every day at 09:00",
                "enabled": true,
                "state": "scheduled",
                "next_run_at": "2026-08-16T09:00:00+08:00",
                "last_status": "ok",
                "skills": ["sophonote-ai-radar"],
                "latest_execution": {"status": "running"},
                "schedule": {"kind": "cron", "expr": "0 9 * * *"},
                "workdir": "/tmp/sophonote-projects/project-1",
                "provider": "deepseek",
                "model": "deepseek-v4-pro",
                "profile": "default"
            }),
            &workdirs,
        )
        .unwrap();
        assert_eq!(running.status, "running");
        assert_eq!(running.skills, vec!["sophonote-ai-radar"]);
        assert_eq!(running.schedule_kind, "cron");
        assert_eq!(running.project_id.as_deref(), Some("project-1"));
        assert_eq!(running.project_name.as_deref(), Some("AI 学习"));
        assert_eq!(running.provider.as_deref(), Some("deepseek"));
        assert_eq!(running.model.as_deref(), Some("deepseek-v4-pro"));

        let failed = project_hermes_cron_job(
            &serde_json::json!({
                "id": "job-2",
                "enabled": true,
                "state": "scheduled",
                "next_run_at": "2026-08-16T10:00:00+08:00",
                "last_status": "error",
                "last_error": "provider unavailable"
            }),
            &workdirs,
        )
        .unwrap();
        assert_eq!(failed.status, "error");
        assert_eq!(failed.last_error.as_deref(), Some("provider unavailable"));

        let paused = project_hermes_cron_job(
            &serde_json::json!({
                "id": "job-3", "enabled": false, "state": "paused",
                "latest_execution": {"status": "running"}
            }),
            &workdirs,
        )
        .unwrap();
        assert_eq!(paused.status, "paused");
    }

    #[test]
    fn validates_hermes_cron_management_inputs() {
        let valid = HermesCronDraft {
            name: "每日论文".into(),
            prompt: "拉取并整理当天论文".into(),
            schedule: "0 9 * * *".into(),
            project_id: None,
            skills: vec!["sophonote-ai-radar".into()],
            provider: None,
            model: None,
            start_paused: false,
        };
        assert!(validate_hermes_cron_draft(&valid).is_ok());
        assert!(validate_hermes_cron_identity("35a9ed1c9f40", "default").is_ok());
        assert!(validate_hermes_cron_identity("../../jobs", "default").is_err());

        let mut invalid = valid;
        invalid.prompt.clear();
        assert!(validate_hermes_cron_draft(&invalid).is_err());
    }

    #[test]
    fn cron_draft_start_paused_is_explicit_and_backward_compatible() {
        let legacy: HermesCronDraft = serde_json::from_value(serde_json::json!({
            "name": "普通任务",
            "prompt": "执行任务",
            "schedule": "0 9 * * *"
        }))
        .unwrap();
        assert!(!legacy.start_paused);

        let example: HermesCronDraft = serde_json::from_value(serde_json::json!({
            "name": "公开范例",
            "prompt": "执行范例",
            "schedule": "0 9 * * *",
            "startPaused": true
        }))
        .unwrap();
        assert!(example.start_paused);
    }

    #[test]
    fn migrates_parameterized_daily_prompt_to_natural_chinese() {
        let prompt = "action=daily sources=[\"github\",\"arxiv\"] lanes=[\"github\",\"model\"] countPerLane=2 language=zh-CN";
        let skills = vec!["sophonote-ai-radar".to_string()];
        let migrated =
            migrate_cron_prompt_to_natural_language(prompt, &skills).expect("parameterized prompt");
        assert!(migrated.contains("请使用「sophonote-ai-radar」Skill"));
        assert!(migrated.contains("GitHub、arXiv、AI 热榜"));
        assert!(migrated.contains("每个信源最多预筛 2 条"));
        assert!(migrated.contains("使用中文生成结果"));
        assert!(!migrated.contains("action="));
        assert!(migrate_cron_prompt_to_natural_language(&migrated, &skills).is_none());
    }

    #[test]
    fn migrates_mindbox_cron_brand_without_losing_human_prompt() {
        let prompt = "请使用「mindbox-ai-radar」Skill 完成每日高质量发现。";
        let skills = vec!["mindbox-ai-radar".to_string()];
        let migrated =
            migrate_cron_prompt_to_natural_language(prompt, &skills).expect("brand migration");
        assert_eq!(
            migrated,
            "请使用「sophonote-ai-radar」Skill 完成每日高质量发现。"
        );
    }

    #[test]
    fn preserves_daily_choices_when_naturalizing_prompt() {
        let prompt = "action=daily sources=[github,arxiv,hackernews] lanes=[model] prefilterLimitPerSource=4 language=zh-CN notification=qualified-only";
        let skills = vec!["sophonote-ai-radar".to_string()];
        let migrated =
            migrate_cron_prompt_to_natural_language(prompt, &skills).expect("bare sources");
        assert!(migrated.contains("GitHub、arXiv、Hacker News、AI 热榜"));
        assert!(migrated.contains("按 模型与研究 视角"));
        assert!(migrated.contains("最多预筛 4 条"));
        assert!(migrated.contains("仅在发现合格内容时通知"));
    }

    #[test]
    fn naturalizes_all_bundled_report_and_ranking_prompts() {
        let radar = vec!["sophonote-ai-radar".to_string()];
        for (period, label) in [
            ("daily", "AI 日报"),
            ("weekly", "AI 周报"),
            ("monthly", "AI 月报"),
        ] {
            let prompt = format!("action=report period={period} language=zh-CN");
            let migrated =
                migrate_cron_prompt_to_natural_language(&prompt, &radar).expect("report prompt");
            assert!(migrated.contains(label));
            assert!(!migrated.contains("action="));
        }

        let rankings = vec!["sophonote-openrouter-rankings".to_string()];
        let migrated = migrate_cron_prompt_to_natural_language(
            "action=refresh firstAction=mcp__sophonote_bridge__refresh_openrouter_rankings notification=changes-only",
            &rankings,
        )
        .expect("rankings prompt");
        assert_eq!(migrated, OPENROUTER_RANKINGS_PROMPT);
        assert!(!migrated.contains("action="));
    }

    #[test]
    fn maps_native_cron_terminal_reasons_without_false_failures() {
        assert_eq!(hermes_cron_run_status(true, None, false), "running");
        assert_eq!(
            hermes_cron_run_status(false, Some("cron_complete"), true),
            "completed"
        );
        assert_eq!(
            hermes_cron_run_status(false, Some("KeyboardInterrupt"), true),
            "error"
        );
        assert_eq!(hermes_cron_run_status(false, None, false), "error");
    }

    #[test]
    fn projects_native_tool_messages_into_chinese_business_steps() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "call-1",
                    "function": {
                        "name": "tool_call",
                        "arguments": "{\"name\":\"mcp__sophonote_bridge__refresh_discovery_sources\",\"arguments\":{\"sources\":[\"github\",\"arxiv\"]}}"
                    }
                }]
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-1",
                "content": "{\"success\":true,\"newItemIds\":[\"item-1\"]}"
            }),
        ];
        let steps = hermes_cron_run_steps(&messages);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].phase, "抓取");
        assert_eq!(steps[0].title, "刷新发现数据源");
        assert_eq!(steps[0].input, "数据源：GitHub、arXiv");
        assert_eq!(steps[0].status, "completed");
        assert!(steps[0].output.contains("已完成"));
    }

    #[test]
    fn projects_direct_mcp_calls_and_ignores_runtime_tools() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "tool_calls": [
                    {
                        "id": "call-runtime",
                        "function": { "name": "terminal", "arguments": "{\"command\":\"pwd\"}" }
                    },
                    {
                        "id": "call-business",
                        "function": {
                            "name": "mcp__sophonote_bridge__list_discovery_candidates",
                            "arguments": "{\"sources\":[\"github\"],\"limitPerSource\":4}"
                        }
                    }
                ]
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-business",
                "content": "{\"success\":true}"
            }),
        ];
        let steps = hermes_cron_run_steps(&messages);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].phase, "预筛");
        assert_eq!(steps[0].tool_name, "list_discovery_candidates");
        assert_eq!(steps[0].status, "completed");
    }
}

#[cfg(test)]
mod cron_corpse_projection_tests {
    use super::*;

    #[test]
    fn run_started_before_boot_without_terminal_state_is_corpse() {
        assert!(cron_run_is_corpse(Some(1000.0), Some(900.0), false));
    }

    #[test]
    fn run_started_after_boot_is_not_corpse() {
        assert!(!cron_run_is_corpse(Some(1000.0), Some(1005.0), false));
    }

    #[test]
    fn terminated_run_is_never_corpse() {
        assert!(!cron_run_is_corpse(Some(1000.0), Some(900.0), true));
    }

    #[test]
    fn without_boot_stamp_upstream_projection_wins() {
        assert!(!cron_run_is_corpse(None, Some(900.0), false));
    }

    #[test]
    fn one_second_tolerance_for_clock_edge() {
        assert!(!cron_run_is_corpse(Some(1000.0), Some(999.5), false));
    }
}

#[cfg(test)]
mod hermes_usage_tests {
    use super::*;

    #[test]
    fn usage_report_accepts_runtime_snake_case_and_null_totals() {
        let mut value = serde_json::json!({
            "daily": [{
                "day": "2026-08-19",
                "input_tokens": 120,
                "output_tokens": 30,
                "cache_read_tokens": null,
                "reasoning_tokens": 8,
                "estimated_cost": 0.01,
                "actual_cost": null,
                "sessions": 2,
                "api_calls": 4
            }],
            "by_model": [{
                "model": "deepseek-v4-pro",
                "input_tokens": 120,
                "output_tokens": 30,
                "estimated_cost": 0.01,
                "sessions": 2,
                "api_calls": 4
            }],
            "totals": {
                "total_input": 120,
                "total_output": 30,
                "total_cache_read": null,
                "total_reasoning": 8,
                "total_estimated_cost": 0.01,
                "total_actual_cost": null,
                "total_sessions": 2,
                "total_api_calls": 4
            },
            "period_days": 30
        });
        normalize_hermes_usage_nulls(&mut value);
        let report: HermesUsageReport = serde_json::from_value(value).unwrap();
        assert_eq!(report.totals.total_cache_read, 0);
        assert_eq!(report.daily[0].actual_cost, 0.0);
        assert_eq!(report.by_model[0].model, "deepseek-v4-pro");
        assert_eq!(report.totals.total_api_calls, 4);
    }
}
