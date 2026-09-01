// ============================================================
// Track B · 智能体演进（AG-08 追加）：MCP stdio 客户端适配器（Spike 门禁⑥）
// 实施基线：docs/architecture.md「ToolGateway 是真正的核心」——
// 外部工具一律经统一契约进同一个 ToolRegistry；MCP 是首个真实外部工具源。
//
// 边界：
// - rmcp 类型全部收敛在本模块（硬性限制⑤对 rig 的同款口径）：
//   tools/mod.rs 契约面（ToolDescriptor/ToolOutput/ToolError/SophoNoteTool）不出现 rmcp；
//   驱动循环（run_controller.rs）对 MCP 工具与内置假工具一视同仁。
// - Spike 期口径：stdio 握手 → list_all_tools → call_tool_once（单次确定性调用）。
//   MRTR 追问轮（call_tool 自动续轮）、InputRequired、Tasks 扩展全部不做——
//   前两者是交互式能力，属 Phase 2 ToolGateway 完整面；本模块遇之显式报错。
// - 连接生命周期由调用方命令持有（用完 drop，rmcp DropGuard 负责关停子进程）——
//   Spike 调试路径保持该口径；正式路径见下方 AG-28 McpManager（常驻/重连/退出清理）。
//
// AG-28（Phase 5，docs/architecture.md「MCP 接入策略」全量落点）：
// - 常驻 McpManager：安装配置持久化（mcp_servers）、启动/调用超时、有限懒重连、
//   应用退出清理子进程；状态口径 disabled/disconnected/connected/failed
//   （connecting 为持锁期瞬态，不对外序列化）。
// - 工具授权（mcp_tool_auth）：默认拒绝，仅授权且已连接的工具进 ToolRegistry；
//   工具名命名空间化 mcp.<server>.<tool>（§九：避免与内置工具冲突）。
// - 安全边界（结构性）：① 模型不能安装/启动 MCP——本文件管理命令全部是用户侧
//   Tauri 命令，从不注册进任何 ToolRegistry；② 文档写仍经 DocumentService——
//   MCP 工具是外部进程，不触达文档域；③ 结果大小限制（§九/§十二 max_tool_output）。
// ============================================================
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::ApiResponse;
use crate::tools::{SophoNoteTool, ProvenanceRef, ToolDescriptor, ToolError, ToolOutput};

/// MCP 连接（子进程拉起 + initialize 握手 + tools/list）总超时（docs/architecture.md「启动超时」）
pub const MCP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// 单次 tools/call 超时（docs/architecture.md「调用超时」；Run 总预算另见 §十二）
pub const MCP_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// 工具结果 model_text 上限（docs/architecture.md「结果大小限制」/ §十二 max_tool_output 20-50KB）
const MCP_MAX_MODEL_TEXT_BYTES: usize = 50 * 1024;
/// 懒重连连续失败上限：超过后不再自动重连，等待用户显式连接（docs/architecture.md「最大重启次数」）
const MCP_MAX_AUTO_RECONNECT_FAILURES: u32 = 3;

/// MCP stdio 服务器启动配置。Spike 调试命令入参现传（不落库）；正式路径由
/// AG-28 McpManager 持久化到 mcp_servers 表（用户显式安装，模型不可触达）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStdioConfig {
    /// 可执行命令（如 "npx"）
    pub command: String,
    /// 命令参数（如 ["-y", "@modelcontextprotocol/server-everything"]）
    #[serde(default)]
    pub args: Vec<String>,
    /// 附加环境变量（如 API Key）；合并进子进程环境而非替换
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

/// 握手成功后的客户端句柄。RunningService 内含后台读任务与 DropGuard，
/// 非 Clone → Arc 共享；所有 McpTool 持同一句柄。
pub type McpClient = Arc<rmcp::service::RunningService<rmcp::RoleClient, ()>>;

/// 拉起 stdio 子进程并完成 MCP initialize 握手
pub async fn connect_stdio(cfg: &McpStdioConfig) -> Result<McpClient, String> {
    use rmcp::transport::child_process::TokioChildProcess;
    use rmcp::ServiceExt;

    let mut cmd = tokio::process::Command::new(&cfg.command);
    cmd.args(&cfg.args);
    if let Some(env) = &cfg.env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    let transport = TokioChildProcess::new(cmd).map_err(|e| {
        format!(
            "MCP 子进程启动失败（{} {}）: {}",
            cfg.command,
            cfg.args.join(" "),
            e
        )
    })?;

    let client = ().serve(transport).await.map_err(|e| format!("MCP initialize 握手失败: {}", e))?;
    Ok(Arc::new(client))
}

/// rmcp Tool → 自有 ToolDescriptor（input_schema: Arc<JsonObject> → Value::Object）
pub fn descriptor_from_mcp_tool(tool: &rmcp::model::Tool) -> ToolDescriptor {
    ToolDescriptor {
        name: tool.name.to_string(),
        description: tool.description.as_deref().unwrap_or("").to_string(),
        input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
    }
}

/// 从 content blocks 提取模型可读文本；缺失时回落 structured_content JSON，再缺失为空串
fn model_text_from_result(result: &rmcp::model::CallToolResult) -> String {
    let texts: Vec<&str> = result
        .content
        .iter()
        .filter_map(|block| match block {
            rmcp::model::ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    if !texts.is_empty() {
        return texts.join("\n");
    }
    match &result.structured_content {
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// rmcp CallToolResult → ToolOutput / ToolError。
/// is_error=true 走 Err(Execution) 且带上 content 文本——run_controller 的工具错误
/// 路径会把文本交还模型自行决策（状态机要求「每个调用都有应答」，与内置工具同口径）。
/// AG-21：provenance 标记来源 "mcp" + 工具名（RunStore 可追溯外部来源）。
pub fn tool_output_from_result(
    result: rmcp::model::CallToolResult,
    tool_name: &str,
) -> Result<ToolOutput, ToolError> {
    let mut text = model_text_from_result(&result);
    let mut truncated = false;
    if text.len() > MCP_MAX_MODEL_TEXT_BYTES {
        // docs/architecture.md 结果大小限制：按 UTF-8 字符边界截断并置 truncated 标记
        // （前端卡片显「内容已截断」；模型只看到截断后文本）
        let mut end = MCP_MAX_MODEL_TEXT_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n…[内容过长已截断]");
        truncated = true;
    }
    if result.is_error.unwrap_or(false) {
        return Err(ToolError::Execution(if text.is_empty() {
            "工具返回错误（无详情）".to_string()
        } else {
            text
        }));
    }
    let mut out = ToolOutput::text(
        text,
        result.structured_content.unwrap_or(serde_json::Value::Null),
    );
    out.provenance = vec![ProvenanceRef::new("mcp").with_id(tool_name)];
    out.truncated = truncated;
    Ok(out)
}

/// 把单个 MCP 工具适配为 SophoNoteTool，注册进与内置工具同一个 ToolRegistry。
/// AG-28：descriptor.name 是注册表面名称（正式路径为命名空间化
/// mcp.<server>.<tool>，docs/architecture.md）；wire_name 是服务器侧原名
/// （tools/call 线名）。Spike discover() 两者同名传入，行为不变。
pub struct McpTool {
    descriptor: ToolDescriptor,
    wire_name: String,
    client: McpClient,
}

impl McpTool {
    pub fn new(descriptor: ToolDescriptor, wire_name: String, client: McpClient) -> Self {
        Self {
            descriptor,
            wire_name,
            client,
        }
    }
}

#[async_trait]
impl SophoNoteTool for McpTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let serde_json::Value::Object(map) = arguments else {
            return Err(ToolError::InvalidArguments(
                "MCP 工具参数必须是 JSON 对象".into(),
            ));
        };
        let params =
            rmcp::model::CallToolRequestParams::new(self.wire_name.clone()).with_arguments(map);
        // AG-28（docs/architecture.md 调用超时）：单次 tools/call 硬超时，超时以执行错误交还模型
        match tokio::time::timeout(MCP_CALL_TIMEOUT, self.client.call_tool_once(params)).await {
            Err(_) => Err(ToolError::Execution(format!(
                "MCP 工具调用超时（>{}s）: {}",
                MCP_CALL_TIMEOUT.as_secs(),
                self.wire_name
            ))),
            Ok(Ok(rmcp::model::CallToolResponse::Complete(result))) => {
                tool_output_from_result(result, &self.wire_name)
            }
            Ok(Ok(rmcp::model::CallToolResponse::InputRequired(_))) => Err(ToolError::Execution(
                "工具要求补充输入（MRTR InputRequired），Spike 期不支持".into(),
            )),
            Ok(Ok(rmcp::model::CallToolResponse::Task(_))) => Err(ToolError::Execution(
                "工具返回异步任务（Tasks 扩展），Spike 期不支持".into(),
            )),
            Ok(Err(e)) => Err(ToolError::Execution(format!("MCP 调用失败: {}", e))),
            // CallToolResponse 为 non_exhaustive：未来新增响应形态一律显式报错，
            // 不静默吞掉（Spike 口径：未实现的协议面必须对模型可见）
            Ok(Ok(_unsupported)) => Err(ToolError::Execution(
                "收到 Spike 期不支持的 MCP 响应形态".into(),
            )),
        }
    }
}

/// 一站式发现：连接 → tools/list → 产出（描述符列表，McpTool 实例列表）。
/// 命令层把实例逐个 register 进 ToolRegistry——门禁⑥「MCP 工具与内置工具同表」的落点。
pub async fn discover(
    cfg: &McpStdioConfig,
) -> Result<(Vec<ToolDescriptor>, Vec<Arc<McpTool>>), String> {
    let client = connect_stdio(cfg).await?;
    let tools = client
        .list_all_tools()
        .await
        .map_err(|e| format!("MCP tools/list 失败: {}", e))?;
    let descriptors: Vec<ToolDescriptor> = tools.iter().map(descriptor_from_mcp_tool).collect();
    let instances: Vec<Arc<McpTool>> = tools
        .iter()
        .map(|t| {
            Arc::new(McpTool::new(
                descriptor_from_mcp_tool(t),
                t.name.to_string(), // Spike 调试路径不做命名空间化（正式路径见 McpManager）
                client.clone(),
            ))
        })
        .collect();
    Ok((descriptors, instances))
}

// ============================================================
// AG-28（Phase 5）：常驻 McpManager（docs/architecture.md「MCP 接入策略」）
//
// 职责：安装配置持久化（mcp_servers）、连接状态（disconnected/connected/failed，
// enabled=false 为 disabled）、启动/调用超时、有限懒重连（Run 启动时，连续失败
// ≤ MCP_MAX_AUTO_RECONNECT_FAILURES 次后等待显式重连）、工具授权（mcp_tool_auth，
// 默认拒绝）、命名空间化注册（mcp.<server>.<tool>）、应用退出清理。
//
// 子进程 stderr 继承父进程 → dev.log（docs/architecture.md「stderr 日志采集」的最小落点：
// 不额外管道化，避免管道满导致子进程阻塞）。
// ============================================================

/// 命名空间化工具名（docs/architecture.md：`mcp.<server>.<tool>`，与内置工具永不冲突）
pub fn mcp_tool_id(server_name: &str, tool_name: &str) -> String {
    format!("mcp.{}.{}", server_name, tool_name)
}

/// 服务器名校验：1-32 位小写字母/数字/连字符，字母开头（命名空间分段安全）
pub fn valid_server_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// mcp_servers 行（DB 真相源的内存镜像）
#[derive(Debug, Clone, PartialEq)]
struct ServerRecord {
    name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    enabled: bool,
}

impl ServerRecord {
    fn to_stdio_config(&self) -> McpStdioConfig {
        McpStdioConfig {
            command: self.command.clone(),
            args: self.args.clone(),
            env: if self.env.is_empty() {
                None
            } else {
                Some(self.env.clone())
            },
        }
    }
}

/// 单个服务器的运行期状态（不落库；重启后由懒重连恢复）。
/// 不派生 Debug：McpClient（rmcp RunningService）不保证实现 Debug。
struct ServerRuntime {
    record: ServerRecord,
    client: Option<McpClient>,
    /// 最近一次连接成功时 tools/list 的描述符（wire 名）
    tools: Vec<ToolDescriptor>,
    last_error: Option<String>,
    /// 懒重连连续失败计数（显式连接成功/配置变更时清零）
    auto_failures: u32,
}

impl ServerRuntime {
    fn new(record: ServerRecord) -> Self {
        Self {
            record,
            client: None,
            tools: Vec::new(),
            last_error: None,
            auto_failures: 0,
        }
    }
}

/// mcp_tool_auth 行（工具清单 + 授权位）
#[derive(Debug, Clone)]
struct ToolAuthRow {
    tool_name: String,
    description: String,
    authorized: bool,
}

/// 前端展示用服务器视图（camelCase；env 只出键名不出值——值可能含密钥）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub enabled: bool,
    /// disabled | disconnected | connected | failed
    pub status: String,
    pub last_error: Option<String>,
    pub tools: Vec<McpToolView>,
}

/// 前端展示用工具条目（授权开关的数据源）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolView {
    /// 服务器侧原名（wire 名）
    pub name: String,
    /// 命名空间化注册名 mcp.<server>.<name>（Skill 声明工具用这个）
    pub id: String,
    pub description: String,
    pub authorized: bool,
}

/// 常驻 MCP 管理器。全局单例（lib.rs setup 中 `app.manage(Arc::new(McpManager::new()))`）。
/// 锁用 tokio::sync::Mutex：临界区内有 await（连接握手），std Mutex 不得跨 await
/// （AG-30 同款教训）。
pub struct McpManager {
    servers: tokio::sync::Mutex<HashMap<String, ServerRuntime>>,
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            servers: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 打开 DB 并幂等建 MCP 表（兼容未重启升级的旧库，与 skills 同款口径）
    fn open_db(db_path: &std::path::Path) -> Result<rusqlite::Connection, String> {
        let conn =
            rusqlite::Connection::open(db_path).map_err(|e| format!("打开数据库失败: {}", e))?;
        let _ = create_mcp_tables(&conn);
        Ok(conn)
    }

    /// 内存运行期与 DB 记录对齐：删除已移除的服务器（drop client = 杀子进程），
    /// 配置变更的重置连接（command/args/env/enabled 任一变化都需重连）
    async fn reconcile(&self, records: &HashMap<String, ServerRecord>) {
        let mut guard = self.servers.lock().await;
        guard.retain(|name, _| records.contains_key(name));
        for rt in guard.values_mut() {
            if let Some(rec) = records.get(&rt.record.name) {
                if rt.record != *rec {
                    rt.record = rec.clone();
                    rt.client = None;
                    rt.tools.clear();
                    rt.last_error = None;
                    rt.auto_failures = 0;
                }
            }
        }
    }

    fn load_records(db_path: &std::path::Path) -> Result<HashMap<String, ServerRecord>, String> {
        let conn = Self::open_db(db_path)?;
        read_server_records(&conn)
    }

    /// 服务器列表（状态 + 工具授权面；管理面板数据源）
    pub async fn list_servers(
        &self,
        db_path: &std::path::Path,
    ) -> Result<Vec<McpServerView>, String> {
        let records = Self::load_records(db_path)?;
        self.reconcile(&records).await;
        let conn = Self::open_db(db_path)?;
        let guard = self.servers.lock().await;
        let mut views = Vec::new();
        for rec in records.values() {
            let rows = read_tool_auth(&conn, &rec.name)?;
            views.push(render_view(rec, guard.get(&rec.name), &rows));
        }
        views.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(views)
    }

    /// 单服务器视图（变更类命令的统一返回值构造）
    async fn server_view(
        &self,
        db_path: &std::path::Path,
        name: &str,
    ) -> Result<McpServerView, String> {
        let conn = Self::open_db(db_path)?;
        let records = read_server_records(&conn)?;
        let Some(rec) = records.get(name) else {
            return Err(format!("MCP 服务器 {} 不存在", name));
        };
        let rows = read_tool_auth(&conn, name)?;
        let guard = self.servers.lock().await;
        Ok(render_view(rec, guard.get(name), &rows))
    }

    /// 安装服务器（docs/architecture.md：配置由用户显式安装）：校验 → 落库 → 立即连接。
    /// 连接失败不回滚安装——配置保留、status=failed、错误可见、可重试。
    pub async fn add_server(
        &self,
        db_path: &std::path::Path,
        name: String,
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<McpServerView, String> {
        if !valid_server_name(&name) {
            return Err("服务器名须为 1-32 位小写字母/数字/连字符，且字母开头".into());
        }
        if command.trim().is_empty() {
            return Err("启动命令不能为空".into());
        }
        {
            let conn = Self::open_db(db_path)?;
            if server_exists(&conn, &name)? {
                return Err(format!("MCP 服务器 {} 已存在", name));
            }
            insert_server_record(
                &conn,
                &ServerRecord {
                    name: name.clone(),
                    command,
                    args,
                    env,
                    enabled: true,
                },
            )?;
        }
        let _ = self.connect_core(db_path, &name).await; // 失败体现在视图 lastError
        self.server_view(db_path, &name).await
    }

    /// 移除服务器：断开（drop client = 杀子进程）+ 删配置与授权行
    pub async fn remove_server(&self, db_path: &std::path::Path, name: &str) -> Result<(), String> {
        {
            let mut guard = self.servers.lock().await;
            guard.remove(name);
        }
        let conn = Self::open_db(db_path)?;
        delete_server_rows(&conn, name)
    }

    /// 启用/停用。停用立即断开（disabled 服务器不连接、不出工具）
    pub async fn set_enabled(
        &self,
        db_path: &std::path::Path,
        name: &str,
        enabled: bool,
    ) -> Result<McpServerView, String> {
        {
            let conn = Self::open_db(db_path)?;
            let mut records = read_server_records(&conn)?;
            let Some(rec) = records.get_mut(name) else {
                return Err(format!("MCP 服务器 {} 不存在", name));
            };
            rec.enabled = enabled;
            upsert_server_record(&conn, rec)?;
            if !enabled {
                let mut guard = self.servers.lock().await;
                if let Some(rt) = guard.get_mut(name) {
                    rt.record = rec.clone();
                    rt.client = None;
                    rt.tools.clear();
                    rt.last_error = None;
                    rt.auto_failures = 0;
                }
            }
        }
        self.server_view(db_path, name).await
    }

    /// 显式连接（用户动作）：重置懒重连失败计数后连接
    pub async fn connect_server(
        &self,
        db_path: &std::path::Path,
        name: &str,
    ) -> Result<McpServerView, String> {
        {
            let mut guard = self.servers.lock().await;
            if let Some(rt) = guard.get_mut(name) {
                rt.auto_failures = 0;
            }
        }
        if let Some(err) = self.connect_core(db_path, name).await? {
            return Err(err);
        }
        self.server_view(db_path, name).await
    }

    /// 显式断开（docs/architecture.md「主动断开」）
    pub async fn disconnect_server(
        &self,
        db_path: &std::path::Path,
        name: &str,
    ) -> Result<McpServerView, String> {
        {
            let mut guard = self.servers.lock().await;
            if let Some(rt) = guard.get_mut(name) {
                rt.client = None;
                rt.tools.clear();
                rt.last_error = None;
                rt.auto_failures = 0;
            }
        }
        self.server_view(db_path, name).await
    }

    /// 连接核心：握手 + tools/list（总超时 MCP_CONNECT_TIMEOUT）+ 工具清单落库
    /// （新工具默认拒绝授权）。Ok(Some(err)) = 连接失败但流程完成（错误入 lastError）。
    async fn connect_core(
        &self,
        db_path: &std::path::Path,
        name: &str,
    ) -> Result<Option<String>, String> {
        let records = Self::load_records(db_path)?;
        let Some(rec) = records.get(name) else {
            return Err(format!("MCP 服务器 {} 不存在", name));
        };
        if !rec.enabled {
            return Err(format!("MCP 服务器 {} 已停用，启用后才能连接", name));
        }
        self.reconcile(&records).await;
        let cfg = rec.to_stdio_config();
        {
            let mut guard = self.servers.lock().await;
            let rt = guard
                .entry(name.to_string())
                .or_insert_with(|| ServerRuntime::new(rec.clone()));
            rt.last_error = None;
        }
        let outcome = match tokio::time::timeout(MCP_CONNECT_TIMEOUT, connect_and_list(&cfg)).await
        {
            Ok(inner) => inner,
            Err(_) => Err(format!(
                "MCP 连接超时（>{}s）: {} {}",
                MCP_CONNECT_TIMEOUT.as_secs(),
                cfg.command,
                cfg.args.join(" ")
            )),
        };
        let connect_err = outcome.as_ref().err().cloned();
        let mut guard = self.servers.lock().await;
        let Some(rt) = guard.get_mut(name) else {
            return Err(format!("MCP 服务器 {} 运行态丢失", name));
        };
        match outcome {
            Ok((client, tools)) => {
                let descriptors: Vec<ToolDescriptor> =
                    tools.iter().map(descriptor_from_mcp_tool).collect();
                let conn = Self::open_db(db_path)?;
                let inventory: Vec<(String, String)> = descriptors
                    .iter()
                    .map(|d| (d.name.clone(), d.description.clone()))
                    .collect();
                sync_tool_inventory(&conn, name, &inventory)?;
                rt.client = Some(client);
                rt.tools = descriptors;
                rt.last_error = None;
                rt.auto_failures = 0;
            }
            Err(e) => {
                rt.client = None;
                rt.tools.clear();
                rt.last_error = Some(e);
                rt.auto_failures += 1;
            }
        }
        Ok(connect_err)
    }

    /// Run 启动懒重连（docs/architecture.md「MCP Server 崩溃 → 标记 degraded，有限重启」）：
    /// 对启用但未连接的服务器各尝试一次；连续失败达上限的跳过等待显式重连。
    /// 返回日志行（调用方 println 到 dev.log）。
    pub async fn prepare_for_run(&self, db_path: &std::path::Path) -> Vec<String> {
        let records = match Self::load_records(db_path) {
            Ok(r) => r,
            Err(e) => return vec![format!("MCP 读取配置失败: {}", e)],
        };
        self.reconcile(&records).await;
        let mut notes = Vec::new();
        for rec in records.values().filter(|r| r.enabled) {
            let need = {
                let guard = self.servers.lock().await;
                match guard.get(&rec.name) {
                    Some(rt) if rt.client.is_some() => false,
                    Some(rt) => rt.auto_failures < MCP_MAX_AUTO_RECONNECT_FAILURES,
                    None => true,
                }
            };
            if !need {
                continue;
            }
            match self.connect_core(db_path, &rec.name).await {
                Ok(None) => notes.push(format!("mcp[{}] 已连接（Run 懒重连）", rec.name)),
                Ok(Some(e)) => notes.push(format!("mcp[{}] 重连失败: {}", rec.name, e)),
                Err(e) => notes.push(format!("mcp[{}] 跳过: {}", rec.name, e)),
            }
        }
        notes
    }

    /// 已连接且已授权的工具实例（注册表面名称已命名空间化）——
    /// 正式 Chat registry 注入的唯一来源（「仅授权工具可见」执行点）
    pub async fn ready_tools(&self, db_path: &std::path::Path) -> Vec<Arc<dyn SophoNoteTool>> {
        let authorized = match Self::open_db(db_path).and_then(|conn| read_authorized_pairs(&conn))
        {
            Ok(set) => set,
            Err(e) => {
                eprintln!("[mcp] 读取授权工具失败（按无授权处理）: {}", e);
                return Vec::new();
            }
        };
        let guard = self.servers.lock().await;
        let mut tools: Vec<Arc<dyn SophoNoteTool>> = Vec::new();
        for rt in guard.values() {
            let Some(client) = rt.client.as_ref() else {
                continue;
            };
            for d in &rt.tools {
                if authorized.contains(&(rt.record.name.clone(), d.name.clone())) {
                    tools.push(Arc::new(McpTool::new(
                        ToolDescriptor {
                            name: mcp_tool_id(&rt.record.name, &d.name),
                            description: d.description.clone(),
                            input_schema: d.input_schema.clone(),
                        },
                        d.name.clone(),
                        client.clone(),
                    )));
                }
            }
        }
        tools
    }

    /// 已连接且已授权工具的命名空间化名称集合（Skill 权限交集的「当前可用」侧并入项）
    pub async fn ready_tool_ids(&self, db_path: &std::path::Path) -> BTreeSet<String> {
        self.ready_tools(db_path)
            .await
            .iter()
            .map(|t| t.descriptor().name)
            .collect()
    }

    /// 工具授权开关（docs/architecture.md 工具 allowlist 的产品形态：默认拒绝，逐个授权）
    pub async fn set_tool_authorized(
        &self,
        db_path: &std::path::Path,
        server: &str,
        tool: &str,
        authorized: bool,
    ) -> Result<Vec<McpToolView>, String> {
        let conn = Self::open_db(db_path)?;
        if !server_exists(&conn, server)? {
            return Err(format!("MCP 服务器 {} 不存在", server));
        }
        set_tool_auth_authorized(&conn, server, tool, authorized)?;
        let rows = read_tool_auth(&conn, server)?;
        Ok(rows
            .iter()
            .map(|row| McpToolView {
                name: row.tool_name.clone(),
                id: mcp_tool_id(server, &row.tool_name),
                description: row.description.clone(),
                authorized: row.authorized,
            })
            .collect())
    }

    /// 应用退出清理（docs/architecture.md）：drop 全部 client → rmcp DropGuard 杀子进程。
    /// 同步版本供 lib.rs RunEvent::Exit（同步回调）调用。
    pub fn shutdown_blocking(&self) {
        let mut guard = self.servers.blocking_lock();
        let connected = guard.values().filter(|rt| rt.client.is_some()).count();
        for rt in guard.values_mut() {
            rt.client = None;
            rt.tools.clear();
        }
        if connected > 0 {
            println!("[mcp] shutdown: stopped {} server(s)", connected);
        }
    }
}

/// 连接 + tools/list（供 connect_core 套总超时）
async fn connect_and_list(
    cfg: &McpStdioConfig,
) -> Result<(McpClient, Vec<rmcp::model::Tool>), String> {
    let client = connect_stdio(cfg).await?;
    let tools = client
        .list_all_tools()
        .await
        .map_err(|e| format!("MCP tools/list 失败: {}", e))?;
    Ok((client, tools))
}

fn render_view(
    rec: &ServerRecord,
    rt: Option<&ServerRuntime>,
    rows: &[ToolAuthRow],
) -> McpServerView {
    let status = if !rec.enabled {
        "disabled"
    } else {
        match rt {
            Some(r) if r.client.is_some() => "connected",
            Some(r) if r.last_error.is_some() => "failed",
            _ => "disconnected",
        }
    };
    let mut env_keys: Vec<String> = rec.env.keys().cloned().collect();
    env_keys.sort();
    McpServerView {
        name: rec.name.clone(),
        command: rec.command.clone(),
        args: rec.args.clone(),
        env_keys,
        enabled: rec.enabled,
        status: status.to_string(),
        last_error: rt.and_then(|r| r.last_error.clone()),
        tools: rows
            .iter()
            .map(|row| McpToolView {
                name: row.tool_name.clone(),
                id: mcp_tool_id(&rec.name, &row.tool_name),
                description: row.description.clone(),
                authorized: row.authorized,
            })
            .collect(),
    }
}

// ---------------- 持久化（mcp_servers / mcp_tool_auth） ----------------

/// 建表（幂等）；db.rs::create_schema 与 McpManager::open_db 双挂钩
pub fn create_mcp_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_servers (
            name TEXT PRIMARY KEY,
            command TEXT NOT NULL,
            args_json TEXT NOT NULL DEFAULT '[]',
            env_json TEXT NOT NULL DEFAULT '{}',
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS mcp_tool_auth (
            server_name TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            authorized INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (server_name, tool_name)
        );",
    )
    .map_err(|e| format!("建 MCP 表失败: {}", e))
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn server_exists(conn: &rusqlite::Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM mcp_servers WHERE name = ?1",
        rusqlite::params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(|e| format!("查询 MCP 服务器失败: {}", e))
}

fn insert_server_record(conn: &rusqlite::Connection, rec: &ServerRecord) -> Result<(), String> {
    let now = now_stamp();
    let args_json = serde_json::to_string(&rec.args).unwrap_or_else(|_| "[]".into());
    let env_json = serde_json::to_string(&rec.env).unwrap_or_else(|_| "{}".into());
    conn.execute(
        "INSERT INTO mcp_servers (name, command, args_json, env_json, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            rec.name,
            rec.command,
            args_json,
            env_json,
            rec.enabled as i32,
            now,
            now
        ],
    )
    .map_err(|e| format!("写入 MCP 服务器失败: {}", e))?;
    Ok(())
}

fn upsert_server_record(conn: &rusqlite::Connection, rec: &ServerRecord) -> Result<(), String> {
    let args_json = serde_json::to_string(&rec.args).unwrap_or_else(|_| "[]".into());
    let env_json = serde_json::to_string(&rec.env).unwrap_or_else(|_| "{}".into());
    conn.execute(
        "UPDATE mcp_servers SET command = ?2, args_json = ?3, env_json = ?4, enabled = ?5, updated_at = ?6
         WHERE name = ?1",
        rusqlite::params![
            rec.name,
            rec.command,
            args_json,
            env_json,
            rec.enabled as i32,
            now_stamp()
        ],
    )
    .map_err(|e| format!("更新 MCP 服务器失败: {}", e))?;
    Ok(())
}

fn read_server_records(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, ServerRecord>, String> {
    let mut stmt = conn
        .prepare("SELECT name, command, args_json, env_json, enabled FROM mcp_servers")
        .map_err(|e| format!("查询 MCP 服务器失败: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i32>(4)?,
            ))
        })
        .map_err(|e| format!("查询 MCP 服务器失败: {}", e))?;
    let mut records = HashMap::new();
    for row in rows {
        let (name, command, args_json, env_json, enabled) =
            row.map_err(|e| format!("读取 MCP 服务器行失败: {}", e))?;
        let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
        let env: HashMap<String, String> = serde_json::from_str(&env_json).unwrap_or_default();
        records.insert(
            name.clone(),
            ServerRecord {
                name,
                command,
                args,
                env,
                enabled: enabled != 0,
            },
        );
    }
    Ok(records)
}

fn delete_server_rows(conn: &rusqlite::Connection, name: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM mcp_tool_auth WHERE server_name = ?1",
        rusqlite::params![name],
    )
    .map_err(|e| format!("删除 MCP 授权行失败: {}", e))?;
    conn.execute(
        "DELETE FROM mcp_servers WHERE name = ?1",
        rusqlite::params![name],
    )
    .map_err(|e| format!("删除 MCP 服务器失败: {}", e))?;
    Ok(())
}

/// 工具清单落库：新工具 INSERT（authorized 默认 0 = 拒绝），已有工具只更新描述
fn sync_tool_inventory(
    conn: &rusqlite::Connection,
    server: &str,
    inventory: &[(String, String)],
) -> Result<(), String> {
    for (tool, description) in inventory {
        conn.execute(
            "INSERT INTO mcp_tool_auth (server_name, tool_name, description, authorized)
             VALUES (?1, ?2, ?3, 0)
             ON CONFLICT(server_name, tool_name) DO UPDATE SET description = excluded.description",
            rusqlite::params![server, tool, description],
        )
        .map_err(|e| format!("同步 MCP 工具清单失败: {}", e))?;
    }
    Ok(())
}

fn read_tool_auth(conn: &rusqlite::Connection, server: &str) -> Result<Vec<ToolAuthRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, description, authorized FROM mcp_tool_auth
             WHERE server_name = ?1 ORDER BY tool_name",
        )
        .map_err(|e| format!("查询 MCP 工具授权失败: {}", e))?;
    let rows = stmt
        .query_map(rusqlite::params![server], |row| {
            Ok(ToolAuthRow {
                tool_name: row.get(0)?,
                description: row.get(1)?,
                authorized: row.get::<_, i32>(2)? != 0,
            })
        })
        .map_err(|e| format!("查询 MCP 工具授权失败: {}", e))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 MCP 工具授权行失败: {}", e))
}

/// 全部已授权（server, tool）对——ready_tools 的过滤依据
fn read_authorized_pairs(
    conn: &rusqlite::Connection,
) -> Result<BTreeSet<(String, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT server_name, tool_name FROM mcp_tool_auth WHERE authorized = 1")
        .map_err(|e| format!("查询 MCP 授权失败: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("查询 MCP 授权失败: {}", e))?;
    let mut set = BTreeSet::new();
    for row in rows {
        let pair = row.map_err(|e| format!("读取 MCP 授权行失败: {}", e))?;
        set.insert(pair);
    }
    Ok(set)
}

fn set_tool_auth_authorized(
    conn: &rusqlite::Connection,
    server: &str,
    tool: &str,
    authorized: bool,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE mcp_tool_auth SET authorized = ?3 WHERE server_name = ?1 AND tool_name = ?2",
            rusqlite::params![server, tool, authorized as i32],
        )
        .map_err(|e| format!("更新 MCP 工具授权失败: {}", e))?;
    if n == 0 {
        return Err(format!(
            "工具 {} 不存在于服务器 {}（连接成功后才有工具清单）",
            tool, server
        ));
    }
    Ok(())
}

// ============================================================
// AG-28 用户侧管理命令。安全边界：这些命令从不注册进任何 ToolRegistry——
// 模型不能安装/启动/断开 MCP，也不能改授权（docs/architecture.md「不让模型决定启动
// 哪个 MCP 命令」的结构性保证，与文档写工具同口径）。
// ============================================================

#[tauri::command]
pub async fn mcp_server_list(app: AppHandle) -> ApiResponse<Vec<McpServerView>> {
    // Tauri 约束：async command 含引用入参必须返回 Result；本仓统一 ApiResponse，
    // 故去掉 State<'_> 入参，经 owned AppHandle 内部取（同 agent/commands.rs 步骤 1.55）。
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager.list_servers(&crate::db::get_db_path(&app)).await {
        Ok(views) => ApiResponse::ok(views),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn mcp_server_add(
    app: AppHandle,
    name: String,
    command: String,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
) -> ApiResponse<McpServerView> {
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager
        .add_server(
            &crate::db::get_db_path(&app),
            name,
            command,
            args.unwrap_or_default(),
            env.unwrap_or_default(),
        )
        .await
    {
        Ok(view) => ApiResponse::ok(view),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn mcp_server_remove(app: AppHandle, name: String) -> ApiResponse<()> {
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager
        .remove_server(&crate::db::get_db_path(&app), &name)
        .await
    {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn mcp_server_set_enabled(
    app: AppHandle,
    name: String,
    enabled: bool,
) -> ApiResponse<McpServerView> {
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager
        .set_enabled(&crate::db::get_db_path(&app), &name, enabled)
        .await
    {
        Ok(view) => ApiResponse::ok(view),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn mcp_server_connect(app: AppHandle, name: String) -> ApiResponse<McpServerView> {
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager
        .connect_server(&crate::db::get_db_path(&app), &name)
        .await
    {
        Ok(view) => ApiResponse::ok(view),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn mcp_server_disconnect(app: AppHandle, name: String) -> ApiResponse<McpServerView> {
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager
        .disconnect_server(&crate::db::get_db_path(&app), &name)
        .await
    {
        Ok(view) => ApiResponse::ok(view),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn mcp_tool_set_authorized(
    app: AppHandle,
    server: String,
    tool: String,
    authorized: bool,
) -> ApiResponse<Vec<McpToolView>> {
    let manager = app.state::<Arc<McpManager>>().inner().clone();
    match manager
        .set_tool_authorized(&crate::db::get_db_path(&app), &server, &tool, authorized)
        .await
    {
        Ok(tools) => ApiResponse::ok(tools),
        Err(e) => ApiResponse::err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool as McpToolSpec};
    use serde_json::json;

    fn sample_mcp_tool() -> McpToolSpec {
        let mut schema = JsonObject::new();
        schema.insert("type".into(), json!("object"));
        McpToolSpec::new("echo", "回显输入", schema)
    }

    #[test]
    fn descriptor_maps_name_description_and_schema() {
        let d = descriptor_from_mcp_tool(&sample_mcp_tool());
        assert_eq!(d.name, "echo");
        assert_eq!(d.description, "回显输入");
        assert_eq!(d.input_schema, json!({"type": "object"}));
    }

    #[test]
    fn success_result_maps_text_blocks_and_structured() {
        let mut result = CallToolResult::success(vec![
            ContentBlock::text("第一行"),
            ContentBlock::text("第二行"),
        ]);
        result.structured_content = Some(json!({"value": 42}));
        let out = tool_output_from_result(result, "echo").expect("success");
        assert_eq!(out.model_text, "第一行\n第二行");
        assert_eq!(out.structured, json!({"value": 42}));
        // AG-21：外部来源标记 mcp + 工具名（RunStore 可追溯）
        assert_eq!(out.provenance[0].source, "mcp");
        assert_eq!(out.provenance[0].source_id.as_deref(), Some("echo"));
    }

    #[test]
    fn error_result_becomes_tool_error_with_content_text() {
        let result = CallToolResult::error(vec![ContentBlock::text("参数越界")]);
        let err = tool_output_from_result(result, "echo").expect_err("is_error=true 应为 Err");
        assert_eq!(err, ToolError::Execution("参数越界".into()));
    }

    #[test]
    fn empty_content_falls_back_to_structured_json_text() {
        // 无文本块、仅 structured_content：model_text 回落为 JSON 字符串
        let result: CallToolResult =
            serde_json::from_value(json!({"structuredContent": {"k": "v"}})).expect("deserialize");
        let out = tool_output_from_result(result, "echo").expect("success");
        assert_eq!(out.model_text, json!({"k": "v"}).to_string());
        assert_eq!(out.structured, json!({"k": "v"}));
    }

    #[test]
    fn success_without_structured_defaults_to_null() {
        let out = tool_output_from_result(
            CallToolResult::success(vec![ContentBlock::text("ok")]),
            "echo",
        )
        .expect("success");
        assert_eq!(out.structured, serde_json::Value::Null);
    }

    // ---- AG-28：McpManager 零进程单测；AG-29：真实 stdio 由下方 python3 fixture 集成测试覆盖 ----

    /// 临时 DB（测完删目录）
    fn temp_db(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("sophonote-mcp-test-{}-{}", std::process::id(), tag));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let db = dir.join("t.db");
        (dir, db)
    }

    fn cleanup_dir(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn server_name_validation_rules() {
        assert!(valid_server_name("github"));
        assert!(valid_server_name("fs-local"));
        assert!(valid_server_name("a23"));
        assert!(!valid_server_name("")); // 空
        assert!(!valid_server_name("Github")); // 大写
        assert!(!valid_server_name("9lead")); // 数字开头
        assert!(!valid_server_name("-lead")); // 连字符开头
        assert!(!valid_server_name("has.dot")); // 命名空间分段冲突
        assert!(!valid_server_name("has space"));
        assert!(!valid_server_name("under_score"));
        assert!(!valid_server_name(&"a".repeat(33))); // 超长
    }

    #[test]
    fn tool_id_is_namespaced() {
        assert_eq!(
            mcp_tool_id("github", "search_issues"),
            "mcp.github.search_issues"
        );
    }

    #[test]
    fn mcp_tables_roundtrip_default_deny_and_delete() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        create_mcp_tables(&conn).expect("create tables");
        create_mcp_tables(&conn).expect("idempotent");

        let rec = ServerRecord {
            name: "github".into(),
            command: "github-mcp".into(),
            args: vec!["--stdio".into()],
            env: [("TOKEN".into(), "secret".into())].into_iter().collect(),
            enabled: true,
        };
        insert_server_record(&conn, &rec).expect("insert");
        assert!(server_exists(&conn, "github").unwrap());

        let records = read_server_records(&conn).expect("read");
        assert_eq!(records.get("github"), Some(&rec));

        // 工具清单落库：新工具默认拒绝授权（安全默认）
        sync_tool_inventory(
            &conn,
            "github",
            &[
                ("search_issues".into(), "搜索 issue".into()),
                ("get_issue".into(), "读取 issue".into()),
            ],
        )
        .expect("sync inventory");
        let rows = read_tool_auth(&conn, "github").expect("read auth");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| !r.authorized), "新工具必须默认拒绝");
        assert!(read_authorized_pairs(&conn).unwrap().is_empty());

        // 授权后进入可见集；重复同步不清空已有授权（只更新描述）
        set_tool_auth_authorized(&conn, "github", "search_issues", true).expect("authorize");
        let pairs = read_authorized_pairs(&conn).expect("pairs");
        assert!(pairs.contains(&("github".into(), "search_issues".into())));
        sync_tool_inventory(
            &conn,
            "github",
            &[("search_issues".into(), "新描述".into())],
        )
        .expect("resync");
        assert!(read_authorized_pairs(&conn)
            .unwrap()
            .contains(&("github".into(), "search_issues".into())));
        let rows = read_tool_auth(&conn, "github").expect("read auth 2");
        let si = rows
            .iter()
            .find(|r| r.tool_name == "search_issues")
            .unwrap();
        assert_eq!(si.description, "新描述");

        // 未知工具授权 → 报错（清单来自连接成功后的 tools/list）
        let err = set_tool_auth_authorized(&conn, "github", "nope", true).unwrap_err();
        assert!(err.contains("不存在"));

        // 删除服务器：配置与授权行一并清除
        delete_server_rows(&conn, "github").expect("delete");
        assert!(!server_exists(&conn, "github").unwrap());
        assert!(read_tool_auth(&conn, "github").unwrap().is_empty());
        assert!(read_authorized_pairs(&conn).unwrap().is_empty());
    }

    #[test]
    fn large_result_truncated_at_char_boundary() {
        // 3 字节汉字 × 20000 = 60000B > 50KB 上限；51200 % 3 == 2 → 边界落在字符中间
        let text = "中".repeat(20000);
        let result = CallToolResult::success(vec![ContentBlock::text(text)]);
        let out = tool_output_from_result(result, "echo").expect("success");
        assert!(out.truncated);
        assert!(out.model_text.len() <= MCP_MAX_MODEL_TEXT_BYTES + 64);
        assert!(out.model_text.ends_with("…[内容过长已截断]"));
        assert!(out.model_text.starts_with("中中中"));
        // 小结果不受影响
        let small = tool_output_from_result(
            CallToolResult::success(vec![ContentBlock::text("短文本")]),
            "echo",
        )
        .expect("success");
        assert!(!small.truncated);
        assert_eq!(small.model_text, "短文本");
    }

    #[tokio::test]
    async fn ready_tools_empty_without_connections() {
        let (dir, db) = temp_db("empty");
        let manager = McpManager::new();
        assert!(manager.ready_tools(&db).await.is_empty());
        assert!(manager.ready_tool_ids(&db).await.is_empty());
        assert!(manager.list_servers(&db).await.unwrap().is_empty());
        assert!(manager.prepare_for_run(&db).await.is_empty());
        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn connect_rejects_disabled_server_and_add_validates() {
        let (dir, db) = temp_db("disabled");
        // 直接落一条停用记录（绕过 add_server 的自动连接）
        {
            let conn = McpManager::open_db(&db).expect("open");
            insert_server_record(
                &conn,
                &ServerRecord {
                    name: "local".into(),
                    command: "echo".into(),
                    args: vec![],
                    env: HashMap::new(),
                    enabled: false,
                },
            )
            .expect("insert");
        }
        let manager = McpManager::new();
        let err = manager.connect_server(&db, "local").await.unwrap_err();
        assert!(err.contains("停用"));
        let views = manager.list_servers(&db).await.expect("list");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].status, "disabled");

        // 安装校验：非法名/空命令直接拒绝，不落库
        let err = manager
            .add_server(
                &db,
                "Bad Name".into(),
                "echo".into(),
                vec![],
                HashMap::new(),
            )
            .await
            .unwrap_err();
        assert!(err.contains("服务器名"));
        let err = manager
            .add_server(&db, "ok-name".into(), "  ".into(), vec![], HashMap::new())
            .await
            .unwrap_err();
        assert!(err.contains("命令"));
        assert!(!server_exists(&McpManager::open_db(&db).unwrap(), "ok-name").unwrap());
        cleanup_dir(&dir);
    }

    #[tokio::test]
    async fn set_enabled_toggles_record_and_view() {
        let (dir, db) = temp_db("toggle");
        {
            let conn = McpManager::open_db(&db).expect("open");
            insert_server_record(
                &conn,
                &ServerRecord {
                    name: "local".into(),
                    command: "echo".into(),
                    args: vec![],
                    env: HashMap::new(),
                    enabled: true,
                },
            )
            .expect("insert");
        }
        let manager = McpManager::new();
        let view = manager
            .set_enabled(&db, "local", false)
            .await
            .expect("disable");
        assert_eq!(view.status, "disabled");
        assert!(!view.enabled);
        let view = manager
            .set_enabled(&db, "local", true)
            .await
            .expect("enable");
        assert!(view.enabled);
        assert_eq!(view.status, "disconnected");
        // 不存在的服务器
        let err = manager.set_enabled(&db, "ghost", true).await.unwrap_err();
        assert!(err.contains("不存在"));
        cleanup_dir(&dir);
    }

    // ---- AG-29：真实 stdio 端到端（python3 最小 JSON-RPC fixture，零新增依赖） ----
    //
    // 覆盖 AG-28 遗留的「真实 stdio 连接」：安装→握手→tools/list→默认拒绝→
    // 授权→ready_tools→tools/call→50KB 字符边界截断→断开→重连→移除全链路。
    // python3 缺失时打印并直接通过（跳过），不阻塞门禁（宿主 macOS 自带 python3）。
    // fixture 说 MCP stdio 协议（换行分隔 JSON-RPC）：initialize 回显客户端提议的
    // protocolVersion，tools/list 报 echo/big 两工具，tools/call 真实执行。

    const FIXTURE_PY: &str = r#"
import json, sys

try:
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

TOOLS = [
    {"name": "echo", "description": "Echo back the text argument",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
    {"name": "big", "description": "Return a large multibyte payload",
     "inputSchema": {"type": "object"}},
]

def send(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    method = msg.get("method")
    mid = msg.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {
            "protocolVersion": (msg.get("params") or {}).get("protocolVersion", "2025-03-26"),
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "sophonote-fixture", "version": "0.1.0"},
        }})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = msg.get("params") or {}
        name = params.get("name")
        args = params.get("arguments") or {}
        if name == "echo":
            send({"jsonrpc": "2.0", "id": mid, "result": {
                "content": [{"type": "text", "text": str(args.get("text", ""))}],
                "isError": False}})
        elif name == "big":
            send({"jsonrpc": "2.0", "id": mid, "result": {
                "content": [{"type": "text", "text": "汉" * 60000}],
                "isError": False}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "unknown tool"}})
    elif method == "ping":
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
"#;

    fn python3_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_stdio_lifecycle_with_python_fixture() {
        if !python3_available() {
            eprintln!("[ag29] python3 不可用，跳过真实 stdio 集成测试");
            return;
        }
        let (dir, db) = temp_db("stdio");
        let script = dir.join("fixture_server.py");
        std::fs::write(&script, FIXTURE_PY).expect("write fixture");

        let manager = McpManager::new();

        // 1) 安装即自动连接（add_server 内部 connect_core）：握手 + tools/list
        let view = manager
            .add_server(
                &db,
                "pyfix".into(),
                "python3".into(),
                vec!["-u".into(), script.to_string_lossy().to_string()],
                HashMap::new(),
            )
            .await
            .expect("add_server");
        assert_eq!(view.status, "connected", "lastError: {:?}", view.last_error);
        assert_eq!(view.tools.len(), 2);
        assert!(
            view.tools.iter().all(|t| !t.authorized),
            "新工具必须默认拒绝"
        );
        assert!(view.tools.iter().any(|t| t.id == "mcp.pyfix.echo"));

        // 2) 默认拒绝 → ready 为空（结构性边界：未授权不可见）
        assert!(manager.ready_tools(&db).await.is_empty());

        // 3) 授权 → ready_tools 可见且命名空间化（Skill 交集面）
        let tools_view = manager
            .set_tool_authorized(&db, "pyfix", "echo", true)
            .await
            .expect("authorize echo");
        assert!(
            tools_view
                .iter()
                .find(|t| t.name == "echo")
                .unwrap()
                .authorized
        );
        let ready = manager.ready_tools(&db).await;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].descriptor().name, "mcp.pyfix.echo");
        assert!(manager.ready_tool_ids(&db).await.contains("mcp.pyfix.echo"));

        // 4) 真实 tools/call：echo 往返 + provenance 标记外部来源
        let out = ready[0]
            .execute(json!({"text": "hello-ag29"}))
            .await
            .expect("call echo");
        assert_eq!(out.model_text, "hello-ag29");
        assert!(!out.truncated);
        assert_eq!(out.provenance.len(), 1);

        // 5) 大结果 50KB 截断：180KB 多字节负载 → 字符边界截断 + 标记
        manager
            .set_tool_authorized(&db, "pyfix", "big", true)
            .await
            .expect("authorize big");
        let ready = manager.ready_tools(&db).await;
        let big = ready
            .iter()
            .find(|t| t.descriptor().name == "mcp.pyfix.big")
            .expect("big tool");
        let out = big.execute(json!({})).await.expect("call big");
        assert!(out.truncated);
        assert!(out.model_text.ends_with("\n…[内容过长已截断]"));
        assert!(out.model_text.len() <= MCP_MAX_MODEL_TEXT_BYTES + 64);
        // 截断位落在 UTF-8 字符边界：去掉尾巴后前缀仍是完整的「汉」序列（无半个字符）
        let body = out.model_text.trim_end_matches("\n…[内容过长已截断]");
        assert!(body.chars().all(|c| c == '汉'));

        // 6) 主动断开 → disconnected，ready 立即清空（DB 授权仍在）
        let view = manager
            .disconnect_server(&db, "pyfix")
            .await
            .expect("disconnect");
        assert_eq!(view.status, "disconnected");
        assert!(manager.ready_tools(&db).await.is_empty());

        // 7) 显式重连 → connected（覆盖断开后重连路径）
        let view = manager
            .connect_server(&db, "pyfix")
            .await
            .expect("reconnect");
        assert_eq!(view.status, "connected", "lastError: {:?}", view.last_error);

        // 8) 移除 → 配置与授权行清空，视图归零
        manager.remove_server(&db, "pyfix").await.expect("remove");
        assert!(manager.list_servers(&db).await.unwrap().is_empty());

        drop(manager); // drop client → DropGuard 杀子进程
        cleanup_dir(&dir);
    }
}
