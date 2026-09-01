//! DocumentService（AG-24，审计批次 5 第 2-4 步）：Agent-safe 文档写入服务。
//!
//! 设计基线：docs/architecture.md（写流程/冲突/幂等/恢复）。
//!
//! 铁律（docs/architecture.md）：
//! ① 模型侧写工具恒 dry-run——只有「提议修改」（propose_document_patch），
//!    落盘必须经用户批准（apply 命令），Agent 无直接写入能力；
//! ② apply 在批准时刻复检 base_version 与锚点唯一性——预览到批准之间
//!    用户动过文档 → 冲突拒绝，绝不静默覆盖；
//! ③ 锚点 0 匹配 / 多处匹配 → 冲突停止，不做全局字符串 replace 猜测；
//! ④ idempotency_key 重入返回原结果，不重复执行副作用（docs/architecture.md）；
//! ⑤ 文件与 SQLite 无法同一事务 → 操作日志 + 唯一临时文件（带 operation ID，
//!    不用固定 .md.tmp）+ 原子 rename + 启动恢复的补偿方案。

use std::path::Path;

use rusqlite::OptionalExtension;

use super::anchor;
use super::repository::{self, DocError, DocumentRecord};

// ------------------- 错误 -------------------

/// 服务层错误。Doc 错误透传；冲突/审批类独立变体，便于外层精确断言与措辞。
#[derive(Debug)]
pub enum ServiceError {
    Doc(DocError),
    /// 锚点文本在正文中 0 匹配
    AnchorNotFound,
    /// 锚点文本在正文中匹配 n>1 处：唯一性不足，禁止猜测替换
    AnchorAmbiguous(usize),
    /// Markdown 结构校验失败（AG-25：round-trip 可替换结构不变量子集）
    MarkdownValidation(String),
    /// 输入非法（空锚点 / 空标题等）
    InvalidInput(String),
    OperationNotFound(String),
    /// 操作状态不允许请求的动作（如已 reject 再 apply）
    InvalidOperationState(String),
    /// 不属于当前项目（范围闸门；与只读工具同口径防成员信息泄漏）
    NotInProject,
    /// 无可撤销的修订
    NothingToUndo,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Doc(e) => write!(f, "{e}"),
            Self::AnchorNotFound => write!(
                f,
                "锚点文本未在文档正文中匹配到，可能已被修改——请重新读取文档"
            ),
            Self::AnchorAmbiguous(n) => write!(
                f,
                "锚点文本在正文中匹配到 {n} 处，唯一性不足，已停止——请提供更长的锚点或重新读取文档"
            ),
            Self::MarkdownValidation(msg) => write!(f, "Markdown 结构校验失败：{msg}"),
            Self::InvalidInput(msg) => write!(f, "{msg}"),
            Self::OperationNotFound(id) => write!(f, "操作不存在: {id}"),
            Self::InvalidOperationState(msg) => write!(f, "{msg}"),
            Self::NotInProject => write!(f, "该文档不属于当前项目或不存在，无法操作"),
            Self::NothingToUndo => write!(f, "没有可撤销的修订"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<DocError> for ServiceError {
    fn from(e: DocError) -> Self {
        Self::Doc(e)
    }
}

impl From<rusqlite::Error> for ServiceError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Doc(DocError::Db(e.to_string()))
    }
}

impl From<anchor::AnchorError> for ServiceError {
    fn from(e: anchor::AnchorError) -> Self {
        match e {
            anchor::AnchorError::Empty => {
                Self::InvalidInput("锚点文本 selectedText 不能为空".to_string())
            }
            anchor::AnchorError::HashMismatch => Self::InvalidInput(
                "selectedText 与 selectedTextHash 不一致（选区锚点已失效，请重新捕获选区）"
                    .to_string(),
            ),
            anchor::AnchorError::NotFound => Self::AnchorNotFound,
            anchor::AnchorError::Ambiguous(n) => Self::AnchorAmbiguous(n),
        }
    }
}

// ------------------- 数据类型（命令层/工具层共享，camelCase 序列化） -------------------

/// 行级 diff 块（AG-24 单 hunk → AG-26 多 hunk：patience 式唯一行锚点分割，
/// 支持逐 hunk 部分批准；hunk 升序、互不重叠，start_line 为旧正文绝对行号）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PatchHunk {
    /// 0-based：旧正文中首个被删行的行号（纯插入 hunk = 插入点行号，removed 为空）
    pub start_line: usize,
    pub context_before: Vec<String>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub context_after: Vec<String>,
}

/// dry-run 预览结果（「先 dry-run；预览后确认保存」的前半段）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPreview {
    pub operation_id: String,
    pub approval_id: Option<String>,
    pub document_id: String,
    pub title: String,
    pub base_version: i64,
    pub target_version: i64,
    /// 匹配到的原文（= 锚点）
    pub old_text: String,
    /// 替换后的新文本
    pub new_text: String,
    pub hunks: Vec<PatchHunk>,
    /// pending_approval = 等待用户批准；committed = 幂等重入命中已提交操作
    pub status: String,
    /// 范围语义（AG-25）；None = AG-24 文本锚点路径
    pub scope: Option<PatchScope>,
    /// true = 安全 rebase：预览时版本已变化但锚点仍有效，按当前版本重出 diff（docs/architecture.md）
    pub rebased: bool,
    /// NEXT-042：同一审批内的标题改提案（None = 仅正文变更）。
    /// 整块批准时随正文一次写盘；部分批准不改标题。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_title: Option<String>,
}

/// 项目维度的 patch 操作条目（AG-26 审计/重启重建）：预览全字段 + 操作终局状态。
/// 前端重启后凭此重建审批卡（proposed）与已解决态展示/撤销入口（committed/rejected/failed）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPatchEntry {
    #[serde(flatten)]
    pub preview: PatchPreview,
    /// 操作原始状态：proposed/prepared/committed/rejected/failed/rolled_back
    pub op_status: String,
    /// 失败原因（仅 failed/rolled_back 有值）
    pub error: Option<String>,
    /// 部分批准时实际应用的 hunk 下标子集（committed 后回填；全量批准 = None）
    pub applied_hunks: Option<Vec<usize>>,
    /// 提案创建时间（毫秒时间戳；审批卡排序与「何时提议」审计）
    pub created_at: u64,
    /// 该 patch 仍是文档最新 revision，可以精确撤销。
    pub undoable: bool,
    /// 不可撤销时的稳定原因；前端不得再通过版本猜测。
    pub undo_unavailable_reason: Option<String>,
}

/// 应用/撤销结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub document_id: String,
    pub version: i64,
    pub revision_id: Option<String>,
    /// true = 幂等重入：操作此前已提交，本次调用未产生任何写入
    pub already_committed: bool,
    /// NEXT-042：本次应用实际写盘的新标题（None = 未改标题）。
    /// 前端凭此同步侧边栏，无需全库刷新。
    pub applied_title: Option<String>,
}

/// 建文档结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedDocument {
    pub article_id: String,
    pub title: String,
    pub version: i64,
}

/// Patch 范围语义（docs/architecture.md 优先级：显式选区 > 当前块 > 指定章节 > 显式整篇）。
/// 整篇（R3）刻意不在本枚举：独立高风险工具，默认不向模型开放（docs/architecture.md）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchScope {
    Selection,
    CurrentBlock,
    Section,
}

impl PatchScope {
    /// 中文措辞（审批摘要 / Worklog，稳定口径）
    pub fn label(self) -> &'static str {
        match self {
            Self::Selection => "选区",
            Self::CurrentBlock => "当前块",
            Self::Section => "章节",
        }
    }
}

/// 选区感知的 patch 请求（AG-25：绑定 document_id/base_version/scope/anchor/hash，
/// docs/architecture.md document.patch 契约）
#[derive(Debug)]
pub struct ScopedPatchRequest {
    pub document_id: String,
    pub base_version: i64,
    pub scope: PatchScope,
    pub anchor: anchor::TextAnchor,
    pub replacement_markdown: String,
    pub idempotency_key: Option<String>,
    pub run_id: Option<String>,
    pub project_gate: Option<String>,
}

/// 操作日志行（document_operations 的内存镜像）
#[derive(Debug)]
struct OperationRow {
    id: String,
    #[allow(dead_code)]
    idempotency_key: Option<String>,
    document_id: String,
    operation_type: String,
    base_version: i64,
    target_version: Option<i64>,
    status: String,
    #[allow(dead_code)]
    error: Option<String>,
    #[allow(dead_code)]
    tmp_path: Option<String>,
    approval_id: Option<String>,
    run_id: Option<String>,
    payload_json: Option<String>,
    #[allow(dead_code)]
    created_at: u64,
    #[allow(dead_code)]
    updated_at: u64,
}

/// patch 操作载荷（落 document_operations.payload_json；apply 时刻重新执行锚点匹配，
/// 不信任「存好的新正文」——以当前正文 + 原文锚点重推导，防陈旧快照覆盖）
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchPayload {
    expected_text: String,
    replacement_markdown: String,
    hunks: Vec<PatchHunk>,
    /// AG-25 选区感知字段（serde default：兼容 AG-24 既有行 = None/None/false）
    #[serde(default)]
    scope: Option<PatchScope>,
    #[serde(default)]
    anchor: Option<anchor::TextAnchor>,
    #[serde(default)]
    rebased: bool,
    /// Host 从 Hermes Session 工作副本回收的整篇文档提案。该字段只由
    /// SophoNote Host 写入，模型侧工具不暴露 scope=document。
    #[serde(default)]
    whole_document: bool,
    /// AG-26：部分批准时实际应用的 hunk 下标子集（apply 时刻回填，审计「批准了哪些块」）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_hunks: Option<Vec<usize>>,
    /// NEXT-042：Host 工作副本回收时同一审批携带的标题改提案（模型编辑了工作副本
    /// 的标题区）。serde default = 兼容既有操作行；仅 apply_patch 整块批准时写盘，
    /// 部分批准（apply_patch_partial）不改标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proposed_title: Option<String>,
}

/// create 操作载荷（幂等重入返回原 articleId）
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePayload {
    article_id: String,
    title: String,
}

const HUNK_CONTEXT_LINES: usize = 2;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ------------------- 内部辅助 -------------------

fn load_document(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    id: &str,
) -> Result<DocumentRecord, ServiceError> {
    match repository::get_document(conn, notes_dir, id)? {
        Some(r) if r.file_exists => Ok(r),
        Some(_) => Err(ServiceError::Doc(DocError::NotFound(format!(
            "{id}（文件未落盘）"
        )))),
        None => Err(ServiceError::Doc(DocError::NotFound(id.to_string()))),
    }
}

fn is_project_member(
    conn: &rusqlite::Connection,
    project_id: &str,
    article_id: &str,
) -> Result<bool, ServiceError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1 AND article_id = ?2",
        rusqlite::params![project_id, article_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn find_operation_by_key(
    conn: &rusqlite::Connection,
    key: &str,
) -> Result<Option<OperationRow>, ServiceError> {
    let mut stmt = conn.prepare(
        "SELECT id, idempotency_key, document_id, operation_type, base_version, target_version,
                status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at
         FROM document_operations WHERE idempotency_key = ?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![key], map_operation_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

fn get_operation(
    conn: &rusqlite::Connection,
    operation_id: &str,
) -> Result<Option<OperationRow>, ServiceError> {
    conn.query_row(
        "SELECT id, idempotency_key, document_id, operation_type, base_version, target_version,
                status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at
         FROM document_operations WHERE id = ?1",
        rusqlite::params![operation_id],
        map_operation_row,
    )
    .optional()
    .map_err(|e| ServiceError::Doc(DocError::Db(e.to_string())))
}

fn map_operation_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRow> {
    Ok(OperationRow {
        id: r.get(0)?,
        idempotency_key: r.get(1)?,
        document_id: r.get(2)?,
        operation_type: r.get(3)?,
        base_version: r.get(4)?,
        target_version: r.get(5)?,
        status: r.get(6)?,
        error: r.get(7)?,
        tmp_path: r.get(8)?,
        approval_id: r.get(9)?,
        run_id: r.get(10)?,
        payload_json: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

/// 状态机推进（单一更新点；updated_at 恒真实时间戳，AG-22 同款口径）
fn set_operation_status(
    conn: &rusqlite::Connection,
    id: &str,
    status: &str,
    error: Option<&str>,
    tmp_path: Option<&str>,
    target_version: Option<i64>,
) -> Result<(), ServiceError> {
    conn.execute(
        "UPDATE document_operations
         SET status = ?2,
             error = COALESCE(?3, error),
             tmp_path = COALESCE(?4, tmp_path),
             target_version = COALESCE(?5, target_version),
             updated_at = ?6
         WHERE id = ?1",
        rusqlite::params![id, status, error, tmp_path, target_version, now_ms()],
    )?;
    Ok(())
}

fn create_approval_row(
    conn: &rusqlite::Connection,
    approval_id: &str,
    run_id: &str,
    resource_summary: &str,
) -> Result<(), ServiceError> {
    conn.execute(
        "INSERT OR REPLACE INTO agent_approvals
         (id, run_id, approval_type, status, resource_summary, created_at, resolved_at)
         VALUES (?1, ?2, 'document_patch', 'pending', ?3, ?4, NULL)",
        rusqlite::params![approval_id, run_id, resource_summary, now_ms()],
    )?;
    Ok(())
}

fn resolve_approval_row(
    conn: &rusqlite::Connection,
    approval_id: &str,
    decision: &str,
) -> Result<(), ServiceError> {
    conn.execute(
        "UPDATE agent_approvals SET status = ?2, resolved_at = ?3
         WHERE id = ?1 AND status = 'pending'",
        rusqlite::params![approval_id, decision, now_ms()],
    )?;
    Ok(())
}

fn rebuild_preview_from_operation(
    conn: &rusqlite::Connection,
    op: &OperationRow,
) -> Result<PatchPreview, ServiceError> {
    let payload: PatchPayload = serde_json::from_str(op.payload_json.as_deref().unwrap_or("{}"))
        .map_err(|e| ServiceError::InvalidOperationState(format!("操作载荷损坏: {e}")))?;
    let title = load_title(conn, &op.document_id)?;
    let status = if op.status == "committed" {
        "committed"
    } else {
        "pending_approval"
    };
    Ok(PatchPreview {
        operation_id: op.id.clone(),
        approval_id: op.approval_id.clone(),
        document_id: op.document_id.clone(),
        title,
        base_version: op.base_version,
        target_version: op.target_version.unwrap_or(op.base_version + 1),
        old_text: payload.expected_text,
        new_text: payload.replacement_markdown,
        hunks: payload.hunks,
        status: status.to_string(),
        scope: payload.scope,
        rebased: payload.rebased,
        proposed_title: payload.proposed_title,
    })
}

fn load_title(conn: &rusqlite::Connection, document_id: &str) -> Result<String, ServiceError> {
    conn.query_row(
        "SELECT title FROM articles WHERE id = ?1",
        rusqlite::params![document_id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .ok_or_else(|| ServiceError::Doc(DocError::NotFound(document_id.to_string())))
}

/// 提交正文变更的共享写流程（调用方必须已持有单文档锁且已复检版本/锚点）：
/// 唯一 tmp（带 operation ID）→ prepared → 原子 rename → CAS 递增 → revision 快照 → committed。
/// `new_title` = Some 时标题随正文同一次写盘落文件 + 同一条 CAS UPDATE 落库
/// （NEXT-042：标题与正文要么一起生效、要么都不生效，绝不出现中间态）。
/// 返回 (新版本号, 修订 id)。
fn commit_body_change(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    rec: &DocumentRecord,
    new_body: &str,
    new_title: Option<&str>,
    op_id: &str,
    run_id: Option<&str>,
) -> Result<(i64, String), ServiceError> {
    let final_title = new_title.unwrap_or(rec.title.as_str());
    let safe_id = crate::notes::safe_article_id(&rec.id);
    let tmp_path = notes_dir.join(format!("{safe_id}.{op_id}.tmp"));
    let final_path = notes_dir.join(format!("{safe_id}.md"));

    set_operation_status(
        conn,
        op_id,
        "prepared",
        None,
        Some(&tmp_path.to_string_lossy()),
        None,
    )?;

    let article = crate::db::Article {
        id: rec.id.clone(),
        item_id: rec.item_id.clone(),
        title: final_title.to_string(),
        content: new_body.to_string(),
        article_type: rec.article_type.clone(),
        edited: true,
        created_at: rec.created_at.clone(),
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        prompt_version: rec.prompt_version.clone(),
        blocks_json: None,
    };
    if let Err(e) = crate::notes::write_article_to(&tmp_path, &article) {
        set_operation_status(conn, op_id, "failed", Some(&e.to_string()), None, None)?;
        return Err(ServiceError::Doc(DocError::Io(e.to_string())));
    }
    if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        set_operation_status(conn, op_id, "rolled_back", Some(&e.to_string()), None, None)?;
        return Err(ServiceError::Doc(DocError::Io(e.to_string())));
    }

    // CAS 递增（防御性最后一道：单文档锁已排除并发，理论上不会失败）
    // 标题与正文在同一条 UPDATE 内生效：文件已按 final_title 写出，
    // DB 若不同步会造成 frontmatter 与侧边栏标题分裂（NEXT-042）。
    let changed = conn.execute(
        "UPDATE articles SET title = ?3, content = '', edited = 1, updated_at = CURRENT_TIMESTAMP,
                             version = version + 1
         WHERE id = ?1 AND version = ?2",
        rusqlite::params![rec.id, rec.version, final_title],
    )?;
    if changed == 0 {
        set_operation_status(
            conn,
            op_id,
            "failed",
            Some("版本 CAS 失败（并发写入）"),
            None,
            None,
        )?;
        return Err(ServiceError::Doc(DocError::VersionConflict {
            expected: rec.version,
            actual: repository::current_version(conn, &rec.id)?.unwrap_or(-1),
        }));
    }
    let new_version = rec.version + 1;

    // 修订快照 = 旧正文全量（undo 真相源；version = 本次写入产生的新版本号）
    let revision_id = format!("rev-{}", uuid::Uuid::new_v4());
    conn.execute(
        "INSERT INTO document_revisions
         (id, document_id, version, content_hash, content_snapshot, operation_id, run_id, tool_call_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
        rusqlite::params![
            revision_id,
            rec.id,
            new_version,
            repository::content_hash(&rec.body),
            rec.body,
            op_id,
            run_id,
            now_ms()
        ],
    )?;
    set_operation_status(conn, op_id, "committed", None, None, Some(new_version))?;
    Ok((new_version, revision_id))
}

// ------------------- 公开 API -------------------

/// dry-run 预览：生成 diff 提案，**不写任何文件**（审计批次 5 第 3 步）。
///
/// project_gate = Some(pid) 时做归属校验（工具路径）；命令路径传 None
/// （用户对自己的数据无隔离需求）。run_id = Some 时同步创建审批请求。
#[allow(clippy::too_many_arguments)]
pub fn preview_patch(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    document_id: &str,
    base_version: i64,
    expected_text: &str,
    replacement_markdown: &str,
    idempotency_key: Option<&str>,
    run_id: Option<&str>,
    project_gate: Option<&str>,
) -> Result<PatchPreview, ServiceError> {
    // 0) 幂等重入：同键返回原提案/提交结果，不建第二份提案
    if let Some(key) = idempotency_key {
        if let Some(existing) = find_operation_by_key(conn, key)? {
            return rebuild_preview_from_operation(conn, &existing);
        }
    }
    // 1) 范围闸门（工具路径）
    if let Some(pid) = project_gate {
        if !is_project_member(conn, pid, document_id)? {
            return Err(ServiceError::NotInProject);
        }
    }
    // 2) 读文档 + 版本检查
    let rec = load_document(conn, notes_dir, document_id)?;
    if rec.version != base_version {
        return Err(ServiceError::Doc(DocError::VersionConflict {
            expected: base_version,
            actual: rec.version,
        }));
    }
    // 3) 锚点唯一性（0 匹配 / 多匹配 = 冲突停止，不猜测）
    if expected_text.is_empty() {
        return Err(ServiceError::InvalidInput(
            "锚点文本 expectedText 不能为空".to_string(),
        ));
    }
    let matches = rec.body.match_indices(expected_text).count();
    match matches {
        0 => return Err(ServiceError::AnchorNotFound),
        n if n > 1 => return Err(ServiceError::AnchorAmbiguous(n)),
        _ => {}
    }
    let new_body = rec.body.replacen(expected_text, replacement_markdown, 1);
    let hunks = compute_hunks(&rec.body, &new_body);

    // 4) 操作日志（proposed）+ Run 上下文时同步审批请求
    let op_id = format!("op-{}", uuid::Uuid::new_v4());
    let payload = PatchPayload {
        expected_text: expected_text.to_string(),
        replacement_markdown: replacement_markdown.to_string(),
        hunks: hunks.clone(),
        scope: None,
        anchor: None,
        rebased: false,
        whole_document: false,
        applied_hunks: None,
        proposed_title: None,
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| ServiceError::InvalidOperationState(format!("载荷序列化失败: {e}")))?;
    let approval_id = if let Some(rid) = run_id {
        let aid = format!("apr-{}", uuid::Uuid::new_v4());
        create_approval_row(
            conn,
            &aid,
            rid,
            &format!(
                "修改文档《{}》：替换 {} 字符为 {} 字符",
                rec.title,
                expected_text.chars().count(),
                replacement_markdown.chars().count()
            ),
        )?;
        Some(aid)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'patch', ?4, ?5, 'proposed', NULL, NULL, ?6, ?7, ?8, ?9, ?9)",
        rusqlite::params![
            op_id,
            idempotency_key,
            document_id,
            base_version,
            base_version + 1,
            approval_id,
            run_id,
            payload_json,
            now_ms()
        ],
    )?;

    Ok(PatchPreview {
        operation_id: op_id,
        approval_id,
        document_id: document_id.to_string(),
        title: rec.title,
        base_version,
        target_version: base_version + 1,
        old_text: expected_text.to_string(),
        new_text: replacement_markdown.to_string(),
        hunks,
        status: "pending_approval".into(),
        scope: None,
        rebased: false,
        proposed_title: None,
    })
}

/// dry-run 预览（选区感知，AG-25）：绑定 scope + TextAnchor（docs/architecture.md document.patch 契约）。
///
/// 与 `preview_patch` 相同的铁律（零文件写入 / 幂等 / 审批），外加两条 AG-25 语义：
/// - **范围收敛**：替换只发生在锚点解析出的唯一字节范围，范围外正文逐字保留；
/// - **安全 rebase**（docs/architecture.md）：base_version 已变化但锚点在最新正文仍能唯一解析
///   且目标 hash 未变 → 按当前版本重出 diff（`rebased=true`）；否则进 conflict，
///   绝不静默覆盖。
/// - **结构校验**：Markdown round-trip 可替换结构不变量子集（围栏边界/配对/伪 frontmatter）。
pub fn preview_scoped_patch(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    req: &ScopedPatchRequest,
) -> Result<PatchPreview, ServiceError> {
    // 0) 幂等重入：同键返回原提案/提交结果，不建第二份提案
    if let Some(key) = &req.idempotency_key {
        if let Some(existing) = find_operation_by_key(conn, key)? {
            return rebuild_preview_from_operation(conn, &existing);
        }
    }
    // 1) 范围闸门（工具路径）
    if let Some(pid) = &req.project_gate {
        if !is_project_member(conn, pid, &req.document_id)? {
            return Err(ServiceError::NotInProject);
        }
    }
    // 2) 读文档 + 版本检查 / 安全 rebase
    let rec = load_document(conn, notes_dir, &req.document_id)?;
    let mut rebased = false;
    let base_version;
    let range: (usize, usize);
    if rec.version == req.base_version {
        base_version = req.base_version;
        // 3) 锚点唯一性（hash 校验 + 上下文消歧；0/多 = 冲突停止，不猜测）
        range = anchor::resolve_anchor(&rec.body, &req.anchor)?;
    } else {
        // 版本已变化：仅当锚点在最新正文仍唯一解析（hash 未变）才安全 rebase
        match anchor::resolve_anchor(&rec.body, &req.anchor) {
            Ok(r) => {
                rebased = true;
                base_version = rec.version;
                range = r;
            }
            Err(_) => {
                return Err(ServiceError::Doc(DocError::VersionConflict {
                    expected: req.base_version,
                    actual: rec.version,
                }))
            }
        }
    }
    // 4) Markdown 结构校验（round-trip 可替换子集）
    anchor::validate_patch_structure(&rec.body, range, &req.replacement_markdown)
        .map_err(ServiceError::MarkdownValidation)?;
    let (start, end) = range;
    let new_body = format!(
        "{}{}{}",
        &rec.body[..start],
        req.replacement_markdown,
        &rec.body[end..]
    );
    let hunks = compute_hunks(&rec.body, &new_body);

    // 5) 操作日志（proposed）+ Run 上下文时同步审批请求
    let op_id = format!("op-{}", uuid::Uuid::new_v4());
    let payload = PatchPayload {
        expected_text: req.anchor.selected_text.clone(),
        replacement_markdown: req.replacement_markdown.clone(),
        hunks: hunks.clone(),
        scope: Some(req.scope),
        anchor: Some(req.anchor.clone()),
        rebased,
        whole_document: false,
        applied_hunks: None,
        proposed_title: None,
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| ServiceError::InvalidOperationState(format!("载荷序列化失败: {e}")))?;
    let approval_id = if let Some(rid) = &req.run_id {
        let aid = format!("apr-{}", uuid::Uuid::new_v4());
        create_approval_row(
            conn,
            &aid,
            rid,
            &format!(
                "修改文档《{}》（{}）：替换 {} 字符为 {} 字符",
                rec.title,
                req.scope.label(),
                req.anchor.selected_text.chars().count(),
                req.replacement_markdown.chars().count()
            ),
        )?;
        Some(aid)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'patch', ?4, ?5, 'proposed', NULL, NULL, ?6, ?7, ?8, ?9, ?9)",
        rusqlite::params![
            op_id,
            req.idempotency_key,
            req.document_id,
            base_version,
            base_version + 1,
            approval_id,
            req.run_id,
            payload_json,
            now_ms()
        ],
    )?;

    Ok(PatchPreview {
        operation_id: op_id,
        approval_id,
        document_id: req.document_id.clone(),
        title: rec.title,
        base_version,
        target_version: base_version + 1,
        old_text: req.anchor.selected_text.clone(),
        new_text: req.replacement_markdown.clone(),
        hunks,
        status: "pending_approval".into(),
        scope: Some(req.scope),
        rebased,
        proposed_title: None,
    })
}

/// Hermes Client Surface 当前文档工作副本的 Host-side dry-run。
///
/// 这不是模型工具：模型只能编辑 Session 暂存副本；Host 在回合终态校验副本
/// 后调用本入口，把整篇差异转换为普通多 hunk Patch。实际落盘仍只能由
/// `document_apply_patch` 在用户逐 hunk 决策后完成。
#[allow(clippy::too_many_arguments)]
pub fn preview_host_document_patch(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    document_id: &str,
    base_version: i64,
    expected_markdown: &str,
    replacement_markdown: &str,
    idempotency_key: Option<&str>,
    run_id: Option<&str>,
    proposed_title: Option<&str>,
) -> Result<PatchPreview, ServiceError> {
    if let Some(key) = idempotency_key {
        if let Some(existing) = find_operation_by_key(conn, key)? {
            return rebuild_preview_from_operation(conn, &existing);
        }
    }
    let rec = load_document(conn, notes_dir, document_id)?;
    if rec.version != base_version {
        return Err(ServiceError::Doc(DocError::VersionConflict {
            expected: base_version,
            actual: rec.version,
        }));
    }
    // NEXT-042：标题改提案与正文 Patch 合并为同一审批（格式/同名 fail-closed 校验）。
    // 与当前标题相同 → 归一为 None，不产生无意义的改名语义。
    let proposed_title = match proposed_title {
        Some(raw) => validate_proposed_title(conn, document_id, &rec.title, raw)?,
        None => None,
    };
    if rec.body != expected_markdown {
        return Err(ServiceError::InvalidOperationState(
            "发送后的编辑器草稿尚未与 DocumentService 基线一致，已停止生成 Patch，请保存后重试"
                .to_string(),
        ));
    }
    if rec.body == replacement_markdown {
        return Err(ServiceError::InvalidInput(
            "Hermes 工作副本与当前文档没有差异".to_string(),
        ));
    }
    // 整篇替换不复用“跨围栏选区”判断，只验证新正文自身不会破坏围栏与
    // frontmatter 边界。原正文在 apply 时再由 baseVersion 做 CAS 复检。
    anchor::validate_patch_structure("", (0, 0), replacement_markdown)
        .map_err(ServiceError::MarkdownValidation)?;
    let hunks = compute_hunks(&rec.body, replacement_markdown);
    if hunks.is_empty() {
        return Err(ServiceError::InvalidInput(
            "Hermes 工作副本没有可应用的 Markdown 变更".to_string(),
        ));
    }

    let op_id = format!("op-{}", uuid::Uuid::new_v4());
    let payload = PatchPayload {
        expected_text: rec.body.clone(),
        replacement_markdown: replacement_markdown.to_string(),
        hunks: hunks.clone(),
        scope: None,
        anchor: None,
        rebased: false,
        whole_document: true,
        applied_hunks: None,
        proposed_title: proposed_title.clone(),
    };
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| ServiceError::InvalidOperationState(format!("载荷序列化失败: {e}")))?;
    let approval_id = if let Some(rid) = run_id {
        let aid = format!("apr-{}", uuid::Uuid::new_v4());
        let label = match &proposed_title {
            Some(new_title) => format!(
                "修改当前文档《{}》（{} 个变更块），并重命名为《{}》",
                rec.title,
                hunks.len(),
                new_title
            ),
            None => format!("修改当前文档《{}》（{} 个变更块）", rec.title, hunks.len()),
        };
        create_approval_row(conn, &aid, rid, &label)?;
        Some(aid)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'patch', ?4, ?5, 'proposed', NULL, NULL, ?6, ?7, ?8, ?9, ?9)",
        rusqlite::params![
            op_id,
            idempotency_key,
            document_id,
            base_version,
            base_version + 1,
            approval_id,
            run_id,
            payload_json,
            now_ms()
        ],
    )?;

    Ok(PatchPreview {
        operation_id: op_id,
        approval_id,
        document_id: document_id.to_string(),
        title: rec.title,
        base_version,
        target_version: base_version + 1,
        old_text: rec.body,
        new_text: replacement_markdown.to_string(),
        hunks,
        status: "pending_approval".into(),
        scope: None,
        rebased: false,
        proposed_title,
    })
}

/// NEXT-042：标题改提案的统一校验（Host 工作副本合并路径与独立 rename 卡同口径）：
/// trim 后 1–200 字符、不含换行、与当前标题不同；文档所属项目内不得已存在同名文档。
/// 与当前标题相同 → Ok(None)（无需提案）；不合法 → InvalidInput（fail-closed，不猜测）。
pub fn validate_proposed_title(
    conn: &rusqlite::Connection,
    document_id: &str,
    current_title: &str,
    proposed: &str,
) -> Result<Option<String>, ServiceError> {
    let title = proposed.trim();
    if title.is_empty() || title == current_title {
        return Ok(None);
    }
    if title.contains('\n') || title.contains('\r') || title.chars().count() > 200 {
        return Err(ServiceError::InvalidInput(
            "标题必须为 1–200 个字符且不能包含换行".to_string(),
        ));
    }
    let project_id: Option<String> = conn
        .query_row(
            "SELECT project_id FROM project_documents WHERE article_id = ?1",
            rusqlite::params![document_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(project_id) = project_id {
        let duplicate: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_documents d
                 JOIN articles a ON a.id = d.article_id
                 WHERE d.project_id = ?1 AND d.article_id != ?2 AND a.title = ?3",
                rusqlite::params![project_id, document_id, title],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if duplicate > 0 {
            return Err(ServiceError::InvalidInput(format!(
                "项目内已存在同名文档《{title}》"
            )));
        }
    }
    Ok(Some(title.to_string()))
}

/// 批准后应用提案（「预览后确认保存」的后半段）：
/// 锁内复检 base_version 与锚点唯一性 → 唯一 tmp → rename → CAS 递增 → revision。
/// 幂等：已 committed 的操作重入直接返回原结果，零写入。
pub fn apply_patch(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    operation_id: &str,
) -> Result<ApplyResult, ServiceError> {
    let op = get_operation(conn, operation_id)?
        .ok_or_else(|| ServiceError::OperationNotFound(operation_id.to_string()))?;
    if op.status == "committed" {
        return Ok(ApplyResult {
            document_id: op.document_id,
            version: op.target_version.unwrap_or(op.base_version + 1),
            revision_id: None,
            already_committed: true,
            applied_title: None,
        });
    }
    if op.status != "proposed" {
        return Err(ServiceError::InvalidOperationState(format!(
            "操作状态为 {}，无法应用（仅 proposed 可批准应用）",
            op.status
        )));
    }
    let payload: PatchPayload = serde_json::from_str(op.payload_json.as_deref().unwrap_or("{}"))
        .map_err(|e| ServiceError::InvalidOperationState(format!("操作载荷损坏: {e}")))?;

    let _guard = repository::document_lock(&op.document_id);
    // 复检①：预览→批准之间用户改过文档 → 版本冲突，绝不静默覆盖
    let rec = load_document(conn, notes_dir, &op.document_id)?;
    if rec.version != op.base_version {
        set_operation_status(
            conn,
            &op.id,
            "failed",
            Some(&format!(
                "应用时版本冲突：base {} 已过期，当前 {}",
                op.base_version, rec.version
            )),
            None,
            None,
        )?;
        return Err(ServiceError::Doc(DocError::VersionConflict {
            expected: op.base_version,
            actual: rec.version,
        }));
    }
    // 复检②：锚点重新验证——选区感知操作用全锚点（hash + 上下文消歧，不猜测），
    // AG-24 文本锚点路径保持唯一性计数口径。防版本未变但结构恰好变化（如围栏移动）
    let new_body = if payload.whole_document {
        anchor::validate_patch_structure("", (0, 0), &payload.replacement_markdown)
            .map_err(ServiceError::MarkdownValidation)?;
        payload.replacement_markdown.clone()
    } else {
        match &payload.anchor {
            Some(a) => {
                let range = match anchor::resolve_anchor(&rec.body, a) {
                    Ok(r) => r,
                    Err(e) => {
                        set_operation_status(
                            conn,
                            &op.id,
                            "failed",
                            Some("应用时锚点复检失败（目标已变化或唯一性不足）"),
                            None,
                            None,
                        )?;
                        return Err(e.into());
                    }
                };
                if let Err(reason) = anchor::validate_patch_structure(
                    &rec.body,
                    range,
                    &payload.replacement_markdown,
                ) {
                    set_operation_status(
                        conn,
                        &op.id,
                        "failed",
                        Some(&format!("应用时结构校验失败: {reason}")),
                        None,
                        None,
                    )?;
                    return Err(ServiceError::MarkdownValidation(reason));
                }
                format!(
                    "{}{}{}",
                    &rec.body[..range.0],
                    payload.replacement_markdown,
                    &rec.body[range.1..]
                )
            }
            None => {
                let matches = rec.body.match_indices(&payload.expected_text).count();
                if matches == 0 {
                    set_operation_status(
                        conn,
                        &op.id,
                        "failed",
                        Some("应用时锚点未匹配"),
                        None,
                        None,
                    )?;
                    return Err(ServiceError::AnchorNotFound);
                }
                if matches > 1 {
                    set_operation_status(
                        conn,
                        &op.id,
                        "failed",
                        Some(&format!("应用时锚点匹配 {matches} 处")),
                        None,
                        None,
                    )?;
                    return Err(ServiceError::AnchorAmbiguous(matches));
                }
                rec.body
                    .replacen(&payload.expected_text, &payload.replacement_markdown, 1)
            }
        }
    };

    let (new_version, revision_id) = commit_body_change(
        conn,
        notes_dir,
        &rec,
        &new_body,
        payload.proposed_title.as_deref(),
        &op.id,
        op.run_id.as_deref(),
    )?;
    if let Some(aid) = &op.approval_id {
        resolve_approval_row(conn, aid, "approved")?;
    }
    Ok(ApplyResult {
        document_id: op.document_id,
        version: new_version,
        revision_id: Some(revision_id),
        already_committed: false,
        applied_title: payload.proposed_title,
    })
}

/// AG-26 逐 hunk 部分批准：只应用 approved_hunks 指定的 hunk 子集（0-based 下标），
/// 被拒绝的 hunk 不写入（docs/architecture.md）。铁律同 apply_patch：
/// 锁内复检版本 → 每个被批准 hunk 的 removed 行与当前正文逐行比对（不匹配 = 冲突停止，
/// 绝不猜测）→ 结构不变量复检（围栏配对保持 / 伪 frontmatter 防护）→ 原子写入。
/// 部分批准的子集记录回 payload.applied_hunks（重启后可审计「批准了哪些块」）。
pub fn apply_patch_partial(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    operation_id: &str,
    approved_hunks: &[usize],
) -> Result<ApplyResult, ServiceError> {
    let op = get_operation(conn, operation_id)?
        .ok_or_else(|| ServiceError::OperationNotFound(operation_id.to_string()))?;
    if op.status == "committed" {
        return Ok(ApplyResult {
            document_id: op.document_id,
            version: op.target_version.unwrap_or(op.base_version + 1),
            revision_id: None,
            already_committed: true,
            applied_title: None,
        });
    }
    if op.status != "proposed" {
        return Err(ServiceError::InvalidOperationState(format!(
            "操作状态为 {}，无法应用（仅 proposed 可批准应用）",
            op.status
        )));
    }
    let payload: PatchPayload = serde_json::from_str(op.payload_json.as_deref().unwrap_or("{}"))
        .map_err(|e| ServiceError::InvalidOperationState(format!("操作载荷损坏: {e}")))?;
    if payload.hunks.is_empty() {
        return Err(ServiceError::InvalidOperationState(
            "提案没有可批准的变更块（diff 为空）".to_string(),
        ));
    }
    // 子集归一：去重 + 升序；越界/空集显式报错
    let mut selected: Vec<usize> = approved_hunks.to_vec();
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        return Err(ServiceError::InvalidInput(
            "至少批准一个变更块；全部拒绝请使用拒绝操作".to_string(),
        ));
    }
    if let Some(&bad) = selected.iter().find(|&&i| i >= payload.hunks.len()) {
        return Err(ServiceError::InvalidInput(format!(
            "变更块下标 {bad} 越界（共 {} 个）",
            payload.hunks.len()
        )));
    }
    if selected.len() == payload.hunks.len() {
        // 全量批准 → 走既有完整路径（锚点复检口径完全一致）
        return apply_patch(conn, notes_dir, operation_id);
    }

    let _guard = repository::document_lock(&op.document_id);
    // 复检①：版本一致（预览→批准之间用户改过文档 → 冲突，绝不静默覆盖）
    let rec = load_document(conn, notes_dir, &op.document_id)?;
    if rec.version != op.base_version {
        set_operation_status(
            conn,
            &op.id,
            "failed",
            Some(&format!(
                "应用时版本冲突：base {} 已过期，当前 {}",
                op.base_version, rec.version
            )),
            None,
            None,
        )?;
        return Err(ServiceError::Doc(DocError::VersionConflict {
            expected: op.base_version,
            actual: rec.version,
        }));
    }
    // 复检②：每个被批准 hunk 的 removed 行与当前正文逐行一致（防文件被外部修改；
    // 版本一致时理论必然通过——never-guess 纵深防御，与全量路径锚点复检同构），
    // 且逐 hunk 结构校验（与预览同规则：围栏不跨界/围栏内纯净/围栏外配对/伪 frontmatter）
    let body_lines: Vec<&str> = rec.body.split('\n').collect();
    let mut line_byte_starts: Vec<usize> = Vec::with_capacity(body_lines.len());
    {
        let mut pos = 0usize;
        for line in body_lines.iter() {
            line_byte_starts.push(pos);
            pos += line.len() + 1;
        }
    }
    for &hi in &selected {
        let h = &payload.hunks[hi];
        let end = h.start_line + h.removed.len();
        if end > body_lines.len() || body_lines[h.start_line..end] != h.removed[..] {
            set_operation_status(
                conn,
                &op.id,
                "failed",
                Some(&format!(
                    "应用时变更块 {} 的原文行与正文不一致（文档可能已被修改），已停止",
                    hi + 1
                )),
                None,
                None,
            )?;
            return Err(ServiceError::InvalidOperationState(format!(
                "变更块 {} 应用失败：原文行与当前正文不一致，请重新提议",
                hi + 1
            )));
        }
        let bstart = line_byte_starts
            .get(h.start_line)
            .copied()
            .unwrap_or(rec.body.len());
        let bend = if h.removed.is_empty() {
            bstart // 纯插入：空范围
        } else {
            line_byte_starts[end - 1] + body_lines[end - 1].len()
        };
        if let Err(reason) =
            anchor::validate_patch_structure(&rec.body, (bstart, bend), &h.added.join("\n"))
        {
            set_operation_status(
                conn,
                &op.id,
                "failed",
                Some(&format!("变更块 {} 结构校验失败: {reason}", hi + 1)),
                None,
                None,
            )?;
            return Err(ServiceError::MarkdownValidation(format!(
                "变更块 {} 结构校验失败：{reason}",
                hi + 1
            )));
        }
    }
    // 行级拼接：按 start_line 降序应用，前序 hunk 行号不受影响
    let mut lines: Vec<String> = body_lines.iter().map(|s| s.to_string()).collect();
    for &hi in selected.iter().rev() {
        let h = &payload.hunks[hi];
        lines.splice(
            h.start_line..h.start_line + h.removed.len(),
            h.added.iter().cloned(),
        );
    }
    let new_body = lines.join("\n");
    // 复检③：结构不变量——子集应用未经过整块预览的结构校验，须独立复检
    let touches_line0 = selected.iter().any(|&hi| payload.hunks[hi].start_line == 0);
    if anchor::fences_balanced(&rec.body) && !anchor::fences_balanced(&new_body) {
        set_operation_status(
            conn,
            &op.id,
            "failed",
            Some("部分应用后代码围栏不再配对，已停止"),
            None,
            None,
        )?;
        return Err(ServiceError::MarkdownValidation(
            "所选变更块部分应用会使代码围栏失去配对，请整块批准或调整选择".to_string(),
        ));
    }
    if touches_line0
        && !rec.body.starts_with("---\n")
        && (new_body == "---" || new_body.starts_with("---\n"))
    {
        set_operation_status(
            conn,
            &op.id,
            "failed",
            Some("部分应用会在正文开头注入 frontmatter 分隔符，已停止"),
            None,
            None,
        )?;
        return Err(ServiceError::MarkdownValidation(
            "替换不得在正文开头注入 frontmatter 分隔符".to_string(),
        ));
    }

    // 记录部分批准子集（审计「批准了哪些块」；重启后凭 payload 重建）
    let mut payload_write = payload;
    payload_write.applied_hunks = Some(selected);
    let payload_json = serde_json::to_string(&payload_write)
        .map_err(|e| ServiceError::InvalidOperationState(format!("载荷序列化失败: {e}")))?;
    conn.execute(
        "UPDATE document_operations SET payload_json = ?2, updated_at = ?3 WHERE id = ?1",
        rusqlite::params![op.id, payload_json, now_ms()],
    )?;

    // 部分批准刻意不应用 proposed_title：标题改是「整块批准」语义的一部分，
    // 子集批准只代表用户接受了部分正文，不能顺带改名（NEXT-042）。
    let (new_version, revision_id) = commit_body_change(
        conn,
        notes_dir,
        &rec,
        &new_body,
        None,
        &op.id,
        op.run_id.as_deref(),
    )?;
    if let Some(aid) = &op.approval_id {
        resolve_approval_row(conn, aid, "approved")?;
    }
    Ok(ApplyResult {
        document_id: op.document_id,
        version: new_version,
        revision_id: Some(revision_id),
        already_committed: false,
        applied_title: None,
    })
}

/// 拒绝提案：操作 → rejected，审批同步 rejected（零文件写入）
pub fn reject_patch(conn: &rusqlite::Connection, operation_id: &str) -> Result<(), ServiceError> {
    let op = get_operation(conn, operation_id)?
        .ok_or_else(|| ServiceError::OperationNotFound(operation_id.to_string()))?;
    if op.status == "committed" {
        return Err(ServiceError::InvalidOperationState(
            "操作已提交，不能拒绝（可用 undo 撤销）".to_string(),
        ));
    }
    if op.status != "proposed" {
        return Err(ServiceError::InvalidOperationState(format!(
            "操作状态为 {}，无法拒绝",
            op.status
        )));
    }
    set_operation_status(conn, &op.id, "rejected", None, None, None)?;
    if let Some(aid) = &op.approval_id {
        resolve_approval_row(conn, aid, "rejected")?;
    }
    Ok(())
}

/// 撤销最近一次修订：还原快照为正文新版本（版本只增不减 → 撤销本身可再撤销 = redo）
pub fn undo_last_change(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    document_id: &str,
    project_gate: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<ApplyResult, ServiceError> {
    // 幂等重入
    if let Some(key) = idempotency_key {
        if let Some(existing) = find_operation_by_key(conn, key)? {
            if existing.status == "committed" {
                return Ok(ApplyResult {
                    document_id: existing.document_id,
                    version: existing.target_version.unwrap_or(existing.base_version),
                    revision_id: None,
                    already_committed: true,
                    applied_title: None,
                });
            }
            // 未完成的同键旧记录（如启动恢复回滚过的）：清掉，避免 UNIQUE 冲突
            conn.execute(
                "DELETE FROM document_operations WHERE id = ?1",
                rusqlite::params![existing.id],
            )?;
        }
    }
    if let Some(pid) = project_gate {
        if !is_project_member(conn, pid, document_id)? {
            return Err(ServiceError::NotInProject);
        }
    }
    let rec = load_document(conn, notes_dir, document_id)?;
    // 最近修订（version 倒序首条）
    let snapshot: Option<String> = conn
        .query_row(
            "SELECT content_snapshot FROM document_revisions
             WHERE document_id = ?1 ORDER BY version DESC LIMIT 1",
            rusqlite::params![document_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    let Some(snapshot) = snapshot else {
        return Err(ServiceError::NothingToUndo);
    };
    if snapshot == rec.body {
        return Err(ServiceError::NothingToUndo);
    }

    let _guard = repository::document_lock(document_id);
    let op_id = format!("op-{}", uuid::Uuid::new_v4());
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'undo', ?4, NULL, 'proposed', NULL, NULL, NULL, NULL, NULL, ?5, ?5)",
        rusqlite::params![op_id, idempotency_key, document_id, rec.version, now_ms()],
    )?;
    let (new_version, revision_id) =
        commit_body_change(conn, notes_dir, &rec, &snapshot, None, &op_id, None)?;
    Ok(ApplyResult {
        document_id: document_id.to_string(),
        version: new_version,
        revision_id: Some(revision_id),
        already_committed: false,
        applied_title: None,
    })
}

/// 精确撤销某次 Agent patch：只读取该 operation 写入时留下的 revision checkpoint。
/// 文档在应用后又发生任何写入时拒绝撤销，避免“撤销最近一次”误伤用户后续编辑。
pub fn undo_patch(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    operation_id: &str,
) -> Result<ApplyResult, ServiceError> {
    let undo_key = format!("undo-patch:{operation_id}");
    if let Some(existing) = find_operation_by_key(conn, &undo_key)? {
        if existing.status == "committed" {
            return Ok(ApplyResult {
                document_id: existing.document_id,
                version: existing.target_version.unwrap_or(existing.base_version + 1),
                revision_id: None,
                already_committed: true,
                applied_title: None,
            });
        }
    }

    let patch = get_operation(conn, operation_id)?
        .ok_or_else(|| ServiceError::OperationNotFound(operation_id.to_string()))?;
    if patch.operation_type != "patch" {
        return Err(ServiceError::InvalidOperationState(
            "只能按 operation 撤销文档 patch".to_string(),
        ));
    }
    if patch.status != "committed" {
        let reason = if patch.status == "undone" {
            "本次 Agent 修改已经撤销".to_string()
        } else {
            format!("操作状态为 {}，没有可撤销的已应用修改", patch.status)
        };
        return Err(ServiceError::InvalidOperationState(reason));
    }

    let _guard = repository::document_lock(&patch.document_id);
    let rec = load_document(conn, notes_dir, &patch.document_id)?;
    let applied_version = patch.target_version.unwrap_or(patch.base_version + 1);
    if rec.version != applied_version {
        return Err(ServiceError::InvalidOperationState(format!(
            "文档在应用后又发生了修改（本次修改 v{applied_version}，当前 v{}），为避免覆盖后续内容，无法撤销",
            rec.version
        )));
    }

    let snapshot: Option<String> = conn
        .query_row(
            "SELECT content_snapshot FROM document_revisions
             WHERE document_id = ?1 AND operation_id = ?2
             ORDER BY version DESC LIMIT 1",
            rusqlite::params![patch.document_id, operation_id],
            |row| row.get(0),
        )
        .optional()?;
    let snapshot = snapshot.ok_or_else(|| {
        ServiceError::InvalidOperationState(
            "找不到本次 Agent 修改对应的 checkpoint，无法安全撤销".to_string(),
        )
    })?;

    let undo_id = format!("op-{}", uuid::Uuid::new_v4());
    let now = now_ms();
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'undo_patch', ?4, NULL, 'proposed', NULL, NULL, NULL, ?5, NULL, ?6, ?6)",
        rusqlite::params![undo_id, undo_key, patch.document_id, rec.version, patch.run_id, now],
    )?;
    let (version, revision_id) = commit_body_change(
        conn,
        notes_dir,
        &rec,
        &snapshot,
        None,
        &undo_id,
        patch.run_id.as_deref(),
    )?;
    set_operation_status(conn, &patch.id, "undone", None, None, None)?;
    Ok(ApplyResult {
        document_id: patch.document_id,
        version,
        revision_id: Some(revision_id),
        already_committed: false,
        applied_title: None,
    })
}

/// AG-26 项目 patch 操作列表（新→旧，至多 50 条）：前端重启后重建审批卡与
/// 审计轨迹的数据源（完成标准「重启后可审计和撤销」）。条目含已解决态
///（committed/rejected/failed/rolled_back）——前端按 op_status 差异渲染：
/// proposed → 可交互审批卡；committed → 已应用 + undo 入口；其余 → 状态展示。
/// 载荷损坏的行跳过（审计原表仍保留原始行，不因视图构造失败而丢列）。
pub fn list_project_patches(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Vec<ProjectPatchEntry>, ServiceError> {
    let mut stmt = conn.prepare(
        "SELECT op.id, op.idempotency_key, op.document_id, op.operation_type, op.base_version,
                op.target_version, op.status, op.error, op.tmp_path, op.approval_id, op.run_id,
                op.payload_json, op.created_at, op.updated_at
         FROM document_operations op
         JOIN project_documents pd ON pd.article_id = op.document_id
         WHERE pd.project_id = ?1 AND op.operation_type = 'patch'
         ORDER BY op.created_at DESC
         LIMIT 50",
    )?;
    let rows = stmt.query_map(rusqlite::params![project_id], map_operation_row)?;
    let mut out = Vec::new();
    for row in rows {
        let op = row?;
        let payload: PatchPayload =
            match serde_json::from_str(op.payload_json.as_deref().unwrap_or("{}")) {
                Ok(p) => p,
                Err(_) => continue,
            };
        let title =
            load_title(conn, &op.document_id).unwrap_or_else(|_| "（文档缺失）".to_string());
        let status = match op.status.as_str() {
            "committed" => "committed",
            "proposed" => "pending_approval",
            other => other,
        };
        let target_version = op.target_version.unwrap_or(op.base_version + 1);
        let current_version = repository::current_version(conn, &op.document_id)?;
        let undoable = op.status == "committed" && current_version == Some(target_version);
        let undo_unavailable_reason = match (op.status.as_str(), current_version) {
            ("committed", Some(current)) if current != target_version => Some(format!(
                "文档在应用后又发生了修改（本次修改 v{target_version}，当前 v{current}），为避免覆盖后续内容，无法撤销"
            )),
            ("committed", None) => Some("文档已不存在，无法撤销".to_string()),
            ("undone", _) => Some("本次 Agent 修改已经撤销".to_string()),
            _ => None,
        };
        out.push(ProjectPatchEntry {
            preview: PatchPreview {
                operation_id: op.id.clone(),
                approval_id: op.approval_id.clone(),
                document_id: op.document_id.clone(),
                title,
                base_version: op.base_version,
                target_version,
                old_text: payload.expected_text,
                new_text: payload.replacement_markdown,
                hunks: payload.hunks,
                status: status.to_string(),
                scope: payload.scope,
                rebased: payload.rebased,
                proposed_title: payload.proposed_title,
            },
            op_status: op.status.clone(),
            error: op.error.clone(),
            applied_hunks: payload.applied_hunks,
            created_at: op.created_at,
            undoable,
            undo_unavailable_reason,
        });
    }
    Ok(out)
}

/// 文档当前版本号（AG-26 前端选区 chip 的 baseVersion 来源；轻量只读，不读文件）。
/// Article DTO 不含 version 字段，此命令是前端获取版本号的唯一通道。
pub fn get_current_version(
    conn: &rusqlite::Connection,
    document_id: &str,
) -> Result<i64, ServiceError> {
    match repository::current_version(conn, document_id).map_err(ServiceError::Doc)? {
        Some(v) => Ok(v),
        None => Err(ServiceError::Doc(DocError::NotFound(
            document_id.to_string(),
        ))),
    }
}

/// 在指定项目内建文档（create 无覆盖风险 → 免审批直接提交；幂等键防重复创建）
pub fn create_document_in_project(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    project_id: &str,
    title: &str,
    body: &str,
    idempotency_key: Option<&str>,
) -> Result<CreatedDocument, ServiceError> {
    if let Some(key) = idempotency_key {
        if let Some(existing) = find_operation_by_key(conn, key)? {
            if existing.status == "committed" {
                let payload: CreatePayload =
                    serde_json::from_str(existing.payload_json.as_deref().unwrap_or("{}"))
                        .map_err(|e| {
                            ServiceError::InvalidOperationState(format!("操作载荷损坏: {e}"))
                        })?;
                return Ok(CreatedDocument {
                    article_id: payload.article_id,
                    title: payload.title,
                    version: existing.target_version.unwrap_or(1),
                });
            }
        }
    }
    if title.trim().is_empty() {
        return Err(ServiceError::InvalidInput("文档标题不能为空".to_string()));
    }
    let project_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )?;
    if project_exists == 0 {
        return Err(ServiceError::NotInProject);
    }

    let article_id = uuid::Uuid::new_v4().to_string();
    let version = repository::create_document(conn, notes_dir, &article_id, title, "manual", body)?;
    conn.execute(
        "INSERT OR REPLACE INTO project_documents (project_id, article_id, added_at)
         VALUES (?1, ?2, CURRENT_TIMESTAMP)",
        rusqlite::params![project_id, article_id],
    )?;

    let op_id = format!("op-{}", uuid::Uuid::new_v4());
    let payload_json = serde_json::to_string(&CreatePayload {
        article_id: article_id.clone(),
        title: title.to_string(),
    })
    .map_err(|e| ServiceError::InvalidOperationState(format!("载荷序列化失败: {e}")))?;
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'create', 0, ?4, 'committed', NULL, NULL, NULL, NULL, ?5, ?6, ?6)",
        rusqlite::params![
            op_id,
            idempotency_key,
            article_id,
            version,
            payload_json,
            now_ms()
        ],
    )?;
    Ok(CreatedDocument {
        article_id,
        title: title.to_string(),
        version,
    })
}

/// 移动文档归属（project_documents PK=article_id → INSERT OR REPLACE 即 move 语义）。
/// source_gate = 调用方绑定的项目：仅能移动自己项目的成员文档。
pub fn move_document(
    conn: &rusqlite::Connection,
    document_id: &str,
    target_project_id: &str,
    source_gate: &str,
) -> Result<(), ServiceError> {
    if !is_project_member(conn, source_gate, document_id)? {
        return Err(ServiceError::NotInProject);
    }
    let target_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM projects WHERE id = ?1",
        rusqlite::params![target_project_id],
        |r| r.get(0),
    )?;
    if target_exists == 0 {
        return Err(ServiceError::InvalidInput("目标项目不存在".to_string()));
    }
    let version = repository::current_version(conn, document_id)?
        .ok_or_else(|| ServiceError::Doc(DocError::NotFound(document_id.to_string())))?;
    conn.execute(
        "INSERT OR REPLACE INTO project_documents (project_id, article_id, added_at)
         VALUES (?1, ?2, CURRENT_TIMESTAMP)",
        rusqlite::params![target_project_id, document_id],
    )?;
    // 审计留痕：move 也记操作日志（不改正文，base=target=当前版本）
    let op_id = format!("op-{}", uuid::Uuid::new_v4());
    let payload_json =
        serde_json::json!({ "from": source_gate, "to": target_project_id }).to_string();
    conn.execute(
        "INSERT INTO document_operations
         (id, idempotency_key, document_id, operation_type, base_version, target_version,
          status, error, tmp_path, approval_id, run_id, payload_json, created_at, updated_at)
         VALUES (?1, NULL, ?2, 'move', ?3, ?3, 'committed', NULL, NULL, NULL, NULL, ?4, ?5, ?5)",
        rusqlite::params![op_id, document_id, version, payload_json, now_ms()],
    )?;
    Ok(())
}

/// 启动恢复（docs/architecture.md「启动时扫描未完成 operation 恢复或回滚」）：
/// prepared = tmp 已写、rename 前/中中断——正文文件未动（rename 原子），
/// 清 tmp 残留并回滚状态即可；committed 是真相，不碰。返回回滚条数。
pub fn recover_pending_operations(conn: &rusqlite::Connection, notes_dir: &Path) -> usize {
    let rows: Vec<(String, Option<String>)> = match conn
        .prepare("SELECT id, tmp_path FROM document_operations WHERE status = 'prepared'")
    {
        Ok(mut stmt) => stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default(),
        Err(_) => return 0,
    };
    let mut rolled_back = 0usize;
    for (id, tmp_path) in rows {
        if let Some(tmp) = tmp_path {
            let path = if Path::new(&tmp).is_absolute() {
                std::path::PathBuf::from(tmp)
            } else {
                notes_dir.join(tmp)
            };
            let _ = std::fs::remove_file(path);
        }
        if set_operation_status(
            conn,
            &id,
            "rolled_back",
            Some("启动恢复：上次写入未完成，已回滚"),
            None,
            None,
        )
        .is_ok()
        {
            rolled_back += 1;
        }
    }
    rolled_back
}

/// 行级 diff（AG-26 升级：Myers O(ND) 多 hunk，支持逐 hunk 部分批准）。
/// 纯函数，直接可测。算法：Myers 最短编辑脚本 → 连续变更区域 → 每区域一个
/// hunk（前后各带 HUNK_CONTEXT_LINES 行上下文）。编辑距离超过 DIFF_MAX_D
/// 时降级为整块单 hunk（正确性不受影响，只是审批粒度变粗）。
pub fn compute_hunks(old: &str, new: &str) -> Vec<PatchHunk> {
    if old == new {
        return Vec::new();
    }
    let old_lines: Vec<&str> = old.split('\n').collect();
    let new_lines: Vec<&str> = new.split('\n').collect();
    let regions = match myers_line_edits(&old_lines, &new_lines) {
        Some(edits) => edits_to_regions(&edits),
        None => vec![(0, old_lines.len(), 0, new_lines.len())],
    };
    regions
        .into_iter()
        .map(|(o_start, o_end, n_start, n_end)| {
            let cb_start = o_start.saturating_sub(HUNK_CONTEXT_LINES);
            let ca_end = std::cmp::min(o_end + HUNK_CONTEXT_LINES, old_lines.len());
            PatchHunk {
                start_line: o_start,
                context_before: own_lines(&old_lines[cb_start..o_start]),
                removed: own_lines(&old_lines[o_start..o_end]),
                added: own_lines(&new_lines[n_start..n_end]),
                context_after: own_lines(&old_lines[o_end..ca_end]),
            }
        })
        .collect()
}

fn own_lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| s.to_string()).collect()
}

/// Myers 编辑距离上限：超过则 compute_hunks 降级整块单 hunk。
/// 512 远超真实选区编辑的变更行数，同时钉死最坏情况复杂度。
const DIFF_MAX_D: usize = 512;

/// 行编辑动作（Myers 输出）：删除旧正文第 x 行 / 插入新正文第 y 行
#[derive(Debug, Clone, Copy, PartialEq)]
enum LineEdit {
    Delete(usize),
    Insert(usize),
}

/// Myers O(ND) 最短编辑脚本（按路径序返回，Delete/Insert 各自下标升序）。
/// 编辑距离 > DIFF_MAX_D 返回 None（调用方降级整块）。
fn myers_line_edits(old: &[&str], new: &[&str]) -> Option<Vec<LineEdit>> {
    let n = old.len() as isize;
    let m = new.len() as isize;
    if n == 0 && m == 0 {
        return Some(Vec::new());
    }
    let cap = (DIFF_MAX_D as isize).min(n + m);
    let off = cap + 1; // 对角线 k ∈ [-cap-1, cap+1] 的数组偏移
    let mut v = vec![0isize; (2 * cap + 3) as usize];
    v[(off + 1) as usize] = 0;
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut final_d: Option<isize> = None;
    'outer: for d in 0..=cap {
        for k in (-d..=d).step_by(2) {
            let ki = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[ki - 1] < v[ki + 1]) {
                v[ki + 1] // 向下 = 插入新行
            } else {
                v[ki - 1] + 1 // 向右 = 删除旧行
            };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            v[ki] = x;
            if x >= n && y >= m {
                final_d = Some(d);
                break 'outer;
            }
        }
        trace.push(v.clone());
    }
    let d_end = final_d?;
    // 回溯：从 (n, m) 收集每一步非对角移动（蛇形相等段不产生编辑）
    let mut edits_rev: Vec<LineEdit> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=d_end).rev() {
        let v_prev = &trace[(d - 1) as usize];
        let k = x - y;
        let ki = (k + off) as usize;
        let k_prev = if k == -d || (k != d && v_prev[ki - 1] < v_prev[ki + 1]) {
            k + 1
        } else {
            k - 1
        };
        let x_prev = v_prev[(k_prev + off) as usize];
        let y_prev = x_prev - k_prev;
        if k_prev == k + 1 {
            edits_rev.push(LineEdit::Insert(y_prev as usize)); // 向下移动
        } else {
            edits_rev.push(LineEdit::Delete(x_prev as usize)); // 向右移动
        }
        x = x_prev;
        y = y_prev;
    }
    edits_rev.reverse();
    Some(edits_rev)
}

/// 编辑脚本 → 连续变更区域 (old_start, old_end, new_start, new_end)（半开区间）。
/// 相邻编辑之间若隔相等行则分裂为两个区域（hunk 粒度的来源）。
fn edits_to_regions(edits: &[LineEdit]) -> Vec<(usize, usize, usize, usize)> {
    let mut regions: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut o = 0usize; // 下一个未消费的旧行
    let mut ni = 0usize; // 下一个未消费的新行
    let mut cur: Option<[usize; 4]> = None; // [o_start, o_end, n_start, n_end]
    let close = |regions: &mut Vec<_>, c: [usize; 4]| regions.push((c[0], c[1], c[2], c[3]));
    for &e in edits {
        match e {
            LineEdit::Delete(x) => match &mut cur {
                Some(c) if x <= c[1] => c[1] += 1,
                Some(_) => {
                    let c = cur.take().unwrap();
                    let eq = x - c[1]; // 相等间隙行数
                    close(&mut regions, c);
                    o = c[1] + eq;
                    ni = c[3] + eq;
                    cur = Some([x, x + 1, ni, ni]);
                }
                None => {
                    let eq = x - o;
                    o = x;
                    ni += eq;
                    cur = Some([x, x + 1, ni, ni]);
                }
            },
            LineEdit::Insert(y) => match &mut cur {
                Some(c) if y <= c[3] => c[3] += 1,
                Some(_) => {
                    let c = cur.take().unwrap();
                    let eq = y - c[3];
                    close(&mut regions, c);
                    o = c[1] + eq;
                    ni = c[3] + eq;
                    cur = Some([o, o, y, y + 1]);
                }
                None => {
                    let eq = y - ni;
                    ni = y;
                    o += eq;
                    cur = Some([o, o, y, y + 1]);
                }
            },
        }
    }
    if let Some(c) = cur {
        close(&mut regions, c);
    }
    regions
}

#[cfg(test)]
mod tests {
    //! AG-24 零模型测试：dry-run 不落盘 / 冲突检测 / 幂等 / 审批 / undo / 恢复。
    use super::*;
    use crate::documents::repository::tests::RepoFixture;

    /// 预览一个提案（默认参数：a1 旧正文含「这是正文第一句。」）
    fn preview_ok(fx: &RepoFixture) -> PatchPreview {
        let conn = fx.conn();
        preview_patch(
            &conn,
            &fx.notes,
            "a1",
            1,
            "这是正文第一句。",
            "这是修改后的第一句。",
            None,
            None,
            None,
        )
        .expect("预览应成功")
    }

    fn seed(fx: &RepoFixture) {
        fx.seed_article("a1", "测试笔记", "这是正文第一句。\n这是正文第二句。");
    }

    #[test]
    fn preview_is_dry_run_no_write() {
        let fx = RepoFixture::setup("dry");
        seed(&fx);
        let before = std::fs::read_to_string(fx.notes.join("a1.md")).unwrap();
        let preview = preview_ok(&fx);

        // 不写文件：盘上字节与版本都不变
        assert_eq!(
            std::fs::read_to_string(fx.notes.join("a1.md")).unwrap(),
            before
        );
        let conn = fx.conn();
        assert_eq!(repository::current_version(&conn, "a1").unwrap(), Some(1));
        // 提案落操作日志
        assert_eq!(preview.status, "pending_approval");
        assert_eq!(preview.base_version, 1);
        assert_eq!(preview.target_version, 2);
        assert_eq!(preview.title, "测试笔记");
        let op = get_operation(&conn, &preview.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "proposed");
        // hunks：第 0 行被替换，上下文带第二句
        assert_eq!(preview.hunks.len(), 1);
        assert_eq!(preview.hunks[0].start_line, 0);
        assert_eq!(preview.hunks[0].removed, vec!["这是正文第一句。"]);
        assert_eq!(preview.hunks[0].added, vec!["这是修改后的第一句。"]);
        assert_eq!(preview.hunks[0].context_after, vec!["这是正文第二句。"]);
        // 无 Run 上下文 → 无审批行
        assert!(preview.approval_id.is_none());
    }

    #[test]
    fn preview_base_version_mismatch_is_conflict() {
        let fx = RepoFixture::setup("ver");
        seed(&fx);
        let conn = fx.conn();
        let err = preview_patch(
            &conn,
            &fx.notes,
            "a1",
            5,
            "这是正文第一句。",
            "x",
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ServiceError::Doc(DocError::VersionConflict {
                expected: 5,
                actual: 1
            })
        ));
    }

    #[test]
    fn preview_anchor_zero_or_multi_match_is_conflict() {
        let fx = RepoFixture::setup("anchor");
        seed(&fx);
        let conn = fx.conn();
        // 0 匹配
        let err = preview_patch(
            &conn,
            &fx.notes,
            "a1",
            1,
            "不存在的文本",
            "x",
            None,
            None,
            None,
        )
        .unwrap_err();
        assert!(matches!(err, ServiceError::AnchorNotFound));
        // 多匹配：把正文改成含两处相同句子
        repository::write_body(&conn, &fx.notes, "a1", "重复句。\n重复句。", None).unwrap();
        let err = preview_patch(&conn, &fx.notes, "a1", 2, "重复句。", "x", None, None, None)
            .unwrap_err();
        assert!(matches!(err, ServiceError::AnchorAmbiguous(2)));
        // 空锚点
        let err = preview_patch(&conn, &fx.notes, "a1", 2, "", "x", None, None, None).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidInput(_)));
    }

    #[test]
    fn apply_commits_writes_revision_and_bumps_version() {
        let fx = RepoFixture::setup("apply");
        seed(&fx);
        let conn = fx.conn();
        let preview = preview_ok(&fx);
        let result = apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        assert_eq!(result.version, 2);
        assert!(!result.already_committed);
        // 正文已变 + 版本递增
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert!(rec.body.contains("这是修改后的第一句。"));
        assert!(rec.body.contains("这是正文第二句。"), "范围外内容必须保持");
        assert_eq!(rec.version, 2);
        // 修订快照 = 旧正文（undo 真相源）
        let snapshot: String = conn
            .query_row(
                "SELECT content_snapshot FROM document_revisions WHERE id = ?1",
                rusqlite::params![result.revision_id.unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(snapshot.contains("这是正文第一句。"));
        assert!(!snapshot.contains("修改后"));
        // 操作 committed + tmp 无残留
        let op = get_operation(&conn, &preview.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "committed");
        assert_eq!(op.target_version, Some(2));
        let leftovers: Vec<_> = std::fs::read_dir(&fx.notes)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "提交后不得有 tmp 残留");
    }

    #[test]
    fn apply_detects_user_edit_between_preview_and_apply() {
        // 核心安全语义：预览→批准之间用户改过文档 → 冲突拒绝，绝不覆盖
        let fx = RepoFixture::setup("race");
        seed(&fx);
        let conn = fx.conn();
        let preview = preview_ok(&fx);
        // 用户并发编辑（编辑器路径，版本 1→2）
        repository::write_body(&conn, &fx.notes, "a1", "用户刚改的内容", None).unwrap();
        let err = apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap_err();
        assert!(matches!(
            err,
            ServiceError::Doc(DocError::VersionConflict {
                expected: 1,
                actual: 2
            })
        ));
        // 用户内容完整保留，操作标记 failed
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "用户刚改的内容");
        let op = get_operation(&conn, &preview.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "failed");
    }

    #[test]
    fn idempotent_preview_and_apply_execute_once() {
        let fx = RepoFixture::setup("idem");
        seed(&fx);
        let conn = fx.conn();
        // 同键两次预览 → 同一操作
        let p1 = preview_patch(
            &conn,
            &fx.notes,
            "a1",
            1,
            "这是正文第一句。",
            "新句。",
            Some("key-1"),
            None,
            None,
        )
        .unwrap();
        let p2 = preview_patch(
            &conn,
            &fx.notes,
            "a1",
            1,
            "这是正文第一句。",
            "新句。",
            Some("key-1"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(p1.operation_id, p2.operation_id);
        // apply 一次成功；重入返回 already_committed 且版本不再递增
        let r1 = apply_patch(&conn, &fx.notes, &p1.operation_id).unwrap();
        assert_eq!(r1.version, 2);
        assert!(!r1.already_committed);
        let r2 = apply_patch(&conn, &fx.notes, &p1.operation_id).unwrap();
        assert!(r2.already_committed);
        assert_eq!(r2.version, 2);
        assert_eq!(repository::current_version(&conn, "a1").unwrap(), Some(2));
    }

    #[test]
    fn reject_blocks_later_apply_and_resolves_approval() {
        let fx = RepoFixture::setup("reject");
        seed(&fx);
        let conn = fx.conn();
        // Run 上下文 → 创建审批行
        conn.execute(
            "INSERT INTO agent_runs (id, thread_id, status, created_at, updated_at)
             VALUES ('r1', 't1', 'running', 1, 1)",
            [],
        )
        .unwrap();
        let preview = preview_patch(
            &conn,
            &fx.notes,
            "a1",
            1,
            "这是正文第一句。",
            "新句。",
            None,
            Some("r1"),
            None,
        )
        .unwrap();
        let aid = preview.approval_id.expect("Run 上下文应创建审批");
        let status: String = conn
            .query_row(
                "SELECT status FROM agent_approvals WHERE id = ?1",
                rusqlite::params![aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending");

        reject_patch(&conn, &preview.operation_id).unwrap();
        let op = get_operation(&conn, &preview.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "rejected");
        let status: String = conn
            .query_row(
                "SELECT status FROM agent_approvals WHERE id = ?1",
                rusqlite::params![aid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "rejected");
        // rejected 后 apply 必须拒绝，且零写入
        let err = apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidOperationState(_)));
        assert_eq!(repository::current_version(&conn, "a1").unwrap(), Some(1));
    }

    #[test]
    fn undo_restores_snapshot_as_new_version_and_is_redoable() {
        let fx = RepoFixture::setup("undo");
        seed(&fx);
        let conn = fx.conn();
        let original = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap()
            .body;
        let preview = preview_ok(&fx);
        apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        // 撤销 → 正文还原，版本只增不减（2→3）
        let undone = undo_last_change(&conn, &fx.notes, "a1", None, None).unwrap();
        assert_eq!(undone.version, 3);
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, original);
        // 修订链两条（apply 一条 + undo 一条）→ 撤销可再撤销（redo）
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM document_revisions WHERE document_id = 'a1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        let redone = undo_last_change(&conn, &fx.notes, "a1", None, None).unwrap();
        assert_eq!(redone.version, 4);
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert!(rec.body.contains("这是修改后的第一句。"));
        // 无可撤销 → NothingToUndo
        let fx2 = RepoFixture::setup("undo-none");
        seed(&fx2);
        let conn2 = fx2.conn();
        assert!(matches!(
            undo_last_change(&conn2, &fx2.notes, "a1", None, None).unwrap_err(),
            ServiceError::NothingToUndo
        ));
    }

    #[test]
    fn operation_checkpoint_undo_is_exact_and_refuses_to_overwrite_later_changes() {
        let fx = RepoFixture::setup("undo-operation");
        seed(&fx);
        let conn = fx.conn();
        let original = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap()
            .body;
        let preview = preview_ok(&fx);
        apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();

        let undone = undo_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        assert_eq!(undone.version, 3);
        assert_eq!(
            repository::get_document(&conn, &fx.notes, "a1")
                .unwrap()
                .unwrap()
                .body,
            original
        );
        assert_eq!(
            get_operation(&conn, &preview.operation_id)
                .unwrap()
                .unwrap()
                .status,
            "undone"
        );
        assert!(
            undo_patch(&conn, &fx.notes, &preview.operation_id)
                .unwrap()
                .already_committed
        );

        let fx2 = RepoFixture::setup("undo-operation-stale");
        seed(&fx2);
        let conn2 = fx2.conn();
        let first = preview_ok(&fx2);
        apply_patch(&conn2, &fx2.notes, &first.operation_id).unwrap();
        let second = preview_patch(
            &conn2,
            &fx2.notes,
            "a1",
            2,
            "这是正文第二句。",
            "这是后续修改。",
            None,
            None,
            None,
        )
        .unwrap();
        apply_patch(&conn2, &fx2.notes, &second.operation_id).unwrap();
        let error = undo_patch(&conn2, &fx2.notes, &first.operation_id).unwrap_err();
        assert!(error.to_string().contains("文档在应用后又发生了修改"));
    }

    #[test]
    fn recover_rolls_back_prepared_operations_and_cleans_tmp() {
        let fx = RepoFixture::setup("recover");
        seed(&fx);
        let conn = fx.conn();
        // 伪造一个中断的 prepared 操作 + tmp 残留
        let tmp = fx.notes.join("a1.op-fake.tmp");
        std::fs::write(&tmp, "half-written").unwrap();
        conn.execute(
            "INSERT INTO document_operations
             (id, idempotency_key, document_id, operation_type, base_version, target_version,
              status, tmp_path, created_at, updated_at)
             VALUES ('op-fake', NULL, 'a1', 'patch', 1, NULL, 'prepared', ?1, 1, 1)",
            rusqlite::params![tmp.to_string_lossy().to_string()],
        )
        .unwrap();
        let n = recover_pending_operations(&conn, &fx.notes);
        assert_eq!(n, 1);
        assert!(!tmp.exists(), "tmp 残留必须被清理");
        let status: String = conn
            .query_row(
                "SELECT status FROM document_operations WHERE id = 'op-fake'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "rolled_back");
        // 正文未受影响（rename 未发生）
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert!(rec.body.contains("这是正文第一句。"));
    }

    #[test]
    fn create_document_in_project_with_membership_and_idempotency() {
        let fx = RepoFixture::setup("create-doc");
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', '项目一')",
            [],
        )
        .unwrap();
        let created = create_document_in_project(
            &conn,
            &fx.notes,
            "p1",
            "Agent 建的笔记",
            "hello",
            Some("ck-1"),
        )
        .unwrap();
        assert_eq!(created.version, 1);
        assert!(is_project_member(&conn, "p1", &created.article_id).unwrap());
        let rec = repository::get_document(&conn, &fx.notes, &created.article_id)
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "hello");
        // 幂等重入 → 同一篇文档，不重复创建
        let again = create_document_in_project(
            &conn,
            &fx.notes,
            "p1",
            "Agent 建的笔记",
            "hello",
            Some("ck-1"),
        )
        .unwrap();
        assert_eq!(again.article_id, created.article_id);
        // 项目不存在 → 拒绝
        assert!(matches!(
            create_document_in_project(&conn, &fx.notes, "ghost", "x", "", None).unwrap_err(),
            ServiceError::NotInProject
        ));
        // 空标题 → 拒绝
        assert!(matches!(
            create_document_in_project(&conn, &fx.notes, "p1", "  ", "", None).unwrap_err(),
            ServiceError::InvalidInput(_)
        ));
    }

    #[test]
    fn move_document_transfers_membership_with_gate() {
        let fx = RepoFixture::setup("move");
        seed(&fx);
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', '项目一'), ('p2', '项目二')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id, added_at) VALUES ('p1', 'a1', 1)",
            [],
        )
        .unwrap();
        move_document(&conn, "a1", "p2", "p1").unwrap();
        assert!(!is_project_member(&conn, "p1", "a1").unwrap());
        assert!(is_project_member(&conn, "p2", "a1").unwrap());
        // 闸门：非源项目成员不得移动
        assert!(matches!(
            move_document(&conn, "a1", "p1", "p1").unwrap_err(),
            ServiceError::NotInProject
        ));
        // 目标项目必须存在
        assert!(matches!(
            move_document(&conn, "a1", "ghost", "p2").unwrap_err(),
            ServiceError::InvalidInput(_)
        ));
    }

    #[test]
    fn gate_rejects_non_member_in_preview() {
        let fx = RepoFixture::setup("gate");
        seed(&fx);
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', '项目一')",
            [],
        )
        .unwrap();
        // a1 未入任何项目 → 闸门拒绝（不泄漏存在性）
        let err = preview_patch(
            &conn,
            &fx.notes,
            "a1",
            1,
            "这是正文第一句。",
            "x",
            None,
            None,
            Some("p1"),
        )
        .unwrap_err();
        assert!(matches!(err, ServiceError::NotInProject));
    }

    #[test]
    fn compute_hunks_shapes() {
        // 相同 → 空
        assert!(compute_hunks("a\nb", "a\nb").is_empty());
        // 单行替换：行号与上下文
        let hunks = compute_hunks("l1\nl2\nl3\nl4", "l1\nX\nl3\nl4");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start_line, 1);
        assert_eq!(hunks[0].removed, vec!["l2"]);
        assert_eq!(hunks[0].added, vec!["X"]);
        assert_eq!(hunks[0].context_before, vec!["l1"]);
        assert_eq!(hunks[0].context_after, vec!["l3", "l4"]);
        // 多行替换为多行
        let hunks = compute_hunks("a\nb\nc", "a\nx\ny\nc");
        assert_eq!(hunks[0].removed, vec!["b"]);
        assert_eq!(hunks[0].added, vec!["x", "y"]);
    }

    // ------------------- AG-25：选区感知 / 安全 rebase / 结构校验 -------------------

    /// 构造选区感知请求（锚点 = 选中文本 + 真实 hash + 前后文）
    fn scoped_req(
        document_id: &str,
        base: i64,
        text: &str,
        before: &str,
        after: &str,
    ) -> ScopedPatchRequest {
        ScopedPatchRequest {
            document_id: document_id.to_string(),
            base_version: base,
            scope: PatchScope::Selection,
            anchor: anchor::TextAnchor {
                selected_text: text.to_string(),
                selected_text_hash: repository::content_hash(text),
                before_context: before.to_string(),
                after_context: after.to_string(),
            },
            replacement_markdown: "这是修改后的第一句。".to_string(),
            idempotency_key: None,
            run_id: None,
            project_gate: None,
        }
    }

    #[test]
    fn scoped_preview_replaces_only_selected_range_and_is_dry_run() {
        let fx = RepoFixture::setup("ag25-scope");
        seed(&fx);
        let conn = fx.conn();
        let before = std::fs::read_to_string(fx.notes.join("a1.md")).unwrap();
        let req = scoped_req("a1", 1, "这是正文第一句。", "", "\n这是正文第二句。");
        let preview = preview_scoped_patch(&conn, &fx.notes, &req).expect("选区预览应成功");

        // dry-run 铁律：盘上字节与版本不变
        assert_eq!(
            std::fs::read_to_string(fx.notes.join("a1.md")).unwrap(),
            before
        );
        assert_eq!(repository::current_version(&conn, "a1").unwrap(), Some(1));
        assert_eq!(preview.scope, Some(PatchScope::Selection));
        assert!(!preview.rebased);
        // 只覆盖选区所在行
        assert_eq!(preview.hunks[0].removed, vec!["这是正文第一句。"]);
        assert_eq!(preview.hunks[0].added, vec!["这是修改后的第一句。"]);

        // 应用后：仅选区变化，范围外逐字保留
        let result = apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        assert_eq!(result.version, 2);
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert!(rec.body.contains("这是修改后的第一句。"));
        assert!(rec.body.contains("这是正文第二句。"), "范围外内容必须保持");
        assert!(!rec.body.contains("这是正文第一句。"));
    }

    #[test]
    fn scoped_preview_rebases_when_anchor_still_valid_after_version_change() {
        // 安全 rebase（docs/architecture.md）：版本变化但目标 hash 未变且锚点仍唯一 → 重出 diff
        let fx = RepoFixture::setup("ag25-rebase");
        seed(&fx);
        let conn = fx.conn();
        // 用户在无关位置追加内容（版本 1→2，选区目标未变）
        repository::write_body(
            &conn,
            &fx.notes,
            "a1",
            "这是正文第一句。\n这是正文第二句。\n这是用户新加的第三句。",
            None,
        )
        .unwrap();
        let req = scoped_req("a1", 1, "这是正文第一句。", "", "\n这是正文第二句。");
        let preview = preview_scoped_patch(&conn, &fx.notes, &req).expect("安全 rebase 应成功");
        assert!(preview.rebased, "版本变化但锚点有效 → rebased");
        assert_eq!(preview.base_version, 2, "rebase 后 base = 当前版本");
        assert_eq!(preview.target_version, 3);
        // 应用：只改选区，用户新加内容保留
        let result = apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        assert_eq!(result.version, 3);
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert!(rec.body.contains("这是修改后的第一句。"));
        assert!(rec.body.contains("这是用户新加的第三句。"));
    }

    #[test]
    fn scoped_preview_conflicts_when_target_changed_with_version() {
        // 版本变化且目标内容已变 → conflict，零写入、无操作行
        let fx = RepoFixture::setup("ag25-conflict");
        seed(&fx);
        let conn = fx.conn();
        repository::write_body(
            &conn,
            &fx.notes,
            "a1",
            "用户重写了第一句。\n这是正文第二句。",
            None,
        )
        .unwrap();
        let req = scoped_req("a1", 1, "这是正文第一句。", "", "\n这是正文第二句。");
        let err = preview_scoped_patch(&conn, &fx.notes, &req).unwrap_err();
        assert!(matches!(
            err,
            ServiceError::Doc(DocError::VersionConflict {
                expected: 1,
                actual: 2
            })
        ));
        let ops: i64 = conn
            .query_row("SELECT COUNT(*) FROM document_operations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ops, 0, "冲突停止不得留下半成品操作");
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "用户重写了第一句。\n这是正文第二句。");
    }

    #[test]
    fn scoped_preview_ambiguous_anchor_is_conflict() {
        // 匹配不唯一且无上下文可消歧 → Ambiguous，不猜测覆盖
        let fx = RepoFixture::setup("ag25-ambig");
        fx.seed_article("a1", "测试笔记", "重复句。\n重复句。");
        let conn = fx.conn();
        let req = scoped_req("a1", 1, "重复句。", "", "");
        let err = preview_scoped_patch(&conn, &fx.notes, &req).unwrap_err();
        assert!(matches!(err, ServiceError::AnchorAmbiguous(2)));
        // 带上消歧上下文 → 唯一命中
        let req = scoped_req("a1", 1, "重复句。", "", "\n重复句。");
        let preview = preview_scoped_patch(&conn, &fx.notes, &req).expect("消歧后应唯一命中");
        apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(
            rec.body, "这是修改后的第一句。\n重复句。",
            "只替换命中的那一处"
        );
    }

    #[test]
    fn scoped_preview_structure_violation_rejected_without_side_effects() {
        // 选区跨越代码围栏边界 → MarkdownValidation，零副作用
        let fx = RepoFixture::setup("ag25-struct");
        fx.seed_article("a1", "测试笔记", "前言。\n```\ncode\n```\n后记。");
        let conn = fx.conn();
        let req = scoped_req("a1", 1, "前言。\n```\ncode", "", "");
        let err = preview_scoped_patch(&conn, &fx.notes, &req).unwrap_err();
        assert!(matches!(err, ServiceError::MarkdownValidation(_)));
        let ops: i64 = conn
            .query_row("SELECT COUNT(*) FROM document_operations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ops, 0);
        assert_eq!(repository::current_version(&conn, "a1").unwrap(), Some(1));
    }

    #[test]
    fn scoped_apply_conflicts_on_user_edit_and_replay_is_idempotent() {
        // 预览→批准之间用户编辑 → apply 冲突；同键重入返回原提案
        let fx = RepoFixture::setup("ag25-race");
        seed(&fx);
        let conn = fx.conn();
        let mut req = scoped_req("a1", 1, "这是正文第一句。", "", "\n这是正文第二句。");
        req.idempotency_key = Some("ag25-key".to_string());
        let p1 = preview_scoped_patch(&conn, &fx.notes, &req).unwrap();
        let p2 = preview_scoped_patch(&conn, &fx.notes, &req).unwrap();
        assert_eq!(p1.operation_id, p2.operation_id, "幂等重入同操作");
        // 用户并发编辑（版本 1→2，选区目标消失）
        repository::write_body(&conn, &fx.notes, "a1", "用户刚改的内容", None).unwrap();
        let err = apply_patch(&conn, &fx.notes, &p1.operation_id).unwrap_err();
        assert!(matches!(
            err,
            ServiceError::Doc(DocError::VersionConflict {
                expected: 1,
                actual: 2
            })
        ));
        let rec = repository::get_document(&conn, &fx.notes, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "用户刚改的内容", "绝不静默覆盖用户内容");
    }

    // ------------------- AG-26：多 hunk diff / 逐 hunk 部分批准 / 项目审计列表 -------------------

    /// 全量选区请求（自定义替换文本；锚点 = 选中文本 + 真实 hash）
    fn scoped_req_full(
        document_id: &str,
        base: i64,
        text: &str,
        replacement: &str,
    ) -> ScopedPatchRequest {
        ScopedPatchRequest {
            document_id: document_id.to_string(),
            base_version: base,
            scope: PatchScope::Selection,
            anchor: anchor::TextAnchor {
                selected_text: text.to_string(),
                selected_text_hash: repository::content_hash(text),
                before_context: String::new(),
                after_context: String::new(),
            },
            replacement_markdown: replacement.to_string(),
            idempotency_key: None,
            run_id: None,
            project_gate: None,
        }
    }

    #[test]
    fn compute_hunks_multi_regions_and_pure_edits() {
        // 两处独立修改被唯一相等行隔开 → 2 个升序、互不重叠的 hunk
        let hunks = compute_hunks("p1\nmid\np2", "P1\nmid\nP2");
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].start_line, 0);
        assert_eq!(hunks[0].removed, vec!["p1"]);
        assert_eq!(hunks[0].added, vec!["P1"]);
        assert_eq!(hunks[1].start_line, 2);
        assert_eq!(hunks[1].removed, vec!["p2"]);
        assert_eq!(hunks[1].added, vec!["P2"]);
        // 纯插入：removed 为空，start_line = 插入点行号
        let hunks = compute_hunks("a\nb", "a\nX\nb");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start_line, 1);
        assert!(hunks[0].removed.is_empty());
        assert_eq!(hunks[0].added, vec!["X"]);
        // 纯删除：added 为空
        let hunks = compute_hunks("a\nX\nb", "a\nb");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start_line, 1);
        assert_eq!(hunks[0].removed, vec!["X"]);
        assert!(hunks[0].added.is_empty());
        // 编辑距离超 DIFF_MAX_D → 降级为覆盖全区的单 hunk（正确性不依赖 diff 质量）
        let old: String = (0..600)
            .map(|i| format!("old-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let new: String = (0..600)
            .map(|i| format!("new-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let hunks = compute_hunks(&old, &new);
        assert_eq!(hunks.len(), 1, "超上限必须降级为单 hunk");
        assert_eq!(hunks[0].start_line, 0);
        assert_eq!(hunks[0].removed.len(), 600);
        assert_eq!(hunks[0].added.len(), 600);
    }

    #[test]
    fn host_document_workcopy_creates_reviewable_multi_hunk_patch() {
        let fx = RepoFixture::setup("ag19-host-workcopy");
        let body = "第一段。\n共享中间行。\n第三段。";
        fx.seed_article("doc1", "左侧原文", body);
        let conn = fx.conn();

        let preview = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            body,
            "第一段（格式化）。\n共享中间行。\n第三段（格式化）。",
            Some("run-1:doc1"),
            None,
            None,
        )
        .expect("Hermes 工作副本应转换成普通 Patch");

        assert_eq!(preview.document_id, "doc1");
        assert_eq!(preview.hunks.len(), 2);
        assert_eq!(repository::current_version(&conn, "doc1").unwrap(), Some(1));
        assert_eq!(
            repository::get_document(&conn, &fx.notes, "doc1")
                .unwrap()
                .unwrap()
                .body,
            body,
            "dry-run 阶段不得直接覆盖左侧原文"
        );

        let result = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0]).unwrap();
        assert_eq!(result.version, 2);
        assert_eq!(
            repository::get_document(&conn, &fx.notes, "doc1")
                .unwrap()
                .unwrap()
                .body,
            "第一段（格式化）。\n共享中间行。\n第三段。"
        );
    }

    #[test]
    fn host_document_workcopy_stops_on_stale_editor_baseline() {
        let fx = RepoFixture::setup("ag19-host-workcopy-stale");
        fx.seed_article("doc1", "左侧原文", "当前正文");
        let conn = fx.conn();

        let error = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            "未保存的旧草稿",
            "Hermes 修改稿",
            None,
            None,
            None,
        )
        .unwrap_err();

        assert!(matches!(error, ServiceError::InvalidOperationState(_)));
        let operations: i64 = conn
            .query_row("SELECT COUNT(*) FROM document_operations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(operations, 0, "基线不一致时不得留下 Patch 半成品");
    }

    #[test]
    fn host_workcopy_title_change_is_applied_together_with_body() {
        // NEXT-042：模型同一轮既改标题区又改正文 → 并入同一审批；
        // 整块批准后标题与正文一次写盘（文件 frontmatter 与 DB 同步）。
        let fx = RepoFixture::setup("next042-title-apply");
        let body = "正文第一行。\n共享中间行。\n正文第三行。";
        fx.seed_article("doc1", "旧标题", body);
        let conn = fx.conn();

        let preview = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            body,
            "正文第一行（改写）。\n共享中间行。\n正文第三行。",
            None,
            None,
            Some("  AI圈动态·8月16日  "),
        )
        .expect("标题提案应并入正文 Patch 审批");
        assert_eq!(
            preview.proposed_title.as_deref(),
            Some("AI圈动态·8月16日"),
            "trim 后写入预览"
        );

        let result = apply_patch(&conn, &fx.notes, &preview.operation_id).unwrap();
        assert_eq!(result.applied_title.as_deref(), Some("AI圈动态·8月16日"));
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.title, "AI圈动态·8月16日");
        assert!(rec.body.starts_with("正文第一行（改写）。"));
        let raw = std::fs::read_to_string(fx.notes.join("doc1.md")).unwrap();
        assert!(
            raw.contains("title: \"AI圈动态·8月16日\""),
            "文件 frontmatter 必须承载新标题，否则读回仍是旧标题"
        );
    }

    #[test]
    fn partial_apply_never_renames_even_when_title_was_proposed() {
        // NEXT-042：子集批准只代表部分接受正文，绝不顺带改名。
        let fx = RepoFixture::setup("next042-title-partial");
        let body = "甲段。\n共享行。\n乙段。";
        fx.seed_article("doc1", "旧标题", body);
        let conn = fx.conn();

        let preview = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            body,
            "甲段（改）。\n共享行。\n乙段（改）。",
            None,
            None,
            Some("新标题"),
        )
        .unwrap();
        assert_eq!(preview.proposed_title.as_deref(), Some("新标题"));
        let result = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0]).unwrap();
        assert!(result.applied_title.is_none());
        assert_eq!(result.version, 2);
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.title, "旧标题", "部分批准不得改标题");
    }

    #[test]
    fn proposed_title_validation_is_fail_closed() {
        // NEXT-042：格式违规 / 项目内同名 → 拒绝整个提案且不留半成品；
        // 与当前标题相同 → 归一为无提案，正文 diff 照常。
        let fx = RepoFixture::setup("next042-title-validate");
        fx.seed_article("doc1", "文档甲", "正文");
        fx.seed_article("doc2", "文档乙", "正文");
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', '测试项目')",
            rusqlite::params![],
        )
        .unwrap();
        for id in ["doc1", "doc2"] {
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id, added_at) VALUES ('p1', ?1, 1)",
                rusqlite::params![id],
            )
            .unwrap();
        }

        let error = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            "正文",
            "新正文",
            None,
            None,
            Some("含\n换行"),
        )
        .unwrap_err();
        assert!(matches!(error, ServiceError::InvalidInput(_)));
        let error = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            "正文",
            "新正文",
            None,
            None,
            Some("文档乙"),
        )
        .unwrap_err();
        assert!(
            matches!(error, ServiceError::InvalidInput(_)),
            "项目内同名必须拒绝"
        );
        let operations: i64 = conn
            .query_row("SELECT COUNT(*) FROM document_operations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(operations, 0, "校验失败不得留下 Patch 半成品");

        let preview = preview_host_document_patch(
            &conn,
            &fx.notes,
            "doc1",
            1,
            "正文",
            "新正文",
            None,
            None,
            Some(" 文档甲 "),
        )
        .unwrap();
        assert!(
            preview.proposed_title.is_none(),
            "与当前标题相同 = 无需提案"
        );
    }

    #[test]
    fn partial_apply_writes_only_approved_hunks_and_supports_undo() {
        let fx = RepoFixture::setup("ag26-partial");
        let body = "甲段第一行。\n共享中间行。\n末段最后一行。";
        fx.seed_article("doc1", "双段文档", body);
        let conn = fx.conn();
        let req = scoped_req_full(
            "doc1",
            1,
            body,
            "甲段第一行（改）。\n共享中间行。\n末段最后一行（改）。",
        );
        let preview = preview_scoped_patch(&conn, &fx.notes, &req).expect("预览应成功");
        assert_eq!(preview.hunks.len(), 2, "中间相等行应把 diff 切成 2 个 hunk");

        // 只批准第 1 个 hunk：仅首段落盘，末段原样
        let result = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0]).unwrap();
        assert_eq!(result.version, 2);
        assert!(!result.already_committed);
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(
            rec.body, "甲段第一行（改）。\n共享中间行。\n末段最后一行。",
            "未批准 hunk 绝不写入"
        );
        // 审计：实际批准子集回填 payload
        let payload_json: String = conn
            .query_row(
                "SELECT payload_json FROM document_operations WHERE id = ?1",
                rusqlite::params![preview.operation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(payload_json.contains("\"appliedHunks\":[0]"));
        // 幂等重入：同操作再次批准 → already_committed，零新写入
        let again = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0]).unwrap();
        assert!(again.already_committed);
        assert_eq!(repository::current_version(&conn, "doc1").unwrap(), Some(2));
        // undo：快照还原 = 应用前正文（新版本号）
        let undone = undo_last_change(&conn, &fx.notes, "doc1", None, None).unwrap();
        assert_eq!(undone.version, 3);
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, body);
    }

    #[test]
    fn partial_apply_validates_subset_bounds() {
        let fx = RepoFixture::setup("ag26-subset");
        let body = "甲段第一行。\n共享中间行。\n末段最后一行。";
        fx.seed_article("doc1", "双段文档", body);
        let conn = fx.conn();
        let preview = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full(
                "doc1",
                1,
                body,
                "甲段第一行（改）。\n共享中间行。\n末段最后一行（改）。",
            ),
        )
        .unwrap();
        // 空子集 → InvalidInput（全部拒绝请走拒绝操作），零副作用
        let err = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[]).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidInput(_)));
        // 越界下标 → InvalidInput
        let err = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[5]).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidInput(_)));
        assert_eq!(repository::current_version(&conn, "doc1").unwrap(), Some(1));
        // 重复下标去重后是合法子集：只应用 hunk 1
        let result = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[1, 1]).unwrap();
        assert_eq!(result.version, 2);
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "甲段第一行。\n共享中间行。\n末段最后一行（改）。");
    }

    #[test]
    fn partial_apply_full_subset_delegates_to_full_path() {
        // 全量子集 → 委托 AG-24 既有完整路径（锚点复检口径完全一致）
        let fx = RepoFixture::setup("ag26-full-subset");
        let body = "甲段第一行。\n共享中间行。\n末段最后一行。";
        fx.seed_article("doc1", "双段文档", body);
        let conn = fx.conn();
        let preview = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full(
                "doc1",
                1,
                body,
                "甲段第一行（改）。\n共享中间行。\n末段最后一行（改）。",
            ),
        )
        .unwrap();
        let result = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0, 1]).unwrap();
        assert_eq!(result.version, 2);
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(
            rec.body,
            "甲段第一行（改）。\n共享中间行。\n末段最后一行（改）。"
        );
    }

    #[test]
    fn partial_apply_conflicts_on_external_file_drift() {
        // 预览后文件被绕过版本体系直接修改（DB 版本未变）→ 批准 hunk 原文行不一致
        // → never-guess 停止：op=failed，文件与版本原样
        let fx = RepoFixture::setup("ag26-drift");
        let body = "甲段第一行。\n共享中间行。\n末段最后一行。";
        fx.seed_article("doc1", "双段文档", body);
        let conn = fx.conn();
        let preview = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full(
                "doc1",
                1,
                body,
                "甲段第一行（改）。\n共享中间行。\n末段最后一行（改）。",
            ),
        )
        .unwrap();
        // 外部篡改：只改首个 hunk 覆盖的行，不动 frontmatter
        let path = fx.notes.join("doc1.md");
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, raw.replacen("甲段第一行。", "被外部改写的甲段。", 1)).unwrap();
        let err = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0]).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidOperationState(_)));
        let op = get_operation(&conn, &preview.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "failed");
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "被外部改写的甲段。\n共享中间行。\n末段最后一行。");
        assert_eq!(repository::current_version(&conn, "doc1").unwrap(), Some(1));
    }

    #[test]
    fn partial_apply_rejects_fence_breaking_subset() {
        // 整体预览合法（围栏配对），但子集应用会打破围栏 → 逐 hunk 结构校验拒绝
        let fx = RepoFixture::setup("ag26-fence");
        fx.seed_article("doc1", "围栏文档", "s0\ns1\ns2");
        let conn = fx.conn();
        let preview = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full("doc1", 1, "s0\ns1\ns2", "```\ns1\n```"),
        )
        .expect("整体替换围栏配对，预览应通过");
        assert_eq!(preview.hunks.len(), 2);
        // 只批准第 1 个 hunk → 新正文围栏失配 → MarkdownValidation，零写入
        let err = apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0]).unwrap_err();
        assert!(matches!(err, ServiceError::MarkdownValidation(_)));
        let op = get_operation(&conn, &preview.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(op.status, "failed");
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "s0\ns1\ns2");
        assert_eq!(repository::current_version(&conn, "doc1").unwrap(), Some(1));
        // failed 操作不可再批准（状态锁）
        let err =
            apply_patch_partial(&conn, &fx.notes, &preview.operation_id, &[0, 1]).unwrap_err();
        assert!(matches!(err, ServiceError::InvalidOperationState(_)));
        // 新提案批准全部 hunk（= 全量路径）仍然可行：整体围栏配对
        let p2 = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full("doc1", 1, "s0\ns1\ns2", "```\ns1\n```"),
        )
        .unwrap();
        let result = apply_patch_partial(&conn, &fx.notes, &p2.operation_id, &[0, 1]).unwrap();
        assert_eq!(result.version, 2);
        let rec = repository::get_document(&conn, &fx.notes, "doc1")
            .unwrap()
            .unwrap();
        assert_eq!(rec.body, "```\ns1\n```");
    }

    #[test]
    fn list_project_patches_rebuilds_approval_lifecycle() {
        // 重启后审计源：proposed/committed/rejected 三态 + 项目成员过滤
        let fx = RepoFixture::setup("ag26-list");
        let body = "甲段第一行。\n共享中间行。\n末段最后一行。";
        fx.seed_article("doc1", "双段文档", body);
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', '项目一')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p2', '项目二')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id) VALUES ('p1', 'doc1')",
            [],
        )
        .unwrap();
        // op1：提案后搁置（proposed）
        let p_proposed = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full("doc1", 1, "共享中间行。", "共享中间行（保留提案）。"),
        )
        .unwrap();
        // op2：批准应用（committed）
        let p_committed = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full("doc1", 1, "甲段第一行。", "甲段已应用。"),
        )
        .unwrap();
        apply_patch(&conn, &fx.notes, &p_committed.operation_id).unwrap();
        // op3：拒绝（rejected；此时版本已 2，锚点仍在 → 正常出提案）
        let p_rejected = preview_scoped_patch(
            &conn,
            &fx.notes,
            &scoped_req_full("doc1", 2, "末段最后一行。", "末段已拒绝。"),
        )
        .unwrap();
        reject_patch(&conn, &p_rejected.operation_id).unwrap();

        let entries = list_project_patches(&conn, "p1").unwrap();
        assert_eq!(entries.len(), 3);
        let by_id: std::collections::HashMap<String, &ProjectPatchEntry> = entries
            .iter()
            .map(|e| (e.preview.operation_id.clone(), e))
            .collect();
        let e_proposed = &by_id[&p_proposed.operation_id];
        assert_eq!(e_proposed.op_status, "proposed");
        assert_eq!(e_proposed.preview.status, "pending_approval");
        let e_committed = &by_id[&p_committed.operation_id];
        assert_eq!(e_committed.op_status, "committed");
        assert_eq!(e_committed.preview.status, "committed");
        assert!(e_committed.applied_hunks.is_none(), "全量批准不记子集");
        let e_rejected = &by_id[&p_rejected.operation_id];
        assert_eq!(e_rejected.op_status, "rejected");
        assert!(entries.iter().all(|e| e.created_at > 0));
        // 非成员项目不可见
        assert!(list_project_patches(&conn, "p2").unwrap().is_empty());
    }

    #[test]
    fn current_version_query_reads_and_reports_missing() {
        let fx = RepoFixture::setup("ag26-version");
        seed(&fx);
        let conn = fx.conn();
        assert_eq!(get_current_version(&conn, "a1").unwrap(), 1);
        let err = get_current_version(&conn, "不存在的文档").unwrap_err();
        assert!(matches!(err, ServiceError::Doc(DocError::NotFound(_))));
    }
}
