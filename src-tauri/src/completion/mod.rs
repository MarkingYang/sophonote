// ============================================================
// Track B · AG-30 CompletionService（docs/architecture.md §4.4 / §4.5）
// 独立低延迟补全服务：
//  - 模型调用走 ModelGateway（全应用唯一出口），**不创建 Agent Thread/Run**；
//  - FIM 上下文（标题/大纲/光标前后窗口，近处优先，不默认发整篇）；
//  - 超时/取消（CancellationToken 注册表，前端 NB-33 经 completion_cancel 传播）；
//  - 进程内缓存（TTL 60s、上限 64 条，仅内存不落库）；
//  - 质量过滤（§4.4：禁围栏/多候选/解释，≤120 字，上下文不足返回空）；
//  - 聚合指标只记耗时/接受/拒绝/超时/错误计数——**默认不持久化正文上下文或建议全文**（§4.5 隐私门禁）。
// 前端接入与设置项 UI 归 NB-33；本轮只提供 Rust 服务与命令。
// ============================================================
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::commands::ApiResponse;
use crate::model::gateway::ModelGateway;
use crate::model::messages::{ModelError, ModelMessage, ModelRequest};
use crate::model::openai_compat::OpenAiCompatGateway;
use crate::model::prompt_registry;

/// 服务侧硬超时（设置可覆盖，夹紧 300..=5000）。
/// B0B 实测定案（2026-08-08）：非流式等全文返回，1500ms 在真实模型上必然超时静默；
/// 3000ms 覆盖 max_tokens=128 的典型生成时长，仍保留「宁缺毋滥」的低延迟取向。
const DEFAULT_TIMEOUT_MS: u64 = 3000;
/// 单条建议长度上限（§4.5）
const MAX_SUGGESTION_CHARS: usize = 120;
/// 缓存 TTL 与容量（仅进程内）
const CACHE_TTL: Duration = Duration::from_secs(60);
const CACHE_CAP: usize = 64;

// ---------- §4.3 请求/结果契约（camelCase，与前端 inlineCompletion.ts 逐字段对齐） ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionCaret {
    pub prose_pos: u32,
    pub anchor_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionRequest {
    pub request_id: String,
    pub article_id: String,
    pub document_version: u64,
    pub caret: CompletionCaret,
    #[serde(default)]
    pub language: String,
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub outline: Vec<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub trigger: String,
}

/// finishReason 取值严格 §4.3：complete / timeout / filtered。
/// 服务错误（配置/网络/HTTP）统一收敛为 filtered + 空 text（指标另记 error），
/// 前端控制器对空 text 一律不展示——失败绝不产生幽灵建议。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResponse {
    pub request_id: String,
    pub article_id: String,
    pub document_version: u64,
    pub anchor_hash: String,
    pub text: String,
    pub finish_reason: String,
    pub model: String,
    pub latency_ms: u64,
}

fn bare_response(
    req: &CompletionRequest,
    finish: &str,
    model: &str,
    latency_ms: u64,
) -> CompletionResponse {
    CompletionResponse {
        request_id: req.request_id.clone(),
        article_id: req.article_id.clone(),
        document_version: req.document_version,
        anchor_hash: req.caret.anchor_hash.clone(),
        text: String::new(),
        finish_reason: finish.to_string(),
        model: model.to_string(),
        latency_ms,
    }
}

// ---------- 设置（NB-33 设置页写入；缺省 = 开启、跟随 activeProvider 模型） ----------

#[derive(Debug, Clone)]
pub struct CompletionConfig {
    pub enabled: bool,
    pub model: Option<String>,
    pub timeout_ms: u64,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

/// 读 SQLite settings.completion_config（{enabled, model, timeoutMs}），缺失/畸形回默认
pub fn load_config(app: &AppHandle) -> CompletionConfig {
    let mut cfg = CompletionConfig::default();
    let db_path = crate::db::get_db_path(app);
    let Ok(conn) = rusqlite::Connection::open(&db_path) else {
        return cfg;
    };
    let Ok(raw) = conn.query_row(
        "SELECT value FROM settings WHERE key = 'completion_config'",
        [],
        |r| r.get::<_, String>(0),
    ) else {
        return cfg;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return cfg;
    };
    if let Some(e) = v.get("enabled").and_then(|x| x.as_bool()) {
        cfg.enabled = e;
    }
    if let Some(m) = v
        .get("model")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        cfg.model = Some(m.to_string());
    }
    if let Some(t) = v.get("timeoutMs").and_then(|x| x.as_u64()) {
        cfg.timeout_ms = t.clamp(300, 5000);
    }
    cfg
}

// ---------- 取消注册表（AG-18 CancelRegistry 同款范式） ----------

fn cancel_registry() -> &'static Mutex<HashMap<String, CancellationToken>> {
    static REG: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_cancel(request_id: &str) -> CancellationToken {
    let token = CancellationToken::new();
    cancel_registry()
        .lock()
        .unwrap()
        .insert(request_id.to_string(), token.clone());
    token
}

fn unregister_cancel(request_id: &str) {
    cancel_registry().lock().unwrap().remove(request_id);
}

/// 前端取消传播：true = 请求在途已被派发取消；终态后再调 = 无害 false
pub fn cancel_request(request_id: &str) -> bool {
    match cancel_registry().lock().unwrap().remove(request_id) {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn completion_cancel(request_id: String) -> ApiResponse<bool> {
    ApiResponse::ok(cancel_request(&request_id))
}

// ---------- 进程内缓存（key = 绑定四元组语义子集；value = 已过过滤的建议） ----------

#[derive(Clone)]
struct CacheEntry {
    text: String,
    at: Instant,
    seq: u64,
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static C: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(req: &CompletionRequest) -> String {
    // 模型覆盖变化不影响缓存语义（缓存的是过滤后文本）；key 取定位四元组的上下文子集
    format!(
        "{}|{}|{}|{}",
        req.article_id,
        req.caret.anchor_hash,
        djb2(req.prefix.as_bytes()),
        djb2(req.suffix.as_bytes())
    )
}

fn djb2(bytes: &[u8]) -> u64 {
    let mut h: u64 = 5381;
    for b in bytes {
        h = h.wrapping_mul(33).wrapping_add(*b as u64);
    }
    h
}

fn cache_lookup(key: &str) -> Option<String> {
    let mut map = cache().lock().unwrap();
    let keep = map
        .get(key)
        .map(|e| e.at.elapsed() < CACHE_TTL)
        .unwrap_or(false);
    if !keep {
        map.remove(key);
        return None;
    }
    map.get(key).map(|e| e.text.clone())
}

fn cache_store(key: &str, text: &str) {
    static SEQ: Mutex<u64> = Mutex::new(0);
    let mut map = cache().lock().unwrap();
    let mut seq = SEQ.lock().unwrap();
    *seq += 1;
    map.insert(
        key.to_string(),
        CacheEntry {
            text: text.to_string(),
            at: Instant::now(),
            seq: *seq,
        },
    );
    if map.len() > CACHE_CAP {
        // 淘汰最旧插入
        if let Some(oldest) = map.values().map(|e| e.seq).min() {
            map.retain(|_, e| e.seq != oldest);
        }
    }
}

// ---------- 质量过滤（§4.4：只续写、不解释、不编造、≤120 字） ----------

/// 模型原文 → 可展示建议。任何结构性/越界/重叠内容返回 None（= filtered）。
pub fn sanitize_suggestion(raw: &str, suffix: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains("```") {
        return None;
    }
    // 多候选/换行结构：只取首行
    let line = trimmed.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return None;
    }
    let mut s = line.to_string();
    // 剥成对包裹引号（模型爱给续写加引号）
    loop {
        let stripped: Option<String> = [('"', '"'), ('\'', '\''), ('“', '”'), ('「', '」')]
            .iter()
            .find_map(|(o, c)| {
                let cs: Vec<char> = s.chars().collect();
                if cs.len() > 2 && cs[0] == *o && cs[cs.len() - 1] == *c {
                    Some(cs[1..cs.len() - 1].iter().collect())
                } else {
                    None
                }
            });
        match stripped {
            Some(inner) if !inner.trim().is_empty() => s = inner.trim().to_string(),
            _ => break,
        }
    }
    // Markdown 结构/列表/多候选标记 → 拒
    if s.starts_with('#')
        || s.starts_with('|')
        || s.starts_with("- ")
        || s.starts_with("* ")
        || s.starts_with("- [")
        || s.contains("\n1.")
    {
        return None;
    }
    // ≤120 字；超出按句末标点截断，截不出完整句则拒
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > MAX_SUGGESTION_CHARS {
        let window = &chars[..MAX_SUGGESTION_CHARS];
        match window.iter().rposition(|&ch| "。！？!?；;".contains(ch)) {
            Some(i) if i >= 20 => s = window[..=i].iter().collect(),
            _ => return None,
        }
    }
    if s.trim().is_empty() {
        return None;
    }
    // 与光标后既有文本重叠（互为前缀）→ 拒，避免建议「已经存在的话」
    let suf = suffix.trim_start();
    if !suf.is_empty() {
        let st = s.trim_start();
        if suf.starts_with(st) || st.starts_with(suf) {
            return None;
        }
    }
    Some(s)
}

// ---------- FIM 上下文装配（§4.4 近处优先；prefix/suffix 已由前端截断 ≤400/≤200） ----------

fn build_messages(req: &CompletionRequest) -> Vec<ModelMessage> {
    let mut user = String::new();
    if !req.title.is_empty() {
        user.push_str(&format!("标题：{}\n", req.title));
    }
    if !req.outline.is_empty() {
        user.push_str(&format!("大纲：{}\n", req.outline.join(" / ")));
    }
    user.push_str("光标前：\n");
    user.push_str(&req.prefix);
    user.push_str("\n光标后：\n");
    user.push_str(&req.suffix);
    user.push_str("\n只输出插入光标处的续写文本。");

    vec![
        ModelMessage::system(
            "你是中文写作续写引擎。根据光标前后上下文，续写 1～2 个短句（≤120 字）。\
             规则：不解释、不加引号、不用 Markdown 围栏、不输出多个候选；\
             不编造引用、数字或事实；只「继续写」，不回答提问；\
             上下文不足以自然续写时，输出空字符串。",
        ),
        ModelMessage::user(user),
    ]
}

// ---------- 聚合指标（§4.5：只记聚合，不记正文/建议全文） ----------

#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionMetrics {
    pub requests: u64,
    pub cache_hits: u64,
    pub completes: u64,
    pub filtered: u64,
    pub timeouts: u64,
    pub errors: u64,
    pub accepts: u64,
    pub rejects: u64,
    pub latency_sum_ms: u64,
    pub latency_n: u64,
}

fn metrics() -> &'static Mutex<CompletionMetrics> {
    static M: OnceLock<Mutex<CompletionMetrics>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(CompletionMetrics::default()))
}

/// NB-33 在接受/拒绝时上报，用于接受率与降噪（聚合计数，不携带内容）
#[tauri::command]
pub fn completion_report_feedback(accepted: bool) -> ApiResponse<bool> {
    let mut m = metrics().lock().unwrap();
    if accepted {
        m.accepts += 1;
    } else {
        m.rejects += 1;
    }
    ApiResponse::ok(true)
}

#[tauri::command]
pub fn completion_metrics() -> ApiResponse<CompletionMetrics> {
    ApiResponse::ok(metrics().lock().unwrap().clone())
}

// ---------- 服务核心（与 AppHandle 解耦，可单测） ----------

/// 同步闭包内更新指标——MutexGuard 生命周期绝不跨 await（tauri 命令要求 Future: Send）
fn metrics_inc(f: impl FnOnce(&mut CompletionMetrics)) {
    let mut m = metrics().lock().unwrap();
    f(&mut m);
}

enum SuggestOutcome {
    TimedOut,
    Errored,
    Filtered,
    Complete(String),
}

/// 网关级补全：缓存 → 模型（超时+取消）→ 过滤 → 回包。绑定四元组原样回显，前端二次校验。
pub async fn suggest_with_gateway(
    gateway: &dyn ModelGateway,
    effective_model: &str,
    timeout_ms: u64,
    req: &CompletionRequest,
) -> CompletionResponse {
    let started = Instant::now();
    metrics_inc(|m| m.requests += 1);

    let key = cache_key(req);
    if let Some(text) = cache_lookup(&key) {
        let latency = started.elapsed().as_millis() as u64;
        metrics_inc(|m| {
            m.cache_hits += 1;
            m.completes += 1;
            m.latency_sum_ms += latency;
            m.latency_n += 1;
        });
        return CompletionResponse {
            text,
            finish_reason: "complete".to_string(),
            model: "cache".to_string(),
            ..bare_response(req, "complete", "cache", latency)
        };
    }

    let token = register_cancel(&req.request_id);
    let model_req = ModelRequest {
        model: effective_model.to_string(),
        messages: build_messages(req),
        tools: Vec::new(),
        tool_choice: None,
        temperature: Some(0.2),
        // §4.4 只保留首行且截断到 MAX_SUGGESTION_CHARS=120 字——超出部分全是废生成。
        // 128 token 给中英混排留余量，同时把非流式等待压进超时预算（B0B 实测：256→超时主因）
        max_tokens: Some(128),
        // DeepSeek V4 默认思考会与正文共享 max_tokens；补全只需直接续写，禁用思考
        // 避免 token 被 reasoning_content 耗尽后 content 为空。Gateway 仅对支持的供应商下发。
        thinking: Some(false),
        prompt_version: prompt_registry::expect_version("completion").clone(),
        run_id: None,
    };

    let outcome = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        gateway.complete(model_req, token.clone()),
    )
    .await;
    unregister_cancel(&req.request_id);
    let latency = started.elapsed().as_millis() as u64;

    // 先归类（无锁），再一次性同步更新指标——guard 不跨 await
    let kind = match outcome {
        Err(_elapsed) => {
            token.cancel();
            SuggestOutcome::TimedOut
        }
        Ok(Err(ModelError::Cancelled)) => SuggestOutcome::TimedOut,
        Ok(Err(_)) => SuggestOutcome::Errored,
        Ok(Ok(resp)) => match sanitize_suggestion(&resp.content, &req.suffix) {
            Some(t) => SuggestOutcome::Complete(t),
            None => SuggestOutcome::Filtered,
        },
    };

    let (finish, text) = metrics_inc_result(|m| {
        m.latency_sum_ms += latency;
        m.latency_n += 1;
        match kind {
            SuggestOutcome::TimedOut => {
                m.timeouts += 1;
                ("timeout".to_string(), String::new())
            }
            SuggestOutcome::Errored => {
                m.errors += 1;
                ("filtered".to_string(), String::new())
            }
            SuggestOutcome::Filtered => {
                m.filtered += 1;
                ("filtered".to_string(), String::new())
            }
            SuggestOutcome::Complete(t) => {
                m.completes += 1;
                ("complete".to_string(), t)
            }
        }
    });

    if finish == "complete" {
        cache_store(&key, &text);
    }
    CompletionResponse {
        text,
        ..bare_response(req, &finish, effective_model, latency)
    }
}

fn metrics_inc_result<T>(f: impl FnOnce(&mut CompletionMetrics) -> T) -> T {
    let mut m = metrics().lock().unwrap();
    f(&mut m)
}

// ---------- Tauri 命令层 ----------

/// 自然语言补全（AG-30）：不创建 Thread/Run 的轻量路径。
/// 关闭/配置缺失/失败一律返回 ok + 空 text（filtered），前端静默不展示——补全失败不得弹错。
#[tauri::command]
pub async fn completion_suggest(
    app: AppHandle,
    request: CompletionRequest,
) -> ApiResponse<CompletionResponse> {
    let started = Instant::now();
    let cfg = load_config(&app);
    if !cfg.enabled {
        return ApiResponse::ok(bare_response(&request, "filtered", "disabled", 0));
    }
    let gateway = match OpenAiCompatGateway::from_settings(&app, None) {
        Ok(g) => g,
        Err(_) => {
            return ApiResponse::ok(bare_response(
                &request,
                "filtered",
                "unconfigured",
                started.elapsed().as_millis() as u64,
            ))
        }
    };
    let model = cfg
        .model
        .clone()
        .unwrap_or_else(|| gateway.default_model.clone());
    let resp = suggest_with_gateway(&gateway, &model, cfg.timeout_ms, &request).await;
    ApiResponse::ok(resp)
}

// ---------- 单测（零真实模型调用） ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::messages::{FinishReason, ModelResponse, TokenUsage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockGateway {
        text: String,
        delay_ms: u64,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelGateway for MockGateway {
        async fn complete(
            &self,
            req: ModelRequest,
            cancel: CancellationToken,
        ) -> Result<ModelResponse, ModelError> {
            assert_eq!(
                req.thinking,
                Some(false),
                "completion requests must disable model thinking"
            );
            if self.delay_ms > 0 {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {}
                    _ = cancel.cancelled() => return Err(ModelError::Cancelled),
                }
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ModelResponse {
                content: self.text.clone(),
                reasoning: None,
                tool_calls: Vec::new(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }
    }

    fn req(prefix: &str) -> CompletionRequest {
        CompletionRequest {
            request_id: format!("ic-t{}", djb2(prefix.as_bytes())),
            article_id: "a1".to_string(),
            document_version: 1,
            caret: CompletionCaret {
                prose_pos: 5,
                anchor_hash: format!("h{}", djb2(prefix.as_bytes())),
            },
            language: "auto".to_string(),
            prefix: prefix.to_string(),
            suffix: String::new(),
            title: "测试".to_string(),
            outline: vec![],
            project_id: None,
            trigger: "typing".to_string(),
        }
    }

    // ----- sanitize -----

    #[test]
    fn sanitize_keeps_short_sentence() {
        assert_eq!(
            sanitize_suggestion("  今天适合出门走走。 ", ""),
            Some("今天适合出门走走。".to_string())
        );
    }

    #[test]
    fn sanitize_rejects_fence_and_structure() {
        assert_eq!(sanitize_suggestion("```js\nx```", ""), None);
        assert_eq!(sanitize_suggestion("# 标题式续写", ""), None);
        assert_eq!(sanitize_suggestion("| 表格 | 行 |", ""), None);
        assert_eq!(sanitize_suggestion("- 列表项", ""), None);
    }

    #[test]
    fn sanitize_takes_first_line_and_strips_quotes() {
        assert_eq!(
            sanitize_suggestion("「继续写的一句。」\n第二个候选", ""),
            Some("继续写的一句。".to_string())
        );
        assert_eq!(
            sanitize_suggestion("\"带引号的续写\"", ""),
            Some("带引号的续写".to_string())
        );
    }

    #[test]
    fn sanitize_truncates_at_sentence_boundary() {
        // 132 字：第 110 字处有句号（落在 120 窗口内），超出部分应被截掉
        let long = format!(
            "{}。{}。",
            "甲乙丙丁戊己庚辛壬癸".repeat(11),
            "丁".repeat(20)
        );
        let out = sanitize_suggestion(&long, "").unwrap();
        assert!(out.chars().count() <= MAX_SUGGESTION_CHARS);
        assert!(out.ends_with('。'));
        // 无标点超长 → 拒
        let nopunc = "甲".repeat(200);
        assert_eq!(sanitize_suggestion(&nopunc, ""), None);
    }

    #[test]
    fn sanitize_rejects_suffix_overlap() {
        assert_eq!(
            sanitize_suggestion("后半段已有内容", "后半段已有内容，接着写"),
            None
        );
    }

    // ----- 服务核心 -----

    #[tokio::test]
    async fn suggest_complete_echoes_binding() {
        let gw = MockGateway {
            text: "续写的一句。".to_string(),
            delay_ms: 0,
            calls: AtomicUsize::new(0),
        };
        let r = req("今天天气不错");
        let resp = suggest_with_gateway(&gw, "mock-model", 1000, &r).await;
        assert_eq!(resp.finish_reason, "complete");
        assert_eq!(resp.text, "续写的一句。");
        assert_eq!(resp.request_id, r.request_id);
        assert_eq!(resp.article_id, r.article_id);
        assert_eq!(resp.document_version, r.document_version);
        assert_eq!(resp.anchor_hash, r.caret.anchor_hash);
        assert_eq!(resp.model, "mock-model");
    }

    #[tokio::test]
    async fn suggest_filtered_when_model_returns_fence() {
        let gw = MockGateway {
            text: "```\ncode\n```".to_string(),
            delay_ms: 0,
            calls: AtomicUsize::new(0),
        };
        let resp = suggest_with_gateway(&gw, "m", 1000, &req("前")).await;
        assert_eq!(resp.finish_reason, "filtered");
        assert_eq!(resp.text, "");
    }

    #[tokio::test]
    async fn suggest_timeout_when_gateway_slow() {
        let gw = MockGateway {
            text: "慢响应".to_string(),
            delay_ms: 200,
            calls: AtomicUsize::new(0),
        };
        let resp = suggest_with_gateway(&gw, "m", 30, &req("前")).await;
        assert_eq!(resp.finish_reason, "timeout");
        assert_eq!(resp.text, "");
    }

    #[tokio::test]
    async fn suggest_cancel_propagates() {
        // 测试并行运行且共享进程级取消注册表，使用独立绑定键避免与其他超时用例互相注销。
        let r = req("取消传播专用前缀");
        let rid = r.request_id.clone();
        let handle = tokio::spawn(async move {
            let gw = MockGateway {
                text: "慢响应".to_string(),
                delay_ms: 500,
                calls: AtomicUsize::new(0),
            };
            suggest_with_gateway(&gw, "m", 400, &r).await
        });
        // 等在途登记后取消
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(cancel_request(&rid));
        let resp = handle.await.unwrap();
        assert_eq!(resp.finish_reason, "timeout");
    }

    #[tokio::test]
    async fn cache_second_call_hits_without_gateway() {
        let gw = MockGateway {
            text: "缓存候选句。".to_string(),
            delay_ms: 0,
            calls: AtomicUsize::new(0),
        };
        let r = req("缓存前缀");
        let first = suggest_with_gateway(&gw, "m", 1000, &r).await;
        assert_eq!(first.finish_reason, "complete");
        let second = suggest_with_gateway(&gw, "m", 1000, &r).await;
        assert_eq!(second.finish_reason, "complete");
        assert_eq!(second.model, "cache");
        assert_eq!(gw.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_ttl_expiry() {
        let key = "ttl-test-key";
        cache_store(key, "过期候选");
        assert!(cache_lookup(key).is_some());
        // 手动改写时间为过去
        {
            let mut map = cache().lock().unwrap();
            if let Some(e) = map.get_mut(key) {
                e.at = Instant::now() - CACHE_TTL - Duration::from_secs(1);
            }
        }
        assert!(cache_lookup(key).is_none());
    }

    #[test]
    fn build_messages_includes_title_and_window() {
        let mut r = req("光标前文本");
        r.suffix = "光标后".to_string();
        r.outline = vec!["一".to_string(), "二".to_string()];
        let msgs = build_messages(&r);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[1].content.contains("标题：测试"));
        assert!(msgs[1].content.contains("大纲：一 / 二"));
        assert!(msgs[1].content.contains("光标前文本"));
        assert!(msgs[1].content.contains("光标后"));
    }
}
