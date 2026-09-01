//! Loopback HTTP MCP（Streamable-HTTP 兼容子集）：Hermes 经 url 调用 sophonote-bridge。
//! 内部仍走 SophonoteBridge::invoke_with_tools + SidecarLease。

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::tools::ToolRegistry;

use super::lease::{LeaseRegistry, SidecarLease};
use super::server::SophonoteBridge;
use super::tools::BridgeInvokeRequest;
use super::BRIDGE_MCP_NAME;

const HEADER_LEASE: &str = "x-sophonote-lease-id";

/// Bridge HTTP 运行时（进程内单例）
pub struct BridgeHttpRuntime {
    pub bridge: SophonoteBridge,
    pub bearer: String,
    pub base_url: String,
    /// lease_id → 该 Run 的 ToolRegistry
    run_tools: Mutex<HashMap<String, Arc<ToolRegistry>>>,
    /// Host 自动化工具需要 AppHandle 访问 SophoNote 数据面；文档工具仍走逐 Run registry。
    app: RwLock<Option<tauri::AppHandle>>,
    listening: AtomicBool,
}

impl BridgeHttpRuntime {
    pub fn new(bearer: String, base_url: String) -> Self {
        Self {
            bridge: SophonoteBridge::new(LeaseRegistry::new()),
            bearer,
            base_url,
            run_tools: Mutex::new(HashMap::new()),
            app: RwLock::new(None),
            listening: AtomicBool::new(true),
        }
    }

    pub fn install_app_handle(&self, app: tauri::AppHandle) {
        if let Ok(mut slot) = self.app.write() {
            *slot = Some(app);
        }
    }

    pub fn register_run(&self, lease: SidecarLease, tools: Arc<ToolRegistry>) {
        let id = lease.lease_id.clone();
        self.bridge.register_lease(lease);
        self.run_tools.lock().unwrap().insert(id, tools);
    }

    pub fn finish_run(&self, lease_id: &str) {
        self.bridge.revoke_lease(lease_id);
        self.run_tools.lock().unwrap().remove(lease_id);
    }

    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base_url.trim_end_matches('/'))
    }
}

static RUNTIME: OnceLock<Arc<BridgeHttpRuntime>> = OnceLock::new();

/// 开发附着默认 loopback 端口（稳定 URL，避免每次 SophoNote 重启换端口导致 Hermes 连死旧 MCP）
pub const ENV_BRIDGE_PORT: &str = "SOPHONOTE_BRIDGE_PORT";
pub const ENV_BRIDGE_BEARER: &str = "SOPHONOTE_BRIDGE_BEARER";
const DEFAULT_BRIDGE_PORT: u16 = 18765;

fn resolve_bridge_bearer() -> String {
    if let Ok(b) = std::env::var(ENV_BRIDGE_BEARER) {
        let t = b.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    // 落盘到 Hermes home（与 config.yaml 同目录），保证 SophoNote 重启后 Authorization 不变
    let home = crate::agent::hermes::hermes_home();
    if let Some(home) = home {
        let path = home.join("sophonote_bridge_bearer");
        if let Ok(existing) = std::fs::read_to_string(&path) {
            let t = existing.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        let fresh = format!("mb-bridge-{}", Uuid::new_v4().simple());
        let _ = std::fs::create_dir_all(&home);
        let _ = std::fs::write(&path, &fresh);
        return fresh;
    }
    format!("mb-bridge-{}", Uuid::new_v4().simple())
}

fn bind_bridge_listener() -> Result<(TcpListener, u16), String> {
    let preferred = std::env::var(ENV_BRIDGE_PORT)
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_BRIDGE_PORT);
    match TcpListener::bind(("127.0.0.1", preferred)) {
        Ok(l) => Ok((l, preferred)),
        Err(e) => {
            eprintln!("[sophonote-bridge] 绑定 127.0.0.1:{preferred} 失败（{e}），回退系统分配端口");
            let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind MCP: {e}"))?;
            let port = l
                .local_addr()
                .map_err(|e| format!("local_addr: {e}"))?
                .port();
            Ok((l, port))
        }
    }
}

/// 确保 loopback MCP 已监听；返回运行时句柄。
pub fn ensure_bridge_http() -> Result<Arc<BridgeHttpRuntime>, String> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt.clone());
    }
    let (listener, port) = bind_bridge_listener()?;
    let bearer = resolve_bridge_bearer();
    let base_url = format!("http://127.0.0.1:{port}");
    let rt = Arc::new(BridgeHttpRuntime::new(bearer, base_url));
    let _ = RUNTIME.set(rt.clone());

    let accept_rt = rt.clone();
    thread::Builder::new()
        .name("sophonote-mcp-bridge".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if !accept_rt.listening.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(s) => {
                        let rt = accept_rt.clone();
                        thread::spawn(move || {
                            if let Err(e) = handle_client(s, &rt) {
                                eprintln!("[sophonote-bridge] request error: {e}");
                            }
                        });
                    }
                    Err(e) => eprintln!("[sophonote-bridge] accept error: {e}"),
                }
            }
        })
        .map_err(|e| format!("spawn MCP thread: {e}"))?;

    eprintln!(
        "[sophonote-bridge] listening on {} (Hermes mcp url)",
        rt.mcp_url()
    );
    Ok(rt)
}

pub fn bridge_runtime() -> Option<Arc<BridgeHttpRuntime>> {
    RUNTIME.get().cloned()
}

fn handle_client(mut stream: TcpStream, rt: &BridgeHttpRuntime) -> std::io::Result<()> {
    let (method, path, headers, body) = read_http_request(&mut stream)?;
    let auth_ok = headers
        .get("authorization")
        .map(|a| a == &format!("Bearer {}", rt.bearer))
        .unwrap_or(false);

    // 健康/探针：无鉴权也可返回 MCP 友好 content-type
    if method == "HEAD" || method == "GET" {
        if path.starts_with("/mcp") || path == "/" {
            write_response(
                &mut stream,
                200,
                "application/json",
                r#"{"ok":true,"server":"sophonote-bridge"}"#,
            )?;
            return Ok(());
        }
        write_response(&mut stream, 404, "text/plain", "not found")?;
        return Ok(());
    }

    if method != "POST" {
        write_response(&mut stream, 405, "text/plain", "method not allowed")?;
        return Ok(());
    }

    if !path.starts_with("/mcp") && path != "/" {
        write_response(
            &mut stream,
            404,
            "application/json",
            r#"{"error":"not_found"}"#,
        )?;
        return Ok(());
    }

    if !auth_ok {
        write_response(
            &mut stream,
            401,
            "application/json",
            r#"{"jsonrpc":"2.0","error":{"code":-32001,"message":"unauthorized"},"id":null}"#,
        )?;
        return Ok(());
    }

    let req: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    // 批次数组
    if let Some(arr) = req.as_array() {
        let mut out = Vec::new();
        for item in arr {
            if let Some(resp) = handle_rpc(item, rt, &headers) {
                out.push(resp);
            }
        }
        write_response(
            &mut stream,
            200,
            "application/json",
            &serde_json::to_string(&out).unwrap_or_else(|_| "[]".into()),
        )?;
        return Ok(());
    }

    if let Some(resp) = handle_rpc(&req, rt, &headers) {
        write_response(
            &mut stream,
            200,
            "application/json",
            &serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into()),
        )?;
    } else {
        // notification：202 / 空 body
        write_response(&mut stream, 202, "application/json", "")?;
    }
    Ok(())
}

fn handle_rpc(
    req: &Value,
    rt: &BridgeHttpRuntime,
    headers: &HashMap<String, String>,
) -> Option<Value> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned();
    // notifications：无 id
    if id.is_none() || id.as_ref().is_some_and(|v| v.is_null()) {
        return None;
    }
    let id = id.unwrap();

    let result = match method {
        "initialize" => Some(json!({
            "protocolVersion": "2025-03-26",
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": BRIDGE_MCP_NAME, "version": "0.1.0" }
        })),
        "ping" => Some(json!({})),
        "tools/list" => Some(json!({ "tools": bridge_tool_defs() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(json!({}));
            Some(tools_call(rt, headers, &params))
        }
        "resources/list" => Some(json!({ "resources": [] })),
        "prompts/list" => Some(json!({ "prompts": [] })),
        other => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }));
        }
    };

    Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result.unwrap_or(json!({}))
    }))
}

fn bridge_tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_project_documents",
            "description": "列出当前项目下全部文档（标题与 articleId）。读正文前先调用本工具。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "leaseId": {
                        "type": "string",
                        "description": "SophoNote 为当前 Run 提供的内部权限凭据；必须原样传递，不得向用户展示"
                    }
                },
                "required": ["leaseId"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "read_document",
            "description": "读取本项目文档正文（SophoNote notes/<articleId>.md）。长文用 offset 分页；禁止 read_file/扫本机路径。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "leaseId": {
                        "type": "string",
                        "description": "SophoNote 为当前 Run 提供的内部权限凭据；必须原样传递，不得向用户展示"
                    },
                    "articleId": { "type": "string", "description": "文档 ID" },
                    "offset": { "type": "integer", "description": "字符偏移，续读用上次 nextOffset", "minimum": 0 },
                    "maxChars": { "type": "integer", "description": "本页最大字符数，默认 8000", "minimum": 500, "maximum": 16000 }
                },
                "required": ["leaseId", "articleId"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "propose_document_patch",
            "description": "对文档提出 dry-run 修改提案（不落盘）；用户批准后由 SophoNote DocumentService 写入。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "leaseId": {
                        "type": "string",
                        "description": "SophoNote 为当前 Run 提供的内部权限凭据；必须原样传递，不得向用户展示"
                    },
                    "articleId": { "type": "string" },
                    "baseVersion": { "type": "integer" },
                    "instruction": { "type": "string" },
                    "hunks": { "type": "array" }
                },
                "required": ["leaseId", "articleId"]
            },
            "annotations": { "readOnlyHint": false }
        }),
        json!({
            "name": "rename_article",
            "description": "对文档提出标题改名提案（dry-run，不落盘）；用户批准后由 SophoNote 执行完整改名（SQLite + frontmatter + 双链改写 + 索引重建）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "leaseId": {
                        "type": "string",
                        "description": "SophoNote 为当前 Run 提供的内部权限凭据；必须原样传递，不得向用户展示"
                    },
                    "articleId": { "type": "string", "description": "文档 ID" },
                    "newTitle": { "type": "string", "description": "新标题（非空、不含换行、≤ 200 字符）" }
                },
                "required": ["leaseId", "articleId", "newTitle"]
            },
            "annotations": { "readOnlyHint": false }
        }),
        json!({
            "name": "refresh_discovery_sources",
            "description": "刷新 SophoNote 发现/收件箱的固定信息源。仅支持 github、arxiv、hackernews、producthunt、huggingface、aihot；复用 Host 抓取、去重和健康记录，不接受 URL、路径或命令。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sources": {
                        "type": "array",
                        "description": "要刷新的发现类别；必须显式给出 1 至 6 个固定键",
                        "items": {
                            "type": "string",
                            "enum": ["github", "arxiv", "hackernews", "producthunt", "huggingface", "aihot"]
                        },
                        "minItems": 1,
                        "maxItems": 6,
                        "uniqueItems": true
                    }
                },
                "required": ["sources"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false }
        }),
        json!({
            "name": "read_discovery_item",
            "description": "读取 SophoNote 中已存在发现条目的元数据与证据。只接受 itemId；证据质量不足时拒绝，禁止用浏览器或文件系统绕过。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "itemId": { "type": "string", "minLength": 1, "maxLength": 256 }
                },
                "required": ["itemId"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "list_discovery_candidates",
            "description": "在生成前按已抓取元数据预筛固定发现类别，只返回小规模候选、精简评分规则和近期去重记录。此工具不读取完整正文、不生成速览或深度解读。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sources": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["github", "arxiv", "hackernews", "producthunt", "huggingface", "aihot"] },
                        "minItems": 1,
                        "maxItems": 6,
                        "uniqueItems": true
                    },
                    "limitPerSource": { "type": "integer", "minimum": 1, "maximum": 20, "default": 4 }
                },
                "required": ["sources"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "save_discovery_analysis",
            "description": "保存 Hermes 为现存发现条目生成的速览或深度解读。quick 使用结构化 quick 字段；deep 使用 markdown，并必须提交可信数据源规则中的 policyHash；Host 校验当前规则版本、编号章节、证据引用和长度后写入 SophoNote。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "itemId": { "type": "string", "minLength": 1, "maxLength": 256 },
                    "mode": { "type": "string", "enum": ["quick", "deep"] },
                    "quick": {
                        "type": "object",
                        "properties": {
                            "summary": { "type": "string" },
                            "whyImportant": { "type": "string" },
                            "keyPoints": { "type": "array" },
                            "risks": { "type": "array" },
                            "confidence": { "type": "string", "enum": ["high", "medium", "low"] },
                            "tags": { "type": "array" }
                        },
                        "required": ["summary", "whyImportant", "keyPoints", "risks", "confidence", "tags"]
                    },
                    "markdown": { "type": "string" },
                    "policyHash": {
                        "type": "string",
                        "pattern": "^sha256:[a-f0-9]{64}$",
                        "description": "mode=deep 时必填；来自 Skill 可信 source-policy reference 的当前规则哈希"
                    }
                },
                "required": ["itemId", "mode"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
        json!({
            "name": "save_discovery_pick",
            "description": "将已完成速览和深度解读的高分条目发布到今日发现。Host 强制来源最低分、近期同条目和 80% 标题近重复门禁；近期记录只用于去重，不设置每日数量配额。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "itemId": { "type": "string", "minLength": 1, "maxLength": 256 },
                    "lane": { "type": "string", "enum": ["github", "model", "product"] },
                    "score": { "type": "number", "minimum": 0, "maximum": 10 },
                    "reason": { "type": "string", "minLength": 1, "maxLength": 160 }
                },
                "required": ["itemId", "lane", "score", "reason"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
        json!({
            "name": "save_discovery_scores",
            "description": "批量持久化一轮打分的全部产物（NEXT-048 五断面数据面）：score 0-10（落库保留一位小数）、aspect 仅限 模型/产品/行业/论文/观点 或 null、aiTopics 最多 3 个受控主题、reason ≤160 字。条目必须已存在（打分不创建条目）；单条结构错误只拒该条并记入 rejected，不中断整批。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scores": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 200,
                        "items": {
                            "type": "object",
                            "properties": {
                                "itemId": { "type": "string", "minLength": 1, "maxLength": 256 },
                                "score": { "type": "number", "minimum": 0, "maximum": 10 },
                                "aspect": { "type": ["string", "null"], "enum": ["模型", "产品", "行业", "论文", "观点", null] },
                                "aiTopics": {
                                    "type": "array",
                                    "maxItems": 3,
                                    "items": { "type": "string", "minLength": 1, "maxLength": 60 }
                                },
                                "reason": { "type": "string", "maxLength": 160 }
                            },
                            "required": ["itemId", "score"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["scores"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
        json!({
            "name": "read_discovery_feed",
            "description": "读取已持久化的发现评分与分析快照，供日报、周报、月报、模型榜与深度解读补全使用。只读且不抓取、不打分。默认只返回已保存 deep 的条目；missingDeep=true 仅供补全任务读取尚无 deep 的条目。可传闭开区间 [fromDate,toDate)，或传 period + 可选 date 由 Host 按本地日期解析窗口并返回 periodKey。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fromDate": { "type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" },
                    "toDate": { "type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" },
                    "period": { "type": "string", "enum": ["daily", "weekly", "monthly", "rolling7"] },
                    "date": { "type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" },
                    "minScore": { "type": "number", "minimum": 0, "maximum": 10 },
                    "missingDeep": { "type": "boolean" },
                    "allTime": { "type": "boolean" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false }
        }),
        json!({
            "name": "save_discovery_report",
            "description": "保存 AI 雷达生成的日/周/月报告。正文必须是完整中文 Markdown；同 period+periodKey 幂等覆盖。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "period": { "type": "string", "enum": ["daily", "weekly", "monthly"] },
                    "periodKey": { "type": "string", "minLength": 1, "maxLength": 32 },
                    "title": { "type": "string", "minLength": 1, "maxLength": 200 },
                    "markdown": { "type": "string", "minLength": 80, "maxLength": 100000 },
                    "stats": { "type": "object" }
                },
                "required": ["period", "periodKey", "title", "markdown"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
        json!({
            "name": "refresh_openrouter_rankings",
            "description": "从 OpenRouter 官方结构化 API 刷新完整模型榜快照。API Key 由 Rust 从 Keychain 读取；五个数据集全部成功才原子替换，失败保留旧快照。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }),
        json!({
            "name": "read_openrouter_rankings",
            "description": "读取最近一次成功的 OpenRouter 模型榜快照元数据与各分区数量，不抓网、不返回 API Key。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "destructiveHint": false }
        }),
    ]
}

fn tools_call(rt: &BridgeHttpRuntime, headers: &HashMap<String, String>, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    // Hermes 可能加前缀 sophonote-bridge__ 或 mcp_sophonote-bridge_
    let tool_name = strip_mcp_prefix(&name);
    let mut arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    if !arguments.is_object() {
        arguments = json!({});
    }

    // 发现能力只依赖 MCP 环回 Bearer，不复用项目文档短租约。参数限制在固定
    // 来源或已存在 itemId，因此 Cron/卡片 Run 可工作，却不会获得任意路径、
    // URL、命令或项目文档权限。
    match tool_name.as_str() {
        "refresh_discovery_sources" => return refresh_discovery_sources(rt, &arguments),
        "list_discovery_candidates" => return list_discovery_candidates(rt, &arguments),
        "read_discovery_item" => return read_discovery_item(rt, &arguments),
        "save_discovery_analysis" => return save_discovery_analysis(rt, &arguments),
        "save_discovery_pick" => return save_discovery_pick(rt, &arguments),
        "save_discovery_scores" => return save_discovery_scores(rt, &arguments),
        "read_discovery_feed" => return read_discovery_feed(rt, &arguments),
        "save_discovery_report" => return save_discovery_report(rt, &arguments),
        "refresh_openrouter_rankings" => return refresh_openrouter_rankings(rt),
        "read_openrouter_rankings" => return read_openrouter_rankings(rt),
        _ => {}
    }

    let lease_id = headers
        .get(HEADER_LEASE)
        .cloned()
        .or_else(|| {
            arguments
                .get("leaseId")
                .or_else(|| arguments.get("lease_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    if lease_id.is_empty() {
        return tool_error("缺少 lease_id（Header 或参数）");
    }

    let tools = {
        let map = rt.run_tools.lock().unwrap();
        map.get(&lease_id).cloned()
    };
    let Some(tools) = tools else {
        return tool_error("lease 无绑定 ToolRegistry 或已结束");
    };

    // 从 arguments 移除 lease 字段，避免工具 schema 校验失败
    if let Some(obj) = arguments.as_object_mut() {
        obj.remove("leaseId");
        obj.remove("lease_id");
    }

    let claimed_project = arguments
        .get("projectId")
        .or_else(|| arguments.get("project_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let claimed_run = arguments
        .get("runId")
        .or_else(|| arguments.get("run_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let req = BridgeInvokeRequest {
        lease_id,
        tool_name: tool_name.clone(),
        arguments,
        claimed_project_id: claimed_project,
        claimed_run_id: claimed_run,
    };

    // 同步桥接异步：当前 HTTP 线程内 block_on
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(rt.bridge.invoke_with_tools(req, tools.as_ref())));

    match result {
        Ok(Ok(out)) => {
            if out.ok {
                json!({
                    "content": [{ "type": "text", "text": out.output_text }],
                    "isError": false
                })
            } else {
                tool_error(out.error.unwrap_or_else(|| "tool failed".into()))
            }
        }
        Ok(Err(e)) => tool_error(e.to_string()),
        Err(e) => tool_error(format!("runtime: {e}")),
    }
}

fn discovery_source_ids(arguments: &Value) -> Result<Vec<String>, String> {
    let sources = arguments
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| "sources 必须是包含 1 至 6 个固定类别的数组".to_string())?;
    if sources.is_empty() || sources.len() > 6 {
        return Err("sources 必须包含 1 至 6 个固定类别".into());
    }

    let mut ids = Vec::new();
    for source in sources {
        let key = source
            .as_str()
            .ok_or_else(|| "sources 只能包含字符串".to_string())?;
        let mapped: &[&str] = match key {
            "github" => &["github-trending"],
            "arxiv" => &["arxiv-ai"],
            "hackernews" => &["hackernews"],
            "producthunt" => &["producthunt"],
            "huggingface" => &["huggingface-models", "huggingface-papers"],
            "aihot" => &["aihot"],
            _ => return Err(format!("不支持的发现类别: {key}")),
        };
        for id in mapped {
            if !ids.iter().any(|existing| existing == id) {
                ids.push((*id).to_string());
            }
        }
    }
    Ok(ids)
}

fn refresh_discovery_sources(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let source_ids = match discovery_source_ids(arguments) {
        Ok(ids) => ids,
        Err(error) => return tool_error(error),
    };
    let app = rt.app.read().ok().and_then(|slot| slot.clone());
    let Some(app) = app else {
        return tool_error("SophoNote Host 尚未完成发现自动化初始化");
    };

    let requested_source_ids = source_ids.clone();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| {
            runtime.block_on(async {
                // 这里只抓取并规范化来源元数据。正文/证据延迟到候选通过预筛且
                // 被 Agent 选中后，由 read_discovery_item 按单条准备，避免为每个
                // 原始条目提前装载大段正文。
                crate::scheduler::fetch_sources(&app, Some(source_ids)).await
            })
        });
    match result {
        Ok(mut results) => {
            // scheduler 只抓取当前启用且未跳过的源。对显式请求但未返回的来源补出
            // 可见失败，避免 ProductHunt 等默认关闭来源被误报为“全部成功”。
            for source_id in requested_source_ids {
                if !results.iter().any(|item| item.source_id == source_id) {
                    results.push(crate::scheduler::SourceFetchResult {
                        source_id,
                        success: false,
                        fetched: 0,
                        new_items: 0,
                        new_item_ids: Vec::new(),
                        error: Some(
                            "来源未启用或已被数据源准入策略跳过，请到设置 → 数据源检查".into(),
                        ),
                    });
                }
            }
            let success = results.iter().filter(|item| item.success).count();
            let failed = results.len().saturating_sub(success);
            let output = json!({
                "success": failed == 0,
                "summary": format!("发现源刷新完成：成功 {success}，失败 {failed}"),
                "preparedItems": 0,
                "evidencePreparation": "deferred_until_selected",
                "results": results,
            });
            json!({
                "content": [{ "type": "text", "text": output.to_string() }],
                "isError": false
            })
        }
        Err(error) => tool_error(format!("启动发现刷新运行时失败: {error}")),
    }
}

fn default_discovery_policy(source_id: &str) -> (&'static str, &'static str) {
    match source_id {
        "github-trending" => (
            "从技术架构、扩展点、工程成熟度、维护活跃度与真实使用价值解释仓库。",
            "架构或工程创新 35%，可验证采用/热度 25%，维护质量 20%，时效性 20%；纯合集、镜像、营销仓库降分。",
        ),
        "arxiv-ai" | "huggingface-papers" => (
            "从 Research 视角解释方法创新、实验设计、基线对比、可复现性与对模型演进的意义。",
            "研究新颖性 35%，实验可信度 30%，影响潜力 20%，证据完整度 15%；缺实验或仅包装旧方法降分。",
        ),
        "hackernews" => (
            "先判断属于模型研究还是产品信号，再从原创信息、证据质量和高价值讨论中提炼结论。",
            "信息增量 30%，讨论质量 25%，证据与来源 25%，时效性 20%；标题党、重复新闻、低信息评论降分。",
        ),
        "producthunt" => (
            "从产品定位、目标用户、核心工作流、差异化、早期采用信号与商业可行性解释产品。",
            "用户问题与定位 30%，产品差异化 25%，采用信号 20%，完成度 15%，时效性 10%；包装站和低完成度 Demo 降分。",
        ),
        "huggingface-models" => (
            "从模型能力、评测证据、训练/部署成本、License、适用边界和 Research 价值解释。",
            "能力或研究增量 30%，评测证据 25%，可用性 20%，采用信号 15%，时效性 10%；缺 Model Card 或不可验证声明降分。",
        ),
        "aihot" => (
            "从中文 AI 资讯视角解释：事件本身、关键主体与出处、行业影响与后续看点；只基于条目证据，不臆测未披露细节。",
            "信息增量 30%，出处与证据质量 25%，行业影响 25%，时效性 20%；二手转述、软文与低信息量合集降分。",
        ),
        _ => (
            "基于来源正文与证据解释信息价值、局限与建议行动。",
            "信息增量、证据质量、相关性与时效性综合评分。",
        ),
    }
}

fn discovery_policy(source_id: &str, raw_config: Option<&str>) -> Value {
    let config = raw_config
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let (default_prompt, default_rule) = default_discovery_policy(source_id);
    let generation_prompt = config
        .get("generationPrompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_prompt);
    let scoring_rule = config
        .get("scoringRule")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_rule);
    let min_score = config
        .get("minScore")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (0.0..=10.0).contains(value))
        .unwrap_or(8.0);
    let generation_prompt = truncate_chars(generation_prompt, 4_000);
    let scoring_rule = truncate_chars(scoring_rule, 2_000);
    let policy_hash = discovery_policy_hash(&generation_prompt, &scoring_rule, min_score);
    json!({
        "sourceId": source_id,
        "generationPrompt": generation_prompt,
        "scoringRule": scoring_rule,
        "minScore": min_score,
        "policyHash": policy_hash,
        "requiredHeadings": required_numbered_headings(&generation_prompt),
    })
}

fn discovery_policy_hash(generation_prompt: &str, scoring_rule: &str, min_score: f64) -> String {
    let mut digest = Sha256::new();
    digest.update(b"sophonote-discovery-policy-v2\0");
    digest.update(generation_prompt.as_bytes());
    digest.update(b"\0");
    digest.update(scoring_rule.as_bytes());
    digest.update(b"\0");
    digest.update(format!("{min_score:.4}").as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

/// 用户 Prompt 中的二级编号标题是深度解读的可验证输出契约。只提取
/// `## 1. ...` / `## 1、...`，不会把“输出要求”等说明标题误当正文结构。
fn required_numbered_headings(prompt: &str) -> Vec<String> {
    prompt
        .lines()
        .filter_map(|line| {
            let heading = line.trim().strip_prefix("## ")?.trim();
            let digit_count = heading.chars().take_while(char::is_ascii_digit).count();
            if digit_count == 0 {
                return None;
            }
            let remainder = heading.chars().skip(digit_count).collect::<String>();
            if !remainder.starts_with('.') && !remainder.starts_with('、') {
                return None;
            }
            Some(normalize_heading(heading))
        })
        .collect()
}

fn normalize_heading(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn discovery_policy_reference_filenames(source_id: &str) -> Option<(&'static str, &'static str)> {
    match source_id {
        "github-trending" => Some((
            "github-trending-scoring.md",
            "github-trending-generation.md",
        )),
        "arxiv-ai" => Some(("arxiv-ai-scoring.md", "arxiv-ai-generation.md")),
        "hackernews" => Some(("hackernews-scoring.md", "hackernews-generation.md")),
        "producthunt" => Some(("producthunt-scoring.md", "producthunt-generation.md")),
        "huggingface-models" => Some((
            "huggingface-models-scoring.md",
            "huggingface-models-generation.md",
        )),
        "huggingface-papers" => Some((
            "huggingface-papers-scoring.md",
            "huggingface-papers-generation.md",
        )),
        "aihot" => Some(("aihot-scoring.md", "aihot-generation.md")),
        _ => None,
    }
}

fn render_discovery_scoring_reference(policy: &Value) -> Result<String, String> {
    let source_id = policy["sourceId"]
        .as_str()
        .ok_or_else(|| "发现策略缺少 sourceId".to_string())?;
    let policy_hash = policy["policyHash"]
        .as_str()
        .ok_or_else(|| "发现策略缺少 policyHash".to_string())?;
    let scoring_rule = policy["scoringRule"]
        .as_str()
        .ok_or_else(|| "发现策略缺少 scoringRule".to_string())?;
    let min_score = policy["minScore"]
        .as_f64()
        .ok_or_else(|| "发现策略缺少 minScore".to_string())?;
    Ok(format!(
        "# SophoNote 可信评分规则：{source_id}\n\n\
此文件由 SophoNote 根据“设置 → 数据源 → AI 筛选与生成规则”生成，\
只用于生成前的低成本候选筛选。\n\n\
- Source ID: `{source_id}`\n\
- Policy Hash: `{policy_hash}`\n\
- Minimum Score: `{min_score}`\n\n\
## 过滤评分规则\n\n{scoring_rule}\n"
    ))
}

fn render_discovery_generation_reference(policy: &Value) -> Result<String, String> {
    let source_id = policy["sourceId"]
        .as_str()
        .ok_or_else(|| "发现策略缺少 sourceId".to_string())?;
    let policy_hash = policy["policyHash"]
        .as_str()
        .ok_or_else(|| "发现策略缺少 policyHash".to_string())?;
    let generation_prompt = policy["generationPrompt"]
        .as_str()
        .ok_or_else(|| "发现策略缺少 generationPrompt".to_string())?;
    let headings = policy["requiredHeadings"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(|heading| format!("- `{heading}`"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    Ok(format!(
        "# SophoNote 可信深度生成规则：{source_id}\n\n\
此文件由 SophoNote 根据“设置 → 数据源 → AI 筛选与生成规则”生成。\
它是当前数据源深度解读的权威指令，优先于主 Skill 中的通用措辞。\n\n\
- Source ID: `{source_id}`\n\
- Policy Hash: `{policy_hash}`\n\n\
## 深度解读 Prompt\n\n{generation_prompt}\n\n\
## 保存契约\n\n\
调用 `save_discovery_analysis` 保存 `mode=deep` 时，必须传入 \
`policyHash=\"{policy_hash}\"`。正文必须按 Prompt 的编号二级标题输出，\
标题名称和顺序不得改写。缺失证据使用“未披露”或“未验证”，不得删掉章节。\n\n\
当前必须出现的编号标题：\n{headings}\n"
    ))
}

fn write_discovery_policy_reference(source_id: &str, policy: &Value) -> Result<(), String> {
    let (scoring_filename, generation_filename) =
        discovery_policy_reference_filenames(source_id)
            .ok_or_else(|| format!("不支持的数据源策略: {source_id}"))?;
    let home = crate::agent::hermes::hermes_home()
        .ok_or_else(|| "Hermes 私有 Home 尚未就绪".to_string())?;
    let directory = home.join("skills/productivity/sophonote-ai-radar/references/source-policies");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("创建 Hermes 策略目录失败: {error}"))?;
    std::fs::write(
        directory.join(scoring_filename),
        render_discovery_scoring_reference(policy)?,
    )
    .map_err(|error| format!("同步 Hermes 数据源评分规则失败: {error}"))?;
    std::fs::write(
        directory.join(generation_filename),
        render_discovery_generation_reference(policy)?,
    )
    .map_err(|error| format!("同步 Hermes 数据源生成 Prompt 失败: {error}"))
}

/// 将 DB 中用户保存的规则同步为 Hermes Skill 的可信按需引用。外部正文仍通过
/// MCP 的 untrusted tool result 进入，二者不会混入同一信任域。
pub fn sync_discovery_policy_references(app: &tauri::AppHandle) -> Result<usize, String> {
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("读取发现策略失败: {error}"))?;
    let source_ids = [
        "github-trending",
        "arxiv-ai",
        "hackernews",
        "producthunt",
        "huggingface-models",
        "huggingface-papers",
        "aihot",
    ];
    let mut synced = 0;
    for source_id in source_ids {
        let config = conn
            .query_row(
                "SELECT config FROM sources WHERE id = ?1",
                rusqlite::params![source_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap_or(None);
        let policy = discovery_policy(source_id, config.as_deref());
        write_discovery_policy_reference(source_id, &policy)?;
        synced += 1;
    }
    Ok(synced)
}

pub fn sync_discovery_policy_reference(
    app: &tauri::AppHandle,
    source_id: &str,
) -> Result<(), String> {
    if discovery_policy_reference_filenames(source_id).is_none() {
        return Err(format!("不支持的数据源策略: {source_id}"));
    }
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("读取发现策略失败: {error}"))?;
    let config = conn
        .query_row(
            "SELECT config FROM sources WHERE id = ?1",
            rusqlite::params![source_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|error| format!("读取数据源策略失败: {error}"))?;
    write_discovery_policy_reference(source_id, &discovery_policy(source_id, config.as_deref()))
}

fn source_category(source_id: &str) -> Option<&'static str> {
    match source_id {
        "github-trending" => Some("github"),
        "arxiv-ai" => Some("arxiv"),
        "hackernews" => Some("hackernews"),
        "producthunt" => Some("producthunt"),
        "huggingface-models" | "huggingface-papers" => Some("huggingface"),
        "aihot" => Some("aihot"),
        _ => None,
    }
}

fn source_allows_lane(source_id: &str, lane: &str) -> bool {
    match source_id {
        "github-trending" => lane == "github",
        "arxiv-ai" | "huggingface-models" | "huggingface-papers" => lane == "model",
        "producthunt" => lane == "product",
        "hackernews" => matches!(lane, "model" | "product"),
        // aihot 为中文策展池，条目按 category 归入 model（ai-models/paper）或 product（ai-products/industry/tip）lane
        "aihot" => matches!(lane, "model" | "product"),
        _ => false,
    }
}

fn list_discovery_candidates(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result = (|| {
        let app = discovery_app(rt)?;
        let source_ids = discovery_source_ids(arguments)?;
        let limit = arguments
            .get("limitPerSource")
            .and_then(Value::as_i64)
            .unwrap_or(4);
        if !(1..=20).contains(&limit) {
            return Err("limitPerSource 必须在 1 到 20 之间".into());
        }
        let conn = rusqlite::Connection::open(crate::db::get_db_path(&app))
            .map_err(|error| format!("读取发现候选失败: {error}"))?;
        let mut policies = Vec::new();
        let mut candidates = Vec::new();
        for source_id in source_ids {
            let source = conn.query_row(
                "SELECT name, config, enabled, admission FROM sources WHERE id = ?1",
                rusqlite::params![source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            );
            let (source_name, config, enabled, admission) = match source {
                Ok(value) => value,
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(error) => return Err(format!("读取数据源策略失败: {error}")),
            };
            if !enabled || admission == "skipped" {
                continue;
            }
            let policy = discovery_policy(&source_id, config.as_deref());
            policies.push(json!({
                "sourceId": policy["sourceId"],
                "scoringRule": truncate_chars(
                    policy["scoringRule"].as_str().unwrap_or_default(),
                    800,
                ),
                "minScore": policy["minScore"],
                "policyHash": policy["policyHash"],
            }));
            let mut stmt = conn
                .prepare(
                    "SELECT i.id, i.item_type, i.title, i.url, i.description, i.author,
                            i.stars, i.published_at, i.fetched_at,
                            COALESCE(c.quality_level, 0), c.excerpt,
                            CASE WHEN c.status IN ('ready', 'partial') AND c.quality_level >= 2
                                 THEN 1 ELSE 0 END
                     FROM items i LEFT JOIN item_contents c ON c.item_id = i.id
                     WHERE i.source_id = ?1 AND i.status != 'archived'
                       AND trim(i.title) != ''
                       AND datetime(i.fetched_at) >= datetime('now', '-7 day')
                       AND NOT EXISTS (
                           SELECT 1 FROM daily_picks p
                           WHERE p.item_id = i.id
                             AND date(p.date) >= date('now', '-7 day')
                       )
                     ORDER BY COALESCE(i.stars, 0) DESC, i.fetched_at DESC
                     LIMIT ?2",
                )
                .map_err(|error| format!("读取发现候选失败: {error}"))?;
            let rows = stmt
                .query_map(rusqlite::params![source_id, limit], |row| {
                    Ok(json!({
                        "itemId": row.get::<_, String>(0)?,
                        "sourceId": source_id,
                        "sourceName": source_name,
                        "category": source_category(&source_id),
                        "type": row.get::<_, String>(1)?,
                        "title": row.get::<_, String>(2)?,
                        "url": row.get::<_, Option<String>>(3)?,
                        "description": truncate_chars(&row.get::<_, Option<String>>(4)?.unwrap_or_default(), 300),
                        "author": row.get::<_, Option<String>>(5)?,
                        "heat": row.get::<_, Option<i64>>(6)?,
                        "publishedAt": row.get::<_, Option<String>>(7)?,
                        "fetchedAt": row.get::<_, String>(8)?,
                        "qualityLevel": row.get::<_, i64>(9)?,
                        "excerpt": truncate_chars(&row.get::<_, Option<String>>(10)?.unwrap_or_default(), 500),
                        "evidencePrepared": row.get::<_, bool>(11)?,
                    }))
                })
                .map_err(|error| format!("读取发现候选失败: {error}"))?;
            candidates.extend(rows.filter_map(Result::ok));
        }

        let mut recent = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT p.item_id, p.date, p.selection_lane, p.ai_score, p.reason,
                        i.source_id, i.title
                 FROM daily_picks p JOIN items i ON i.id = p.item_id
                 WHERE date(p.date) >= date('now', '-7 day')
                 ORDER BY p.date DESC, p.rank ASC LIMIT 60",
            )
            .map_err(|error| format!("读取近期发现失败: {error}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(json!({
                    "itemId": row.get::<_, String>(0)?,
                    "date": row.get::<_, String>(1)?,
                    "lane": row.get::<_, Option<String>>(2)?,
                    "score": row.get::<_, Option<f64>>(3)?,
                    "reason": row.get::<_, Option<String>>(4)?,
                    "sourceId": row.get::<_, String>(5)?,
                    "title": row.get::<_, String>(6)?,
                }))
            })
            .map_err(|error| format!("读取近期发现失败: {error}"))?;
        recent.extend(rows.filter_map(Result::ok));
        Ok(json!({
            "candidates": candidates,
            "sourcePolicies": policies,
            "recentSelections": recent,
            "stage": "metadata_prefilter",
            "rules": {
                "defaultMinScore": 8,
                "publishAllQualified": true,
                "dailyQuota": null,
                "maxSimilarity": 0.8,
                "limitPerSource": limit,
                "fullEvidenceDeferred": true
            }
        }))
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

struct DiscoveryItemContext {
    title: String,
    evidence_ids: HashSet<String>,
    payload: Value,
}

fn discovery_item_id(arguments: &Value) -> Result<String, String> {
    let item_id = arguments
        .get("itemId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "itemId 不能为空".to_string())?;
    if item_id.chars().count() > 256 || item_id.chars().any(char::is_control) {
        return Err("itemId 无效".into());
    }
    Ok(item_id.to_string())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// AIHOT 官方 items API 已把策展摘要写入 `items.description`。该源没有可供
/// `content` 模块再次抓取的正文端点时，摘要就是本轮候选快照中唯一、已取得的
/// 证据；只允许作为该来源的受限降级，不把“unsupported”放宽为其它来源可生成。
fn aihot_snapshot_evidence(description: Option<&str>) -> Option<Value> {
    let text = description?.trim();
    if text.is_empty() {
        return None;
    }
    Some(json!([{
        "id": "E1",
        "kind": "candidate_snapshot",
        "title": "AIHOT 候选快照",
        "url": "",
        "text": truncate_chars(text, 8_000),
    }]))
}

fn load_discovery_item_context(
    app: &tauri::AppHandle,
    item_id: &str,
) -> Result<DiscoveryItemContext, String> {
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("读取发现条目失败: {error}"))?;
    type Row = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
    );
    let row: Row = conn
        .query_row(
            "SELECT i.source_id, i.item_type, i.title, i.url, i.description, i.author, i.language, i.stars, i.forks, i.topics, i.published_at, COALESCE(c.status, 'pending'), c.excerpt, c.evidence_json, c.content_type, COALESCE(c.quality_level, 0), c.content_hash FROM items i LEFT JOIN item_contents c ON c.item_id = i.id WHERE i.id = ?1",
            rusqlite::params![item_id],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?,
                row.get(15)?, row.get(16)?,
            )),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => "发现条目不存在或已删除".to_string(),
            other => format!("读取发现条目失败: {other}"),
        })?;
    let (
        source_id,
        item_type,
        title,
        url,
        description,
        author,
        language,
        stars,
        forks,
        topics,
        published_at,
        content_status,
        excerpt,
        evidence_json,
        content_type,
        quality_level,
        content_hash,
    ) = row;
    let use_aihot_snapshot =
        source_id == "aihot" && (content_status == "unsupported" || quality_level < 2);
    if content_status == "unsupported" && !use_aihot_snapshot {
        return Err("该来源不支持证据化解读".into());
    }
    if quality_level < 2 && !use_aihot_snapshot {
        return Err(format!(
            "正文证据不足（qualityLevel={quality_level}），暂不生成解读"
        ));
    }
    let raw_evidence = if use_aihot_snapshot {
        None
    } else {
        evidence_json.as_deref()
    };
    let mut evidence: Value = match raw_evidence {
        Some(raw) => {
            serde_json::from_str(raw).map_err(|_| "发现条目 evidence 数据损坏".to_string())?
        }
        None if use_aihot_snapshot => aihot_snapshot_evidence(description.as_deref())
            .ok_or_else(|| "AIHOT 候选快照缺少可用描述，暂不生成解读".to_string())?,
        None => return Err("发现条目缺少 evidence".into()),
    };
    let entries = evidence
        .as_array_mut()
        .ok_or_else(|| "发现条目 evidence 格式无效".to_string())?;
    if entries.is_empty() {
        return Err("发现条目 evidence 为空".into());
    }
    let mut evidence_ids = HashSet::new();
    let mut remaining = 24_000usize;
    for (index, entry) in entries.iter_mut().enumerate() {
        let object = entry
            .as_object_mut()
            .ok_or_else(|| "发现条目 evidence 条目格式无效".to_string())?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("E{}", index + 1));
        evidence_ids.insert(id.clone());
        object.insert("id".into(), Value::String(id));
        if let Some(text) = object.get("text").and_then(Value::as_str) {
            let limit = remaining.min(8_000);
            let bounded = truncate_chars(text, limit);
            remaining = remaining.saturating_sub(bounded.chars().count());
            object.insert("text".into(), Value::String(bounded));
        }
    }
    let source_config = conn
        .query_row(
            "SELECT config FROM sources WHERE id = ?1",
            rusqlite::params![source_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or(None);
    let policy = discovery_policy(&source_id, source_config.as_deref());
    Ok(DiscoveryItemContext {
        title: title.clone(),
        evidence_ids,
        payload: json!({
            "itemId": item_id,
            "sourceId": source_id,
            "type": item_type,
            "title": title,
            "url": url,
            "description": description,
            "author": author,
            "language": language,
            "stars": stars,
            "forks": forks,
            "topics": topics,
            "publishedAt": published_at,
            "contentStatus": content_status,
            "contentType": content_type,
            "qualityLevel": quality_level,
            "contentHash": content_hash,
            "evidenceOrigin": if use_aihot_snapshot { "candidate_snapshot" } else { "content" },
            "sourcePolicy": policy,
            "excerpt": excerpt.map(|value| truncate_chars(&value, 4_000)),
            "evidence": evidence,
        }),
    })
}

fn discovery_app(rt: &BridgeHttpRuntime) -> Result<tauri::AppHandle, String> {
    rt.app
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or_else(|| "SophoNote Host 尚未完成发现能力初始化".to_string())
}

fn tool_json(value: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": value.to_string() }],
        "isError": false
    })
}

fn read_discovery_item(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result = (|| {
        let app = discovery_app(rt)?;
        let item_id = discovery_item_id(arguments)?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("准备发现证据运行时失败: {error}"))?
            .block_on(crate::content::get_or_fetch_item_content(&app, &item_id))
            .map_err(|error| format!("准备入选条目证据失败: {error}"))?;
        load_discovery_item_context(&app, &item_id).map(|context| context.payload)
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

fn bounded_text(value: &Value, field: &str, max_chars: usize) -> Result<String, String> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| format!("{field} 不能为空"))?;
    let count = text.chars().count();
    if count == 0 || count > max_chars || text.chars().any(char::is_control) {
        return Err(format!("{field} 长度或内容无效"));
    }
    Ok(text.to_string())
}

fn bounded_markdown(value: &Value, field: &str, max_chars: usize) -> Result<String, String> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| format!("{field} 不能为空"))?;
    let count = text.chars().count();
    if count == 0
        || count > max_chars
        || text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return Err(format!("{field} 长度或内容无效"));
    }
    Ok(text.to_string())
}

fn cited_evidence_ids(text: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find(']') else {
            break;
        };
        let candidate = &rest[..end];
        if candidate.len() >= 2
            && candidate.starts_with('E')
            && candidate[1..].bytes().all(|byte| byte.is_ascii_digit())
        {
            result.insert(candidate.to_string());
        }
        rest = &rest[end + 1..];
    }
    result
}

fn validate_evidence_citations(text: &str, available: &HashSet<String>) -> Result<(), String> {
    let cited = cited_evidence_ids(text);
    if cited.is_empty() {
        return Err("解读至少需要一个 [Ex] 证据引用".into());
    }
    if let Some(invalid) = cited.iter().find(|id| !available.contains(*id)) {
        return Err(format!("解读引用了不存在的证据 [{invalid}]"));
    }
    Ok(())
}

fn save_quick_analysis(
    app: &tauri::AppHandle,
    item_id: &str,
    context: &DiscoveryItemContext,
    arguments: &Value,
) -> Result<Value, String> {
    let quick = arguments
        .get("quick")
        .and_then(Value::as_object)
        .ok_or_else(|| "quick 模式必须提供 quick 对象".to_string())?;
    let quick_value = Value::Object(quick.clone());
    let summary = bounded_text(&quick_value, "summary", 240)?;
    let why_important = bounded_text(&quick_value, "whyImportant", 240)?;
    let confidence = bounded_text(&quick_value, "confidence", 16)?;
    if !matches!(confidence.as_str(), "high" | "medium" | "low") {
        return Err("confidence 只能是 high、medium 或 low".into());
    }
    let key_points = quick_value
        .get("keyPoints")
        .and_then(Value::as_array)
        .ok_or_else(|| "keyPoints 必须是数组".to_string())?;
    if key_points.is_empty() || key_points.len() > 6 {
        return Err("keyPoints 必须包含 1 至 6 项".into());
    }
    let risks = quick_value
        .get("risks")
        .and_then(Value::as_array)
        .ok_or_else(|| "risks 必须是数组".to_string())?;
    if risks.len() > 6 {
        return Err("risks 最多 6 项".into());
    }
    let tags = quick_value
        .get("tags")
        .and_then(Value::as_array)
        .ok_or_else(|| "tags 必须是数组".to_string())?;
    if tags.is_empty() || tags.len() > 8 {
        return Err("tags 必须包含 1 至 8 项".into());
    }
    let mut cited_text = format!("{summary}\n{why_important}");
    let mut summary_lines = vec![summary.clone()];
    for point in key_points {
        let text = bounded_text(point, "text", 280)?;
        let refs = point
            .get("evidence")
            .and_then(Value::as_array)
            .ok_or_else(|| "keyPoints.evidence 必须是数组".to_string())?;
        if refs.is_empty()
            || refs.iter().any(|reference| {
                reference
                    .as_str()
                    .is_none_or(|id| !context.evidence_ids.contains(id))
            })
        {
            return Err("keyPoints.evidence 必须引用现存证据".into());
        }
        cited_text.push('\n');
        cited_text.push_str(&text);
        summary_lines.push(format!("✨ {text}"));
    }
    for risk in risks {
        let text = risk
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "risks 只能包含非空字符串".to_string())?;
        if text.chars().count() > 240 {
            return Err("单条 risk 不能超过 240 字符".into());
        }
        cited_text.push('\n');
        cited_text.push_str(text);
        summary_lines.push(format!("⚠️ {text}"));
    }
    validate_evidence_citations(&cited_text, &context.evidence_ids)?;
    let tag_values = tags
        .iter()
        .map(|tag| {
            tag.as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.chars().count() <= 40)
                .map(str::to_string)
                .ok_or_else(|| "tags 只能包含 1 至 40 字符的字符串".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let confidence_label = match confidence.as_str() {
        "high" => "高",
        "medium" => "中",
        _ => "低",
    };
    summary_lines.push(format!("💡 {why_important} · 可信度：{confidence_label}"));
    let rendered = summary_lines.join("\n");
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("保存速览失败: {error}"))?;
    let changed = conn
        .execute(
            "UPDATE items SET ai_summary = ?2, ai_tags = ?3, ai_prompt_version = 'hermes-discovery@v1', ai_enrich_json = ?4 WHERE id = ?1",
            rusqlite::params![
                item_id,
                rendered,
                tag_values.join(","),
                serde_json::to_string(&quick_value).map_err(|error| error.to_string())?,
            ],
        )
        .map_err(|error| format!("保存速览失败: {error}"))?;
    if changed != 1 {
        return Err("发现条目不存在或已删除".into());
    }
    Ok(json!({"success": true, "mode": "quick", "itemId": item_id}))
}

fn save_deep_analysis(
    app: &tauri::AppHandle,
    item_id: &str,
    context: &DiscoveryItemContext,
    arguments: &Value,
) -> Result<Value, String> {
    let markdown = bounded_markdown(arguments, "markdown", 30_000)?;
    let policy = context
        .payload
        .get("sourcePolicy")
        .ok_or_else(|| "发现条目缺少当前数据源规则".to_string())?;
    let current_policy_hash = policy
        .get("policyHash")
        .and_then(Value::as_str)
        .ok_or_else(|| "当前数据源规则缺少 policyHash".to_string())?;
    let submitted_policy_hash = arguments
        .get("policyHash")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "深度解读必须使用 Skill 中该数据源的可信 Prompt，并提交 policyHash".to_string()
        })?;
    if submitted_policy_hash != current_policy_hash {
        return Err("数据源生成规则已更新，请重新加载 source-policy 后再生成".into());
    }
    let required_headings = policy
        .get("requiredHeadings")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(normalize_heading)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    validate_required_headings(&markdown, &required_headings)?;
    validate_evidence_citations(&markdown, &context.evidence_ids)?;
    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("读取深度解读失败: {error}"))?;
    let existing = conn.query_row(
        "SELECT id, created_at FROM articles WHERE item_id = ?1 AND article_type = 'deep-dive' ORDER BY created_at DESC LIMIT 1",
        rusqlite::params![item_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    );
    let (article_id, created_at) = match existing {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            (Uuid::new_v4().to_string(), chrono::Utc::now().to_rfc3339())
        }
        Err(error) => return Err(format!("读取深度解读失败: {error}")),
    };
    drop(conn);
    let article = crate::db::Article {
        id: article_id.clone(),
        item_id: Some(item_id.to_string()),
        title: format!("深度解读 · {}", context.title),
        content: markdown,
        article_type: "deep-dive".into(),
        edited: false,
        created_at,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        prompt_version: Some(format!("hermes-discovery@v2:{current_policy_hash}")),
        blocks_json: None,
    };
    crate::notes::insert_article(app, &article)
        .map_err(|error| format!("保存深度解读失败: {error}"))?;
    Ok(json!({
        "success": true,
        "mode": "deep",
        "itemId": item_id,
        "articleId": article_id,
        "policyHash": current_policy_hash,
    }))
}

fn validate_required_headings(markdown: &str, required: &[String]) -> Result<(), String> {
    if required.is_empty() {
        return Ok(());
    }
    let actual = markdown
        .lines()
        .filter_map(|line| line.trim().strip_prefix("## "))
        .map(normalize_heading)
        .collect::<Vec<_>>();
    let mut cursor = 0usize;
    for expected in required {
        let Some(relative) = actual[cursor..]
            .iter()
            .position(|heading| heading == expected)
        else {
            return Err(format!(
                "深度解读未遵循当前数据源 Prompt：缺少或改写了章节 `## {expected}`"
            ));
        };
        cursor += relative + 1;
    }
    Ok(())
}

fn save_discovery_analysis(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result = (|| {
        let app = discovery_app(rt)?;
        let item_id = discovery_item_id(arguments)?;
        let context = load_discovery_item_context(&app, &item_id)?;
        match arguments.get("mode").and_then(Value::as_str) {
            Some("quick") => save_quick_analysis(&app, &item_id, &context, arguments),
            Some("deep") => save_deep_analysis(&app, &item_id, &context, arguments),
            _ => Err("mode 只能是 quick 或 deep".into()),
        }
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

fn title_bigrams(value: &str) -> HashSet<(char, char)> {
    let normalized = value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| ch.is_alphanumeric())
        .collect::<Vec<_>>();
    normalized
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

fn title_similarity(left: &str, right: &str) -> f64 {
    let left = title_bigrams(left);
    let right = title_bigrams(right);
    if left.is_empty() || right.is_empty() {
        return if left == right { 1.0 } else { 0.0 };
    }
    let overlap = left.intersection(&right).count();
    (2 * overlap) as f64 / (left.len() + right.len()) as f64
}

fn save_discovery_pick(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result = (|| {
        let app = discovery_app(rt)?;
        let item_id = discovery_item_id(arguments)?;
        let _context = load_discovery_item_context(&app, &item_id)?;
        let lane = arguments
            .get("lane")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| matches!(*value, "github" | "model" | "product"))
            .ok_or_else(|| "lane 只能是 github、model 或 product".to_string())?;
        let score = arguments
            .get("score")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && (0.0..=10.0).contains(value))
            .ok_or_else(|| "score 必须在 0 到 10 之间".to_string())?;
        let reason = bounded_text(arguments, "reason", 160)?;
        let mut conn = rusqlite::Connection::open(crate::db::get_db_path(&app))
            .map_err(|error| format!("发布发现卡片失败: {error}"))?;
        let (source_id, title, heat, config, has_quick): (
            String,
            String,
            Option<i64>,
            Option<String>,
            bool,
        ) = conn
            .query_row(
                "SELECT i.source_id, i.title, i.stars, s.config,
                        CASE WHEN i.ai_enrich_json IS NOT NULL AND i.ai_enrich_json != '' THEN 1 ELSE 0 END
                 FROM items i JOIN sources s ON s.id = i.source_id WHERE i.id = ?1",
                rusqlite::params![item_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .map_err(|error| format!("读取发现发布条件失败: {error}"))?;
        if !source_allows_lane(&source_id, lane) {
            return Err(format!("来源 {source_id} 不能发布到 {lane} 视角"));
        }
        let policy = discovery_policy(&source_id, config.as_deref());
        let min_score = policy["minScore"].as_f64().unwrap_or(8.0);
        if score < min_score {
            return Err(format!(
                "评分 {score:.1} 低于来源最低分 {min_score:.1}，已剔除"
            ));
        }
        if !has_quick {
            return Err("发布前必须先保存卡片速览".into());
        }
        let has_deep = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM articles WHERE item_id = ?1 AND article_type = 'deep-dive')",
                rusqlite::params![item_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("检查深度解读失败: {error}"))?;
        if !has_deep {
            return Err("发布前必须先保存深度解读".into());
        }

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let existing_today = conn.query_row(
            "SELECT id FROM daily_picks WHERE date = ?1 AND item_id = ?2 LIMIT 1",
            rusqlite::params![today, item_id],
            |row| row.get::<_, String>(0),
        );
        if let Ok(id) = existing_today {
            conn.execute(
                "UPDATE daily_picks SET ai_score = ?2, reason = ?3, selection_lane = ?4 WHERE id = ?1",
                rusqlite::params![id, score, reason, lane],
            )
            .map_err(|error| format!("更新今日发现失败: {error}"))?;
            return Ok(
                json!({"success": true, "itemId": item_id, "lane": lane, "score": score, "alreadySelected": true}),
            );
        }

        let prior_same = conn.query_row(
            "SELECT date, heat_score FROM daily_picks
             WHERE item_id = ?1 AND date(date) >= date('now', '-7 day')
             ORDER BY date DESC LIMIT 1",
            rusqlite::params![item_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        );
        if let Ok((date, previous_heat)) = prior_same {
            let has_material_growth = previous_heat.zip(heat).is_some_and(|(previous, current)| {
                previous > 0 && current as f64 > previous as f64 * 1.2
            });
            if !has_material_growth {
                return Err(format!("该条目已于 {date} 入选，且热度增长未超过 20%"));
            }
        }

        let mut stmt = conn
            .prepare(
                "SELECT i.id, i.title FROM daily_picks p
                 JOIN items i ON i.id = p.item_id
                 WHERE p.selection_lane = ?1 AND date(p.date) >= date('now', '-7 day')",
            )
            .map_err(|error| format!("检查近期重复失败: {error}"))?;
        let recent = stmt
            .query_map(rusqlite::params![lane], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("检查近期重复失败: {error}"))?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        drop(stmt);
        if let Some((duplicate_id, duplicate_title, similarity)) =
            recent.iter().find_map(|(recent_id, recent_title)| {
                let similarity = title_similarity(&title, recent_title);
                (similarity >= 0.8).then_some((recent_id, recent_title, similarity))
            })
        {
            return Err(format!(
                "与近期条目 {duplicate_id}《{duplicate_title}》标题重复度 {:.0}%，已剔除",
                similarity * 100.0
            ));
        }

        let category =
            source_category(&source_id).ok_or_else(|| format!("不支持的发现来源: {source_id}"))?;
        let rank = conn
            .query_row(
                "SELECT COUNT(*) + 1 FROM daily_picks WHERE date = ?1 AND category = ?2",
                rusqlite::params![today, category],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| format!("计算发现排序失败: {error}"))?;
        let tx = conn
            .transaction()
            .map_err(|error| format!("发布发现卡片失败: {error}"))?;
        tx.execute(
            "INSERT INTO daily_picks
             (id, date, category, item_id, rank, heat_score, ai_score, reason, selection_lane, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))",
            rusqlite::params![
                Uuid::new_v4().to_string(),
                today,
                category,
                item_id,
                rank,
                heat,
                score,
                reason,
                lane,
            ],
        )
        .map_err(|error| format!("发布发现卡片失败: {error}"))?;
        tx.commit()
            .map_err(|error| format!("发布发现卡片失败: {error}"))?;
        Ok(json!({
            "success": true,
            "itemId": item_id,
            "lane": lane,
            "category": category,
            "score": score,
            "rank": rank,
        }))
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

/// NEXT-048：全量评分持久化（Skill 打分趟的唯一落库入口）。
/// 信任边界同其余发现工具：只接受已存在 itemId，结构性校验在 discovery 模块。
fn save_discovery_scores(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result = (|| {
        let app = discovery_app(rt)?;
        let conn = rusqlite::Connection::open(crate::db::get_db_path(&app))
            .map_err(|error| format!("打开发现数据库失败: {error}"))?;
        crate::discovery::apply_discovery_scores(&conn, arguments)
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

fn read_discovery_feed(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result: Result<Value, String> = (|| {
        let app = discovery_app(rt)?;
        let conn = rusqlite::Connection::open(crate::db::get_db_path(&app))
            .map_err(|error| format!("打开发现数据库失败: {error}"))?;
        let all_time = arguments
            .get("allTime")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let missing_deep = arguments
            .get("missingDeep")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (from_date, to_date, period_key) = if all_time {
            (None, None, Some("all".to_string()))
        } else {
            let (from, to, key) = resolve_discovery_feed_window(arguments)?;
            (Some(from), Some(to), key)
        };
        let query = crate::discovery::DiscoveryFeedQuery {
            min_score: arguments
                .get("minScore")
                .and_then(Value::as_f64)
                .or(Some(7.0)),
            // 报告/榜单默认只消费成品；补全任务只看尚未成功保存 deep 的队列。
            require_deep: Some(!missing_deep),
            missing_deep: Some(missing_deep),
            from_date: from_date.clone(),
            to_date: to_date.clone(),
            limit: arguments.get("limit").and_then(Value::as_i64).or(Some(500)),
            ..Default::default()
        };
        let page = crate::discovery::query_discovery_feed(&conn, &query)?;
        // 报告/榜单只需要可引用的紧凑事实；把整段 description、UI 状态等字段
        // 原样交给模型会让 6 条动态膨胀到一万余字符，增加延迟且诱发重复读取。
        let items = page
            .rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "sourceId": row.source_id,
                    "sourceName": row.source_name,
                    "type": row.item_type,
                    "title": row.title,
                    "url": row.url,
                    "author": row.author,
                    "publishedAt": row.published_at,
                    "fetchedAt": row.fetched_at,
                    "aiScore": row.ai_score,
                    "aiScoredAt": row.ai_scored_at,
                    "aspect": row.aspect,
                    "aiTopics": row.ai_topics,
                    "aiReason": row.ai_reason,
                    "summary": row.ai_summary.as_deref().filter(|value| !value.is_empty()).map(|value| truncate_chars(value, 400)),
                    "description": row.description.as_deref().filter(|value| !value.is_empty()).map(|value| truncate_chars(value, 400)),
                    "stars": row.stars,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "success": true,
            "fromDate": from_date,
            "toDate": to_date,
            "periodKey": period_key,
            "count": items.len(),
            "items": items,
            "truncated": page.next_cursor.is_some()
        }))
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

fn resolve_discovery_feed_window(
    arguments: &Value,
) -> Result<(String, String, Option<String>), String> {
    use chrono::Datelike;

    let explicit_from = arguments.get("fromDate").and_then(Value::as_str);
    let explicit_to = arguments.get("toDate").and_then(Value::as_str);
    if explicit_from.is_some() || explicit_to.is_some() {
        let from_text = explicit_from.ok_or("fromDate 与 toDate 必须同时提供")?;
        let to_text = explicit_to.ok_or("fromDate 与 toDate 必须同时提供")?;
        let from = chrono::NaiveDate::parse_from_str(from_text, "%Y-%m-%d")
            .map_err(|_| "fromDate 必须是 YYYY-MM-DD")?;
        let to = chrono::NaiveDate::parse_from_str(to_text, "%Y-%m-%d")
            .map_err(|_| "toDate 必须是 YYYY-MM-DD")?;
        if from >= to {
            return Err("toDate 必须晚于 fromDate".into());
        }
        return Ok((from.to_string(), to.to_string(), None));
    }

    let period = arguments
        .get("period")
        .and_then(Value::as_str)
        .ok_or("必须提供 fromDate+toDate，或 period")?;
    let anchor = match arguments.get("date").and_then(Value::as_str) {
        Some(value) => chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| "date 必须是 YYYY-MM-DD")?,
        None => chrono::Local::now().date_naive(),
    };
    let day = chrono::Duration::days(1);
    let (from, to, key) = match period {
        "daily" => (anchor, anchor + day, anchor.to_string()),
        "weekly" => {
            let from =
                anchor - chrono::Duration::days(anchor.weekday().num_days_from_monday().into());
            let iso = anchor.iso_week();
            (
                from,
                from + chrono::Duration::days(7),
                format!("{}-W{:02}", iso.year(), iso.week()),
            )
        }
        "monthly" => {
            let from = chrono::NaiveDate::from_ymd_opt(anchor.year(), anchor.month(), 1)
                .ok_or("无法解析月份")?;
            let (year, month) = if anchor.month() == 12 {
                (anchor.year() + 1, 1)
            } else {
                (anchor.year(), anchor.month() + 1)
            };
            let to = chrono::NaiveDate::from_ymd_opt(year, month, 1).ok_or("无法解析下月")?;
            (
                from,
                to,
                format!("{:04}-{:02}", anchor.year(), anchor.month()),
            )
        }
        "rolling7" => (
            anchor - chrono::Duration::days(6),
            anchor + day,
            format!("rolling7-{anchor}"),
        ),
        _ => return Err("period 必须是 daily、weekly、monthly 或 rolling7".into()),
    };
    Ok((from.to_string(), to.to_string(), Some(key)))
}

fn save_discovery_report(rt: &BridgeHttpRuntime, arguments: &Value) -> Value {
    let result: Result<Value, String> = (|| {
        let app = discovery_app(rt)?;
        let period = arguments
            .get("period")
            .and_then(Value::as_str)
            .filter(|value| ["daily", "weekly", "monthly"].contains(value))
            .ok_or("period 必须是 daily、weekly 或 monthly")?;
        let period_key = arguments
            .get("periodKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 32
                    && value
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            })
            .ok_or("periodKey 仅允许字母、数字和连字符")?;
        let title = arguments
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.chars().count() <= 200)
            .ok_or("title 无效")?;
        let markdown = arguments
            .get("markdown")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| value.chars().count() >= 80 && value.len() <= 100_000)
            .ok_or("markdown 必须是 80-100000 字符的完整报告")?;
        if !markdown.starts_with("# ") || !markdown.contains("\n## ") {
            return Err("markdown 必须包含一级标题和至少一个二级小节".into());
        }
        let id = format!("ai-radar-report-{period}-{period_key}");
        let now = chrono::Utc::now().to_rfc3339();
        let article = crate::db::Article {
            id: id.clone(),
            item_id: None,
            title: title.to_string(),
            content: markdown.to_string(),
            article_type: "report".into(),
            edited: false,
            created_at: now,
            updated_at: None,
            prompt_version: Some(format!("report:{period}:{period_key}")),
            blocks_json: arguments
                .get("stats")
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| format!("stats 序列化失败: {error}"))?,
        };
        crate::notes::insert_article(&app, &article)
            .map_err(|error| format!("保存 AI 报告失败: {error}"))?;
        Ok(json!({
            "success": true,
            "id": id,
            "period": period,
            "periodKey": period_key,
            "title": title
        }))
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

fn openrouter_snapshot_summary(
    snapshot: &crate::openrouter_rankings::OpenRouterRankingSnapshot,
) -> Value {
    json!({
        "success": true,
        "asOf": snapshot.as_of,
        "fetchedAt": snapshot.fetched_at,
        "citation": snapshot.citation,
        "sourceUrl": snapshot.source_url,
        "counts": {
            "models": snapshot.models.as_array().map_or(0, Vec::len),
            "usageRows": snapshot.rankings_daily.as_array().map_or(0, Vec::len),
            "taskClassifications": snapshot.task_classifications
                .get("classifications").and_then(Value::as_array).map_or(0, Vec::len),
            "sessionCosts": snapshot.session_cost.as_array().map_or(0, Vec::len),
            "benchmarks": snapshot.benchmarks.as_array().map_or(0, Vec::len),
        }
    })
}

fn refresh_openrouter_rankings(rt: &BridgeHttpRuntime) -> Value {
    let app = match discovery_app(rt) {
        Ok(app) => app,
        Err(error) => return tool_error(error),
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("创建 OpenRouter 刷新运行时失败: {error}"))
        .and_then(|runtime| runtime.block_on(crate::openrouter_rankings::refresh_snapshot(&app)));
    match result {
        Ok(snapshot) => tool_json(openrouter_snapshot_summary(&snapshot)),
        Err(error) => tool_error(error),
    }
}

fn read_openrouter_rankings(rt: &BridgeHttpRuntime) -> Value {
    let result: Result<Value, String> = (|| {
        let app = discovery_app(rt)?;
        let conn = rusqlite::Connection::open(crate::db::get_db_path(&app))
            .map_err(|error| format!("打开 OpenRouter 快照数据库失败: {error}"))?;
        crate::openrouter_rankings::read_snapshot(&conn)?
            .map(|snapshot| openrouter_snapshot_summary(&snapshot))
            .ok_or_else(|| "尚无 OpenRouter 模型榜快照".to_string())
    })();
    match result {
        Ok(payload) => tool_json(payload),
        Err(error) => tool_error(error),
    }
}

fn strip_mcp_prefix(name: &str) -> String {
    for sep in ["__", ".", "/"] {
        if let Some((_, rest)) = name.split_once(sep) {
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    // mcp_sophonote-bridge_list_project_documents
    if let Some(rest) = name.strip_prefix("mcp_sophonote-bridge_") {
        return rest.to_string();
    }
    if let Some(rest) = name.strip_prefix("sophonote-bridge_") {
        return rest.to_string();
    }
    name.to_string()
}

fn tool_error(msg: impl Into<String>) -> Value {
    let m = msg.into();
    json!({
        "content": [{ "type": "text", "text": m }],
        "isError": true
    })
}

fn read_http_request(
    stream: &mut TcpStream,
) -> std::io::Result<(String, String, HashMap<String, String>, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 256 * 1024 {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(buf.len());
    let header_text = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            }
            headers.insert(key, val);
        }
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    let body = String::from_utf8_lossy(&body).to_string();
    Ok((method, path, headers, body))
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_prefixes() {
        assert_eq!(
            strip_mcp_prefix("sophonote-bridge__read_document"),
            "read_document"
        );
        assert_eq!(
            strip_mcp_prefix("mcp_sophonote-bridge_list_project_documents"),
            "list_project_documents"
        );
        assert_eq!(strip_mcp_prefix("read_document"), "read_document");
    }

    #[test]
    fn discovery_feed_period_windows_are_host_resolved() {
        assert_eq!(
            resolve_discovery_feed_window(&json!({"period":"daily","date":"2026-08-17"})).unwrap(),
            (
                "2026-08-17".into(),
                "2026-08-18".into(),
                Some("2026-08-17".into())
            )
        );
        assert_eq!(
            resolve_discovery_feed_window(&json!({"period":"weekly","date":"2026-08-17"})).unwrap(),
            (
                "2026-08-17".into(),
                "2026-08-24".into(),
                Some("2026-W34".into())
            )
        );
        assert_eq!(
            resolve_discovery_feed_window(&json!({"period":"monthly","date":"2026-12-17"}))
                .unwrap(),
            (
                "2026-12-01".into(),
                "2027-01-01".into(),
                Some("2026-12".into())
            )
        );
        assert_eq!(
            resolve_discovery_feed_window(&json!({"period":"rolling7","date":"2026-08-17"}))
                .unwrap(),
            (
                "2026-08-11".into(),
                "2026-08-18".into(),
                Some("rolling7-2026-08-17".into())
            )
        );
        assert!(resolve_discovery_feed_window(&json!({"fromDate":"2026-08-17"})).is_err());
    }

    #[tokio::test]
    async fn ensure_bridge_listens_and_tools_list() {
        let rt = ensure_bridge_http().expect("listen");
        let url = rt.mcp_url();
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("loopback client");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        });
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", rt.bearer))
            .json(&body)
            .send()
            .await
            .expect("post");
        assert!(resp.status().is_success());
        let v: Value = resp.json().await.unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 14);
        let host_tools = [
            "refresh_discovery_sources",
            "list_discovery_candidates",
            "read_discovery_item",
            "save_discovery_analysis",
            "save_discovery_pick",
            "save_discovery_scores",
            "read_discovery_feed",
            "save_discovery_report",
            "refresh_openrouter_rankings",
            "read_openrouter_rankings",
        ];
        for tool in tools
            .iter()
            .filter(|tool| !host_tools.contains(&tool["name"].as_str().unwrap_or_default()))
        {
            assert!(tool["inputSchema"]["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|field| field == "leaseId")));
        }
        let refresh = tools
            .iter()
            .find(|tool| tool["name"] == "refresh_discovery_sources")
            .expect("refresh tool");
        assert_eq!(refresh["inputSchema"]["required"], json!(["sources"]));
        let read = tools
            .iter()
            .find(|tool| tool["name"] == "read_discovery_item")
            .expect("read discovery tool");
        assert_eq!(read["inputSchema"]["required"], json!(["itemId"]));
        let save = tools
            .iter()
            .find(|tool| tool["name"] == "save_discovery_analysis")
            .expect("save discovery tool");
        assert_eq!(save["inputSchema"]["required"], json!(["itemId", "mode"]));
        assert_eq!(
            save["inputSchema"]["properties"]["policyHash"]["pattern"],
            json!("^sha256:[a-f0-9]{64}$")
        );
        let candidates = tools
            .iter()
            .find(|tool| tool["name"] == "list_discovery_candidates")
            .expect("candidate discovery tool");
        assert_eq!(candidates["inputSchema"]["required"], json!(["sources"]));
        assert_eq!(
            candidates["inputSchema"]["properties"]["limitPerSource"]["default"],
            json!(4)
        );
        assert!(candidates["description"]
            .as_str()
            .is_some_and(|description| description.contains("不读取完整正文")));
        let pick = tools
            .iter()
            .find(|tool| tool["name"] == "save_discovery_pick")
            .expect("save discovery pick tool");
        assert_eq!(
            pick["inputSchema"]["required"],
            json!(["itemId", "lane", "score", "reason"])
        );
        assert!(pick["description"]
            .as_str()
            .is_some_and(|description| description.contains("不设置每日数量配额")));
    }

    #[test]
    fn discovery_source_keys_are_strict_and_expand_huggingface() {
        assert_eq!(
            discovery_source_ids(&json!({"sources": ["github", "huggingface"]})).unwrap(),
            vec![
                "github-trending",
                "huggingface-models",
                "huggingface-papers"
            ]
        );
        assert!(discovery_source_ids(&json!({"sources": ["custom"]})).is_err());
        assert!(discovery_source_ids(&json!({"sources": []})).is_err());
        // aihot 直映射单源；六个类别全量可一次提交
        assert_eq!(
            discovery_source_ids(&json!({"sources": ["aihot"]})).unwrap(),
            vec!["aihot"]
        );
        assert_eq!(
            discovery_source_ids(&json!({"sources": [
                "github", "arxiv", "hackernews", "producthunt", "huggingface", "aihot"
            ]}))
            .unwrap()
            .len(),
            7
        );
        // 输入数组超过 6 个键即拒绝（上限校验先于映射去重）
        assert!(discovery_source_ids(&json!({"sources": [
            "github", "arxiv", "hackernews", "producthunt", "huggingface", "aihot", "aihot"
        ]}))
        .is_err());
    }

    #[test]
    fn discovery_citations_require_existing_evidence() {
        let available = HashSet::from(["E1".to_string(), "E2".to_string()]);
        assert!(validate_evidence_citations("结论 [E1]", &available).is_ok());
        assert!(validate_evidence_citations("没有引用", &available).is_err());
        assert!(validate_evidence_citations("错误引用 [E3]", &available).is_err());
    }

    #[test]
    fn aihot_unsupported_content_uses_candidate_snapshot_as_e1() {
        let evidence =
            aihot_snapshot_evidence(Some("已抓取的完整候选描述")).expect("snapshot evidence");
        assert_eq!(evidence[0]["id"], "E1");
        assert_eq!(evidence[0]["kind"], "candidate_snapshot");
        assert_eq!(evidence[0]["text"], "已抓取的完整候选描述");
        assert!(aihot_snapshot_evidence(Some("   ")).is_none());
    }

    #[test]
    fn discovery_policy_extracts_only_numbered_output_headings() {
        let prompt =
            "# 输出结构\n\n## 1. 一句话结论\n\n### 子标题\n\n## 2、项目速览\n\n## 表达要求";
        assert_eq!(
            required_numbered_headings(prompt),
            vec!["1. 一句话结论", "2、项目速览"]
        );
    }

    #[test]
    fn deep_output_must_preserve_policy_heading_names_and_order() {
        let required = vec!["1. 一句话结论".into(), "2. 项目速览".into()];
        assert!(validate_required_headings(
            "## 1. 一句话结论\n内容 [E1]\n\n## 2. 项目速览\n内容 [E1]",
            &required,
        )
        .is_ok());
        assert!(validate_required_headings(
            "## 2. 项目速览\n内容 [E1]\n\n## 1. 一句话结论\n内容 [E1]",
            &required,
        )
        .is_err());
        assert!(validate_required_headings("## 核心定位\n内容 [E1]", &required,).is_err());
    }

    #[test]
    fn policy_hash_changes_with_user_generation_prompt() {
        let first = discovery_policy_hash("Prompt A", "Rule", 8.0);
        let second = discovery_policy_hash("Prompt B", "Rule", 8.0);
        assert!(first.starts_with("sha256:"));
        assert_eq!(first.len(), 71);
        assert_ne!(first, second);
    }

    #[test]
    fn discovery_title_similarity_rejects_near_duplicates() {
        assert_eq!(
            title_similarity("DeepSeek V4 Release", "DeepSeek V4 Release"),
            1.0
        );
        assert!(
            title_similarity(
                "DeepSeek V4 Release: Architecture and Benchmarks",
                "DeepSeek V4 Release Architecture & Benchmarks"
            ) >= 0.8
        );
        assert!(title_similarity("A New Diffusion Model", "PostgreSQL Query Planner") < 0.8);
    }

    #[test]
    fn registered_runs_never_create_an_implicit_active_lease() {
        let rt = BridgeHttpRuntime::new("test-bearer".into(), "http://127.0.0.1:1".into());
        let route = crate::sophonote_mcp::ModelRoute {
            provider_id: "test".into(),
            base_url: "https://example.invalid".into(),
            model: "test-model".into(),
        };
        for (run_id, project_id) in [("run-a", "project-a"), ("run-b", "project-b")] {
            let lease = crate::sophonote_mcp::issue_lease(
                run_id,
                project_id,
                ["list_project_documents".to_string()],
                route.clone(),
                60_000,
            );
            rt.register_run(lease, Arc::new(ToolRegistry::new()));
        }

        let result = tools_call(
            &rt,
            &HashMap::new(),
            &json!({
                "name": "list_project_documents",
                "arguments": {}
            }),
        );
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("缺少 lease_id")));
    }
}
