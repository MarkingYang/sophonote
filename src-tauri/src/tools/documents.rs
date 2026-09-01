// ============================================================
// Track B · AG-24（审计批次 5 第 5 步）：文档写工具注册进 ToolGateway。
//
// 铁律（docs/architecture.md）：
// ① 模型只有「提议」能力——propose_document_patch 恒 dry-run，只产 diff 提案
//    与 operation（proposed），**零文件写入**；落盘必须经用户侧
//    document_apply_patch 命令（documents/commands.rs）批准；
// ② 范围隔离同 AG-19 口径：project_id/run_id 在构造时绑定，跨项目一律拒绝；
// ③ 幂等键由模型携带，重入返回原结果（docs/architecture.md）。
// ============================================================
use std::path::PathBuf;

use async_trait::async_trait;

use super::{SophoNoteTool, ProvenanceRef, ToolDescriptor, ToolError, ToolOutput, UiArtifact};
use crate::documents::service::{self, PatchPreview};

fn service_err(e: service::ServiceError) -> ToolError {
    ToolError::Execution(e.to_string())
}

fn str_arg(arguments: &serde_json::Value, key: &str) -> Result<String, ToolError> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidArguments(format!("缺少必填字符串参数 {key}")))
}

fn opt_str_arg(arguments: &serde_json::Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 把 hunk 列表渲染成 Markdown diff 代码块（fallback_markdown 与卡片共用口径）
fn render_diff_markdown(preview: &PatchPreview) -> String {
    let mut md = format!(
        "文档《{}》修改建议（v{} → v{}，请在原文中使用 ✓/×）：\n\n```diff\n",
        preview.title, preview.base_version, preview.target_version
    );
    for hunk in &preview.hunks {
        for line in &hunk.context_before {
            md.push_str(&format!("  {line}\n"));
        }
        for line in &hunk.removed {
            md.push_str(&format!("- {line}\n"));
        }
        for line in &hunk.added {
            md.push_str(&format!("+ {line}\n"));
        }
        for line in &hunk.context_after {
            md.push_str(&format!("  {line}\n"));
        }
    }
    md.push_str("```");
    md
}

/// 工具一：在当前项目内新建文档（create 无覆盖风险 → 免审批直接提交）
pub struct CreateDocumentTool {
    db_path: PathBuf,
    notes_dir: PathBuf,
    project_id: String,
    #[allow(dead_code)]
    run_id: String,
}

impl CreateDocumentTool {
    pub fn new(db_path: PathBuf, notes_dir: PathBuf, project_id: String, run_id: String) -> Self {
        Self {
            db_path,
            notes_dir,
            project_id,
            run_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for CreateDocumentTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "create_document".into(),
            description:
                "在当前项目内新建一篇 Markdown 文档并写入初始正文。立即生效（创建无覆盖风险，免审批）。\
重复请求请携带同一个 idempotencyKey，避免重复创建。"
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "文档标题（非空）" },
                    "content": { "type": "string", "description": "初始正文（Markdown，可为空）" },
                    "idempotencyKey": { "type": "string", "description": "幂等键（同一创建请求保持不变）" }
                },
                "required": ["title"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let title = str_arg(&arguments, "title")?;
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let idempotency_key = opt_str_arg(&arguments, "idempotencyKey");

        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| ToolError::Execution(format!("打开数据库失败: {e}")))?;
        let created = service::create_document_in_project(
            &conn,
            &self.notes_dir,
            &self.project_id,
            &title,
            &content,
            idempotency_key.as_deref(),
        )
        .map_err(service_err)?;

        let model_text = format!(
            "已创建文档《{}》（articleId: {}，version: {}）。",
            created.title, created.article_id, created.version
        );
        let structured = serde_json::json!({
            "projectId": self.project_id,
            "articleId": created.article_id,
            "title": created.title,
            "version": created.version,
        });
        let provenance = vec![ProvenanceRef::new("project-document")
            .with_id(&created.article_id)
            .with_title(&created.title)];
        Ok(ToolOutput {
            model_text,
            structured,
            ui_artifact: None,
            provenance,
            truncated: false,
        })
    }
}

/// 在当前项目内设置文档的父节点。项目组织树采用“文档即目录节点”的
/// Notion 式模型；传空 parentArticleId 可把文档移回项目根。
pub struct SetDocumentParentTool {
    db_path: PathBuf,
    project_id: String,
}

impl SetDocumentParentTool {
    pub fn new(db_path: PathBuf, project_id: String) -> Self {
        Self {
            db_path,
            project_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for SetDocumentParentTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "set_document_parent".into(),
            description: "在当前项目内设置一篇文档的父文档，用于生成包含子目录的项目文档树。文档即目录节点；parentArticleId 为空表示移回项目根。只允许同项目文档，且会拒绝循环层级。".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "articleId": { "type": "string", "description": "需要移动的文档 ID" },
                    "parentArticleId": {
                        "type": ["string", "null"],
                        "description": "父文档 ID；null 或空字符串表示项目根"
                    }
                },
                "required": ["articleId"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let article_id = str_arg(&arguments, "articleId")?;
        let parent_article_id = opt_str_arg(&arguments, "parentArticleId");
        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|error| ToolError::Execution(format!("打开数据库失败: {error}")))?;
        crate::project_tree::set_doc_parent_in_project(
            &conn,
            Some(&self.project_id),
            &article_id,
            parent_article_id.as_deref(),
        )
        .map_err(ToolError::Execution)?;

        Ok(ToolOutput {
            model_text: if let Some(parent_id) = parent_article_id.as_deref() {
                format!("已将文档 {article_id} 移到父文档 {parent_id} 下。")
            } else {
                format!("已将文档 {article_id} 移回项目根。")
            },
            structured: serde_json::json!({
                "projectId": self.project_id,
                "articleId": article_id,
                "parentArticleId": parent_article_id,
            }),
            ui_artifact: None,
            provenance: vec![ProvenanceRef::new("project-document").with_id(&article_id)],
            truncated: false,
        })
    }
}

/// 工具二：提议修改文档（恒 dry-run）。只产 diff 提案 + operation（proposed），
/// 零文件写入；落盘经用户批准（document_apply_patch 命令）。
pub struct ProposeDocumentPatchTool {
    db_path: PathBuf,
    notes_dir: PathBuf,
    project_id: String,
    run_id: String,
}

impl ProposeDocumentPatchTool {
    pub fn new(db_path: PathBuf, notes_dir: PathBuf, project_id: String, run_id: String) -> Self {
        Self {
            db_path,
            notes_dir,
            project_id,
            run_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for ProposeDocumentPatchTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "propose_document_patch".into(),
            description: "提议修改当前项目内一篇文档（dry-run，只生成 diff 预览，不会写入文件）。\
有选区时用 scope=selection 并原样传 selectionAnchor（selectedText/selectedTextHash/\
beforeContext/afterContext，来自 SelectionSnapshot）；无选区按块/章节修改用 scope=block 或 \
section 并给 expectedText。锚点必须与文档当前正文逐字一致且能唯一定位（先 read_document 确认）；\
baseVersion 是 read_document 返回的版本号。整篇 scope 不提供。用户的修改指令即授权生成提案，\
无需在 Chat 中再次确认；提案会在原文中显示 ✓/×，点击 ✓ 才会写入。"
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "articleId": { "type": "string", "description": "文档 ID" },
                    "baseVersion": { "type": "integer", "description": "读取文档时的版本号（read_document 返回的 version）" },
                    "scope": { "type": "string", "enum": ["selection", "block", "section"], "description": "修改范围：selection=用户显式选区，block=当前块，section=指定章节（整篇不提供）" },
                    "selectionAnchor": {
                        "type": "object",
                        "description": "选区锚点（SelectionSnapshot 捕获，原样传递，勿改写）",
                        "properties": {
                            "selectedText": { "type": "string", "description": "选区原文（逐字）" },
                            "selectedTextHash": { "type": "string", "description": "选区文本 hash（捕获侧生成）" },
                            "beforeContext": { "type": "string", "description": "选区前文" },
                            "afterContext": { "type": "string", "description": "选区后文" }
                        },
                        "required": ["selectedText"]
                    },
                    "expectedText": { "type": "string", "description": "无 selectionAnchor 时的锚点原文（逐字匹配且唯一）" },
                    "replacementMarkdown": { "type": "string", "description": "替换后的新文本（Markdown）" },
                    "idempotencyKey": { "type": "string", "description": "幂等键（同一提案保持不变）" }
                },
                "required": ["articleId", "baseVersion", "replacementMarkdown"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let article_id = str_arg(&arguments, "articleId")?;
        let base_version = arguments
            .get("baseVersion")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| {
                ToolError::InvalidArguments("缺少必填整数参数 baseVersion".to_string())
            })?;
        let expected_text = opt_str_arg(&arguments, "expectedText");
        let replacement = str_arg(&arguments, "replacementMarkdown")?;
        let idempotency_key = opt_str_arg(&arguments, "idempotencyKey");
        let scope_raw = opt_str_arg(&arguments, "scope");
        let selection_anchor: Option<crate::documents::anchor::TextAnchor> = match arguments
            .get("selectionAnchor")
        {
            Some(v) if !v.is_null() => Some(serde_json::from_value(v.clone()).map_err(|e| {
                ToolError::InvalidArguments(format!(
                    "selectionAnchor 结构非法（需含 selectedText，camelCase）: {e}"
                ))
            })?),
            _ => None,
        };

        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| ToolError::Execution(format!("打开数据库失败: {e}")))?;
        let preview = match scope_raw.as_deref() {
            // AG-24 文本锚点路径（未带 scope，保持旧契约）
            None => {
                let expected_text = expected_text.ok_or_else(|| {
                    ToolError::InvalidArguments("缺少必填字符串参数 expectedText".to_string())
                })?;
                service::preview_patch(
                    &conn,
                    &self.notes_dir,
                    &article_id,
                    base_version,
                    &expected_text,
                    &replacement,
                    idempotency_key.as_deref(),
                    Some(&self.run_id),
                    Some(&self.project_id),
                )
            }
            // AG-25 选区感知路径：scope + TextAnchor
            Some("selection") | Some("block") | Some("section") => {
                let scope = match scope_raw.as_deref() {
                    Some("selection") => service::PatchScope::Selection,
                    Some("block") => service::PatchScope::CurrentBlock,
                    _ => service::PatchScope::Section,
                };
                let anchor = match selection_anchor {
                    Some(a) => a,
                    None => {
                        let text = expected_text.ok_or_else(|| {
                            ToolError::InvalidArguments(
                                "scope 模式需要 selectionAnchor 或 expectedText 作为锚点"
                                    .to_string(),
                            )
                        })?;
                        crate::documents::anchor::TextAnchor {
                            selected_text: text,
                            selected_text_hash: String::new(),
                            before_context: String::new(),
                            after_context: String::new(),
                        }
                    }
                };
                service::preview_scoped_patch(
                    &conn,
                    &self.notes_dir,
                    &service::ScopedPatchRequest {
                        document_id: article_id.clone(),
                        base_version,
                        scope,
                        anchor,
                        replacement_markdown: replacement,
                        idempotency_key,
                        run_id: Some(self.run_id.clone()),
                        project_gate: Some(self.project_id.clone()),
                    },
                )
            }
            // 整篇重写 = R3 高风险，独立工具，默认不向模型开放（docs/architecture.md）
            Some("document") => {
                return Err(ToolError::InvalidArguments(
                    "scope=document（整篇重写）是 R3 高风险操作，当前不提供；\
请仅按 selection/block/section 范围修改"
                        .to_string(),
                ));
            }
            Some(other) => {
                return Err(ToolError::InvalidArguments(format!(
                    "未知 scope: {other}（可用值：selection/block/section）"
                )));
            }
        }
        .map_err(service_err)?;

        let diff_md = render_diff_markdown(&preview);
        let scope_note = preview
            .scope
            .map(|s| format!("，范围：{}", s.label()))
            .unwrap_or_default();
        let rebase_note = if preview.rebased {
            "（安全 rebase：文档版本已变化但锚点仍有效，已按最新版本重出 diff）"
        } else {
            ""
        };
        let model_text = format!(
            "已在文档《{}》原文中显示修改建议（operationId: {}{scope_note}，v{} → v{}）。\
这是 dry-run：文件尚未修改{rebase_note}；原文中的 ✓ 会应用，× 会放弃。\
不要在 Chat 中询问、要求或等待用户批准，也不要重复输出 diff；只需简短告知建议已显示。\
如报版本冲突或锚点冲突，请重新 read_document 后再提议，绝不猜测覆盖。",
            preview.title, preview.operation_id, preview.base_version, preview.target_version
        );
        let structured = serde_json::to_value(&preview)
            .map_err(|e| ToolError::Execution(format!("预览序列化失败: {e}")))?;
        let provenance = vec![ProvenanceRef::new("project-document")
            .with_id(&article_id)
            .with_title(&preview.title)];
        let artifact = UiArtifact::new("diff", structured.clone(), diff_md, provenance.clone())?;
        Ok(ToolOutput {
            model_text,
            structured,
            ui_artifact: Some(artifact),
            provenance,
            truncated: false,
        })
    }
}

/// 工具三：移动文档到另一个项目（归属变更，不改正文）
pub struct MoveDocumentTool {
    db_path: PathBuf,
    project_id: String,
}

impl MoveDocumentTool {
    pub fn new(db_path: PathBuf, project_id: String) -> Self {
        Self {
            db_path,
            project_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for MoveDocumentTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "move_document".into(),
            description:
                "把当前项目内的一篇文档移动到另一个项目（只变更归属，不改正文）。只能移动本项目文档。"
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "articleId": { "type": "string", "description": "文档 ID" },
                    "targetProjectId": { "type": "string", "description": "目标项目 ID" }
                },
                "required": ["articleId", "targetProjectId"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let article_id = str_arg(&arguments, "articleId")?;
        let target_project_id = str_arg(&arguments, "targetProjectId")?;

        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| ToolError::Execution(format!("打开数据库失败: {e}")))?;
        service::move_document(&conn, &article_id, &target_project_id, &self.project_id)
            .map_err(service_err)?;
        let model_text = format!(
            "已把文档 {} 从项目 {} 移动到项目 {}。",
            article_id, self.project_id, target_project_id
        );
        let structured = serde_json::json!({
            "articleId": article_id,
            "fromProjectId": self.project_id,
            "toProjectId": target_project_id,
        });
        Ok(ToolOutput::text(model_text, structured))
    }
}

/// 工具四：提议重命名文档标题（恒 dry-run，只产提案，零写入）。
///
/// 改名完整语义（SQLite `articles.title` + 文件 frontmatter + [[双链]] 级联
/// 改写 + 语义索引重建）由 SophoNote 前端 `updateArticleTitle` 在用户批准后
/// 执行；本工具只做归属校验、标题合法性/重名检查与双链影响扫描，
/// 返回 `pending_approval` 提案卡片（UiArtifact kind = rename）。
pub struct RenameArticleTool {
    db_path: PathBuf,
    notes_dir: PathBuf,
    project_id: String,
    #[allow(dead_code)]
    run_id: String,
}

impl RenameArticleTool {
    pub fn new(db_path: PathBuf, notes_dir: PathBuf, project_id: String, run_id: String) -> Self {
        Self {
            db_path,
            notes_dir,
            project_id,
            run_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for RenameArticleTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "rename_article".into(),
            description:
                "提议把当前项目内一篇文档的标题改为新标题（dry-run，只生成提案，不落盘）。\
改名会同步 SQLite 与文件 frontmatter，并把其它文档中的 [[旧标题]] 双链改写为 [[新标题]]；\
用户批准后由 SophoNote 执行完整改名。标题不能与项目内其它文档重名，长度不超过 200 字符。"
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "articleId": { "type": "string", "description": "文档 ID" },
                    "newTitle": { "type": "string", "description": "新标题（非空、不含换行、≤ 200 字符）" }
                },
                "required": ["articleId", "newTitle"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let article_id = str_arg(&arguments, "articleId")?;
        let new_title = str_arg(&arguments, "newTitle")?;
        if new_title.chars().count() > 200 {
            return Err(ToolError::InvalidArguments(
                "标题过长（上限 200 字符）".to_string(),
            ));
        }
        if new_title.contains('\n') || new_title.contains('\r') {
            return Err(ToolError::InvalidArguments("标题不能包含换行".to_string()));
        }

        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| ToolError::Execution(format!("打开数据库失败: {e}")))?;

        // 归属闸门（与 read_document 同口径）：仅当前项目成员可改名
        let member_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1 AND article_id = ?2",
                rusqlite::params![self.project_id, article_id],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("归属校验失败: {e}")))?;
        if member_count == 0 {
            return Err(ToolError::Execution(
                "该文档不属于当前项目或不存在，无法改名".to_string(),
            ));
        }

        let old_title: String = conn
            .query_row(
                "SELECT title FROM articles WHERE id = ?1",
                rusqlite::params![article_id],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("读取文档标题失败: {e}")))?;

        if old_title == new_title {
            return Err(ToolError::InvalidArguments(
                "新标题与当前标题相同，无需改名".to_string(),
            ));
        }

        // 重名检查：同项目其它文档（不含自身）
        let dup_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_documents d \
                 JOIN articles a ON a.id = d.article_id \
                 WHERE d.project_id = ?1 AND d.article_id != ?2 AND a.title = ?3",
                rusqlite::params![self.project_id, article_id, new_title],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("重名检查失败: {e}")))?;
        if dup_count > 0 {
            return Err(ToolError::InvalidArguments(format!(
                "项目内已存在同名文档《{new_title}》，请换一个标题"
            )));
        }

        // 双链影响扫描：统计项目内其它文档正文包含 [[旧标题 前缀的篇数
        // （覆盖 [[旧]] / [[旧|别名]] / [[旧#标题]] 三形态；前缀匹配可能含误报，
        // 仅用于向用户展示影响范围，不做精确改写承诺）
        let mut affected = 0usize;
        {
            let mut stmt = conn
                .prepare(
                    "SELECT d.article_id FROM project_documents d \
                     JOIN articles a ON a.id = d.article_id \
                     WHERE d.project_id = ?1 AND d.article_id != ?2",
                )
                .map_err(|e| ToolError::Execution(format!("查询失败: {e}")))?;
            let rows = stmt
                .query_map(rusqlite::params![self.project_id, article_id], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(|e| ToolError::Execution(format!("查询失败: {e}")))?;
            let needle = format!("[[{old_title}");
            for row in rows {
                let other_id =
                    row.map_err(|e| ToolError::Execution(format!("读取结果失败: {e}")))?;
                if let Some(body) = crate::notes::read_article_body_in(&self.notes_dir, &other_id) {
                    if body.contains(&needle) {
                        affected += 1;
                    }
                }
            }
        }

        let operation_id = format!("rename-{}", uuid::Uuid::new_v4());
        let structured = serde_json::json!({
            "operationId": operation_id,
            "documentId": article_id,
            "oldTitle": old_title,
            "newTitle": new_title,
            "wikilinkAffectedCount": affected,
            "status": "pending_approval",
        });

        let model_text = format!(
            "已为文档《{old_title}》生成改名提案：改为《{new_title}》\
（{affected} 篇其它文档的双链将同步改写）。这是 dry-run：尚未落盘。\
请在界面确认卡片中点击「应用」完成改名，或忽略放弃；\
不要在 Chat 中询问、要求或等待用户批准，也不要重复输出提案内容。"
        );
        let fallback_md = format!(
            "将把《{old_title}》重命名为《{new_title}》（影响 {affected} 篇文档的双链）。\n\n\
如需应用，请在界面确认；若此处无确认入口，请忽略本提示。"
        );
        let provenance = vec![ProvenanceRef::new("project-document")
            .with_id(&article_id)
            .with_title(&old_title)];
        let artifact = UiArtifact::new(
            "rename",
            structured.clone(),
            fallback_md,
            provenance.clone(),
        )?;
        Ok(ToolOutput {
            model_text,
            structured,
            ui_artifact: Some(artifact),
            provenance,
            truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    //! 零模型测试：三工具的参数校验 / 范围闸门 / dry-run 契约。
    use super::*;
    use crate::documents::repository::tests::RepoFixture;

    fn seed_project(fx: &RepoFixture, project_id: &str) {
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES (?1, '测试项目')",
            rusqlite::params![project_id],
        )
        .unwrap();
    }

    fn seed_member(fx: &RepoFixture, project_id: &str, article_id: &str) {
        fx.seed_article(article_id, "项目笔记", "这是正文第一句。\n这是正文第二句。");
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id, added_at) VALUES (?1, ?2, 1)",
            rusqlite::params![project_id, article_id],
        )
        .unwrap();
    }

    /// agent_approvals.run_id 有 FK 约束——工具绑定 run_id 建审批前须有 Run 行
    /// （真实路径 Run 恒先于工具存在；测试显式补齐，同 service.rs reject 用例口径）
    fn seed_run(fx: &RepoFixture, run_id: &str) {
        let conn = fx.conn();
        conn.execute(
            "INSERT INTO agent_runs (id, thread_id, status, created_at, updated_at)
             VALUES (?1, 't1', 'running', 1, 1)",
            rusqlite::params![run_id],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn propose_patch_is_dry_run_and_emits_diff_artifact() {
        let fx = RepoFixture::setup("tool-propose");
        seed_project(&fx, "p1");
        seed_member(&fx, "p1", "a1");
        seed_run(&fx, "run-1");
        let tool = ProposeDocumentPatchTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        let out = tool
            .execute(serde_json::json!({
                "articleId": "a1",
                "baseVersion": 1,
                "expectedText": "这是正文第一句。",
                "replacementMarkdown": "这是新句。",
                "idempotencyKey": "k1"
            }))
            .await
            .unwrap();
        // dry-run 铁律：文件零变化
        let body = crate::notes::read_article_body_in(&fx.notes, "a1").unwrap();
        assert!(body.contains("这是正文第一句。"), "提案不得写文件");
        // diff 卡片 + 来源
        let artifact = out.ui_artifact.expect("应产 diff 卡片");
        assert_eq!(artifact.kind, "diff");
        assert!(artifact.fallback_markdown.contains("- 这是正文第一句。"));
        assert!(artifact.fallback_markdown.contains("+ 这是新句。"));
        assert_eq!(out.provenance[0].source, "project-document");
        assert!(out.model_text.contains("原文中的 ✓"));
        assert!(out.model_text.contains("不要在 Chat 中询问"));
        assert!(!out.model_text.contains("请确认是否批准"));
        assert!(!out.model_text.contains("等待用户批准。"));
        // 审批行已创建（run_id 绑定）
        let conn = fx.conn();
        let approvals: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_approvals WHERE run_id = 'run-1' AND status = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(approvals, 1);
        // 幂等重入：同键同 operation
        let out2 = tool
            .execute(serde_json::json!({
                "articleId": "a1",
                "baseVersion": 1,
                "expectedText": "这是正文第一句。",
                "replacementMarkdown": "这是新句。",
                "idempotencyKey": "k1"
            }))
            .await
            .unwrap();
        assert_eq!(
            out2.structured["operationId"],
            out.structured["operationId"]
        );
    }

    #[tokio::test]
    async fn propose_patch_cross_project_rejected() {
        let fx = RepoFixture::setup("tool-gate");
        seed_project(&fx, "p1");
        seed_project(&fx, "p2");
        seed_member(&fx, "p2", "a1");
        let tool = ProposeDocumentPatchTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        let err = tool
            .execute(serde_json::json!({
                "articleId": "a1",
                "baseVersion": 1,
                "expectedText": "这是正文第一句。",
                "replacementMarkdown": "x"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(err.to_string().contains("不属于当前项目"));
    }

    #[tokio::test]
    async fn propose_patch_missing_args_rejected() {
        let fx = RepoFixture::setup("tool-args");
        let tool = ProposeDocumentPatchTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        let err = tool
            .execute(serde_json::json!({"articleId": "a1"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        let err = tool
            .execute(
                serde_json::json!({"articleId": "a1", "baseVersion": "not-int",
                "expectedText": "x", "replacementMarkdown": "y"}),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn scoped_propose_with_selection_anchor_is_dry_run() {
        // AG-25：scope=selection + selectionAnchor → 选区感知预览（dry-run + diff 卡片）
        let fx = RepoFixture::setup("tool-scoped");
        seed_project(&fx, "p1");
        seed_member(&fx, "p1", "a1");
        seed_run(&fx, "run-1");
        let tool = ProposeDocumentPatchTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        let hash = crate::documents::repository::content_hash("这是正文第一句。");
        let out = tool
            .execute(serde_json::json!({
                "articleId": "a1",
                "baseVersion": 1,
                "scope": "selection",
                "selectionAnchor": {
                    "selectedText": "这是正文第一句。",
                    "selectedTextHash": hash,
                    "beforeContext": "",
                    "afterContext": "\n这是正文第二句。"
                },
                "replacementMarkdown": "这是新句。",
                "idempotencyKey": "sk1"
            }))
            .await
            .unwrap();
        // dry-run 铁律：文件零变化
        let body = crate::notes::read_article_body_in(&fx.notes, "a1").unwrap();
        assert!(body.contains("这是正文第一句。"), "提案不得写文件");
        assert_eq!(out.structured["scope"], "selection");
        assert_eq!(out.structured["rebased"], false);
        let artifact = out.ui_artifact.expect("应产 diff 卡片");
        assert_eq!(artifact.kind, "diff");
        assert!(out.provenance[0].source == "project-document");
    }

    #[tokio::test]
    async fn scoped_propose_rejects_full_document_and_unknown_scope() {
        let fx = RepoFixture::setup("tool-scope-gate");
        seed_project(&fx, "p1");
        seed_member(&fx, "p1", "a1");
        let tool = ProposeDocumentPatchTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        // 整篇 = R3 高风险，不提供
        let err = tool
            .execute(serde_json::json!({
                "articleId": "a1", "baseVersion": 1, "scope": "document",
                "expectedText": "这是正文第一句。", "replacementMarkdown": "x"
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("R3"));
        // 未知 scope
        let err = tool
            .execute(serde_json::json!({
                "articleId": "a1", "baseVersion": 1, "scope": "chapter",
                "expectedText": "这是正文第一句。", "replacementMarkdown": "x"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn scoped_propose_stale_anchor_conflicts() {
        // AG-25 完成判据：版本变化且目标已变 → conflict，不猜测覆盖
        let fx = RepoFixture::setup("tool-scoped-conflict");
        seed_project(&fx, "p1");
        seed_member(&fx, "p1", "a1");
        seed_run(&fx, "run-1");
        let tool = ProposeDocumentPatchTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        // 用户并发编辑（版本 1→2，选区目标消失）
        {
            let conn = fx.conn();
            crate::documents::repository::write_body(
                &conn,
                &fx.notes,
                "a1",
                "用户重写后的全新正文。",
                None,
            )
            .unwrap();
        }
        let hash = crate::documents::repository::content_hash("这是正文第一句。");
        let err = tool
            .execute(serde_json::json!({
                "articleId": "a1",
                "baseVersion": 1,
                "scope": "selection",
                "selectionAnchor": { "selectedText": "这是正文第一句。", "selectedTextHash": hash },
                "replacementMarkdown": "这是新句。"
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("版本冲突"));
    }

    #[tokio::test]
    async fn create_document_tool_writes_and_binds_membership() {
        let fx = RepoFixture::setup("tool-create");
        seed_project(&fx, "p1");
        let tool = CreateDocumentTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        let out = tool
            .execute(serde_json::json!({
                "title": "Agent 建的文档",
                "content": "初始正文",
                "idempotencyKey": "ck"
            }))
            .await
            .unwrap();
        let article_id = out.structured["articleId"].as_str().unwrap().to_string();
        assert_eq!(out.structured["version"], 1);
        let conn = fx.conn();
        let member: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_documents WHERE project_id = 'p1' AND article_id = ?1",
                rusqlite::params![article_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(member, 1);
        // 幂等重入 → 同 articleId
        let out2 = tool
            .execute(serde_json::json!({
                "title": "Agent 建的文档",
                "content": "初始正文",
                "idempotencyKey": "ck"
            }))
            .await
            .unwrap();
        assert_eq!(out2.structured["articleId"], article_id.as_str());
        // 空标题 → InvalidArguments
        let err = tool
            .execute(serde_json::json!({"title": "  "}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn set_document_parent_builds_nested_tree_and_rejects_cycles() {
        let fx = RepoFixture::setup("tool-document-parent");
        seed_project(&fx, "p1");
        for article_id in ["root", "child", "grandchild"] {
            seed_member(&fx, "p1", article_id);
        }
        let tool = SetDocumentParentTool::new(fx.db_path.clone(), "p1".into());
        tool.execute(serde_json::json!({
            "articleId": "child",
            "parentArticleId": "root"
        }))
        .await
        .unwrap();
        tool.execute(serde_json::json!({
            "articleId": "grandchild",
            "parentArticleId": "child"
        }))
        .await
        .unwrap();

        let conn = fx.conn();
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM project_documents WHERE article_id = 'grandchild'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent.as_deref(), Some("child"));

        let error = tool
            .execute(serde_json::json!({
                "articleId": "root",
                "parentArticleId": "grandchild"
            }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("循环"));
    }

    #[tokio::test]
    async fn move_document_tool_transfers_and_gates() {
        let fx = RepoFixture::setup("tool-move");
        seed_project(&fx, "p1");
        seed_project(&fx, "p2");
        seed_member(&fx, "p1", "a1");
        let tool = MoveDocumentTool::new(fx.db_path.clone(), "p1".into());
        let out = tool
            .execute(serde_json::json!({"articleId": "a1", "targetProjectId": "p2"}))
            .await
            .unwrap();
        assert_eq!(out.structured["toProjectId"], "p2");
        // 已不在 p1：再次移动必须被闸门拒绝
        let err = tool
            .execute(serde_json::json!({"articleId": "a1", "targetProjectId": "p1"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不属于当前项目"));
    }

    #[tokio::test]
    async fn rename_article_emits_rename_proposal_dry_run() {
        let fx = RepoFixture::setup("tool-rename");
        seed_project(&fx, "p1");
        fx.seed_article("a1", "旧标题", "正文内容");
        // 另一篇引用 [[旧标题]] 双链的文档（覆盖 [[旧]] / [[旧|别名]] / [[旧#章节]] 形态）
        fx.seed_article(
            "a2",
            "引用者",
            "参见 [[旧标题]] 和 [[旧标题|别名]] 与 [[旧标题#章节]]",
        );
        let conn = fx.conn();
        for aid in ["a1", "a2"] {
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id, added_at) VALUES (?1, ?2, 1)",
                rusqlite::params!["p1", aid],
            )
            .unwrap();
        }
        let tool = RenameArticleTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        let out = tool
            .execute(serde_json::json!({"articleId": "a1", "newTitle": "新标题"}))
            .await
            .unwrap();
        // dry-run 铁律：文件与 DB 标题均零变化
        let body = crate::notes::read_article_body_in(&fx.notes, "a1").unwrap();
        assert!(body.contains("正文内容"));
        let conn2 = fx.conn();
        let title: String = conn2
            .query_row("SELECT title FROM articles WHERE id = 'a1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "旧标题");
        // 提案卡片 + 结构化载荷
        let artifact = out.ui_artifact.expect("应产 rename 卡片");
        assert_eq!(artifact.kind, "rename");
        assert!(artifact.fallback_markdown.contains("旧标题"));
        assert!(artifact.fallback_markdown.contains("新标题"));
        assert_eq!(out.structured["oldTitle"], "旧标题");
        assert_eq!(out.structured["newTitle"], "新标题");
        assert_eq!(out.structured["status"], "pending_approval");
        assert_eq!(out.structured["wikilinkAffectedCount"], 1);
        assert_eq!(out.provenance[0].source, "project-document");
        assert!(out.model_text.contains("dry-run"));
        assert!(out.model_text.contains("不要在 Chat 中询问"));
    }

    #[tokio::test]
    async fn rename_article_rejects_cross_project_duplicate_same_and_bad_args() {
        let fx = RepoFixture::setup("tool-rename-gate");
        seed_project(&fx, "p1");
        fx.seed_article("a1", "旧标题", "正文");
        fx.seed_article("a2", "新标题", "另一篇");
        let conn = fx.conn();
        for aid in ["a1", "a2"] {
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id, added_at) VALUES (?1, ?2, 1)",
                rusqlite::params!["p1", aid],
            )
            .unwrap();
        }
        let tool = RenameArticleTool::new(
            fx.db_path.clone(),
            fx.notes.clone(),
            "p1".into(),
            "run-1".into(),
        );
        // 非项目成员（不存在）→ 拒绝
        let err = tool
            .execute(serde_json::json!({"articleId": "ghost", "newTitle": "x"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不属于当前项目"));
        // 重名 → 拒绝
        let err = tool
            .execute(serde_json::json!({"articleId": "a1", "newTitle": "新标题"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        assert!(err.to_string().contains("已存在同名文档"));
        // 相同标题 → 拒绝
        let err = tool
            .execute(serde_json::json!({"articleId": "a1", "newTitle": "旧标题"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        // 含换行 → 拒绝
        let err = tool
            .execute(serde_json::json!({"articleId": "a1", "newTitle": "a\nb"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        // 超长（>200 字符）→ 拒绝
        let long = "长".repeat(201);
        let err = tool
            .execute(serde_json::json!({"articleId": "a1", "newTitle": long}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        // 缺参数 → 拒绝
        let err = tool
            .execute(serde_json::json!({"articleId": "a1"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
