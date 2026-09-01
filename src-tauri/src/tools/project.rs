// ============================================================
// Track B · 智能体演进（AG-19 追加）：项目范围只读工具
// 用途：Phase 2 Go 门禁（docs/architecture.md：验收 =「读取当前项目
// 真实文档并基于来源总结」+ 多轮有效；docs/architecture.md 明确 Phase 2 允许
// 只读工具使用 Project/Article）。
//
// 铁律：
// ① 只读——仅 SELECT 与读 .md 文件，零写入（硬性限制④；写入 Phase 3 起
//    走 DocumentService）；
// ② 范围隔离——工具在 agent_run_start 按 Thread 归属的 project_id 逐 Run 构造，
//    跨项目文档一律拒绝，模型无法触达未授权材料（A13「不能跨项目越权」）；
// ③ Phase 2 范围内数据访问收口在本文件；Phase 3 切换到 DocumentRepository
//    读接口（docs/architecture.md「工具不直接访问数据库」的正式形态）。
// ============================================================
use std::path::PathBuf;

use async_trait::async_trait;

use super::{SophoNoteTool, ProvenanceRef, ToolDescriptor, ToolError, ToolOutput, UiArtifact};

/// 单次 read_document 回填上限（超出截断；可用 offset 分页续读）——
/// 防止一篇长文单次吃掉上下文窗口（max_turns 预算内还要留工具往返空间）
const MAX_DOCUMENT_CHARS: usize = 8000;
const MIN_PAGE_CHARS: usize = 500;
const HARD_MAX_PAGE_CHARS: usize = 16000;

/// 工具一：列出当前项目的文档清单。
/// 无参数——project_id 在构造时绑定（范围隔离），模型只需决定「要不要列」。
pub struct ListProjectDocumentsTool {
    db_path: PathBuf,
    project_id: String,
}

impl ListProjectDocumentsTool {
    pub fn new(db_path: PathBuf, project_id: String) -> Self {
        Self {
            db_path,
            project_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for ListProjectDocumentsTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_project_documents".into(),
            description: "列出当前项目下的全部文档，返回每篇的标题与 articleId。\
阅读某篇文档内容前，先用本工具拿到它的 articleId，再调用 read_document。"
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| ToolError::Execution(format!("打开数据库失败: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT d.article_id, COALESCE(a.title, ''), COALESCE(a.article_type, '')
                 FROM project_documents d
                 LEFT JOIN articles a ON a.id = d.article_id
                 WHERE d.project_id = ?1
                 ORDER BY d.added_at ASC",
            )
            .map_err(|e| ToolError::Execution(format!("查询失败: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![self.project_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| ToolError::Execution(format!("查询失败: {e}")))?;

        let mut documents: Vec<(String, String, String)> = Vec::new();
        for r in rows {
            documents.push(r.map_err(|e| ToolError::Execution(format!("读取结果失败: {e}")))?);
        }

        let model_text = if documents.is_empty() {
            "当前项目还没有文档。".to_string()
        } else {
            let mut text = format!("当前项目共有 {} 篇文档：\n", documents.len());
            for (id, title, _) in &documents {
                text.push_str(&format!("- 《{}》（articleId: {}）\n", title, id));
            }
            text
        };

        let structured = serde_json::json!({
            "projectId": self.project_id,
            "documents": documents
                .iter()
                .map(|(id, title, article_type)| serde_json::json!({
                    "articleId": id,
                    "title": title,
                    "articleType": article_type
                }))
                .collect::<Vec<_>>(),
        });

        // AG-21：清单类结果走 text 便捷构造（无卡片需求，structured 已够前端列表用）
        Ok(ToolOutput::text(model_text, structured))
    }
}

/// 工具二：读取当前项目内单篇文档的正文。
/// articleId 必填；非本项目成员（含不存在）一律拒绝——
/// 不区分「属于其它项目」与「不存在」，避免泄漏库内成员信息。
pub struct ReadDocumentTool {
    db_path: PathBuf,
    notes_dir: PathBuf,
    project_id: String,
}

impl ReadDocumentTool {
    pub fn new(db_path: PathBuf, notes_dir: PathBuf, project_id: String) -> Self {
        Self {
            db_path,
            notes_dir,
            project_id,
        }
    }
}

#[async_trait]
impl SophoNoteTool for ReadDocumentTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_document".into(),
            description: "读取当前项目内一篇文档的正文（Markdown，已去除 frontmatter）。\
正文来自 SophoNote notes/<articleId>.md，不是用户任意本机路径。\
参数 articleId 来自 list_project_documents 或 workspace_state。只能读取本项目文档。\
长文请用 offset 分页续读（见返回的 nextOffset），禁止改用 read_file/终端扫描磁盘。\
返回值含 version——后续 propose_document_patch 必须把它作为 baseVersion。"
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "articleId": {
                        "type": "string",
                        "description": "文档 ID（list_project_documents / workspace_state 的 articleId）"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "字符偏移（从 0 起）；续读时用上次返回的 nextOffset",
                        "minimum": 0
                    },
                    "maxChars": {
                        "type": "integer",
                        "description": "本页最多返回字符数（默认 8000，上限 16000）",
                        "minimum": 500,
                        "maximum": 16000
                    }
                },
                "required": ["articleId"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let article_id = arguments
            .get("articleId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                ToolError::InvalidArguments("缺少必填字符串参数 articleId".to_string())
            })?;

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let max_chars = arguments
            .get("maxChars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(MIN_PAGE_CHARS, HARD_MAX_PAGE_CHARS))
            .unwrap_or(MAX_DOCUMENT_CHARS);

        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| ToolError::Execution(format!("打开数据库失败: {e}")))?;

        // 归属闸门（AG-19 核心护栏）：仅当前项目成员可读
        let member_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1 AND article_id = ?2",
                rusqlite::params![self.project_id, article_id],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("归属校验失败: {e}")))?;
        if member_count == 0 {
            return Err(ToolError::Execution(
                "该文档不属于当前项目或不存在，无法读取".to_string(),
            ));
        }

        let title: String = conn
            .query_row(
                "SELECT title FROM articles WHERE id = ?1",
                rusqlite::params![article_id],
                |row| row.get(0),
            )
            .map_err(|e| ToolError::Execution(format!("读取文档标题失败: {e}")))?;

        // 正文版本号（AG-24：propose_document_patch 的 baseVersion 真相源，
        // 经 DocumentRepository 读接口，工具不直接查版本字段）
        let version = crate::documents::repository::current_version(&conn, article_id)
            .map_err(|e| ToolError::Execution(e.to_string()))?
            .unwrap_or(1);

        // 正文来自笔记文件（与笔记本同一落盘口径，notes::read_article_body_in 剥 frontmatter）。
        // 剥离后常残留一个前导空行（文件书写口径 ---\n…\n---\n\n），模型消费侧归一掉
        let body = crate::notes::read_article_body_in(&self.notes_dir, article_id)
            .map(|b| b.trim_start_matches('\n').to_string())
            .ok_or_else(|| {
                ToolError::Execution("笔记文件读取失败（文件可能未落盘）".to_string())
            })?;

        let char_count = body.chars().count();
        let start = offset.min(char_count);
        let page: String = body.chars().skip(start).collect();
        let page_len = page.chars().count();
        let truncated = page_len > max_chars;
        let content: String = if truncated {
            page.chars().take(max_chars).collect()
        } else {
            page
        };
        let returned = content.chars().count();
        let next_offset = if truncated {
            Some(start + returned)
        } else {
            None
        };

        let mut model_text = format!(
            "文档《{}》（articleId: {}，version: {}，全文 {} 字符，本页 offset={}..{}）的正文：\n{}",
            title,
            article_id,
            version,
            char_count,
            start,
            start + returned,
            content
        );
        if let Some(next) = next_offset {
            model_text.push_str(&format!(
                "\n……（本页已截断；继续阅读请再次调用 read_document，articleId={}，offset={}。禁止改用 read_file 或扫描本机目录。）",
                article_id, next
            ));
        }

        let structured = serde_json::json!({
            "projectId": self.project_id,
            "articleId": article_id,
            "title": title,
            "content": content,
            "version": version,
            "truncated": truncated,
            "offset": start,
            "returnedChars": returned,
            "totalChars": char_count,
            "nextOffset": next_offset,
        });

        // AG-21：来源可追溯（project-document + articleId + 标题）+ 截断标记贯通
        let provenance = vec![ProvenanceRef::new("project-document")
            .with_id(article_id)
            .with_title(&title)];
        let mut fallback = content.clone();
        if truncated {
            fallback.push_str("\n\n……（正文过长已截断，可用 offset 续读）");
        }
        let artifact = UiArtifact::new(
            "markdown",
            serde_json::json!({ "markdown": content }),
            fallback,
            provenance.clone(),
        )?;
        Ok(ToolOutput {
            model_text,
            structured,
            ui_artifact: Some(artifact),
            provenance,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 零模型夹具：临时目录 + create_schema 建库 + 种子数据。
    /// p1 含 a1（有笔记文件）；a2 属于 p2（越权测试用）；a3 只在库里无文件。
    struct Fixture {
        dir: PathBuf,
        db_path: PathBuf,
        notes: PathBuf,
    }

    impl Fixture {
        fn setup(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sophonote-ag19-{}-{}-{}",
                tag,
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let notes = dir.join("notes");
            fs::create_dir_all(&notes).unwrap();
            let db_path = dir.join("sophonote.db");
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            crate::db::create_schema(&conn).unwrap();
            conn.execute(
                "INSERT INTO projects (id, name) VALUES ('p1', '项目一'), ('p2', '项目二')",
                rusqlite::params![],
            )
            .unwrap();
            for (id, title) in [
                ("a1", "测试笔记"),
                ("a2", "别的项目的笔记"),
                ("a3", "没有文件的笔记"),
            ] {
                conn.execute(
                    "INSERT INTO articles (id, title, content, article_type) VALUES (?1, ?2, '', 'manual')",
                    rusqlite::params![id, title],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id, added_at) VALUES
                 ('p1', 'a1', 1), ('p2', 'a2', 2), ('p1', 'a3', 3)",
                rusqlite::params![],
            )
            .unwrap();
            // a1 笔记文件：带 frontmatter，正文含可断言文本
            fs::write(
                notes.join("a1.md"),
                "---\nid: a1\ntitle: \"测试笔记\"\n---\n\n这是正文第一句。这是正文第二句。",
            )
            .unwrap();
            Self {
                dir,
                db_path,
                notes,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn list_returns_only_project_members() {
        let fx = Fixture::setup("list");
        let tool = ListProjectDocumentsTool::new(fx.db_path.clone(), "p1".into());
        let out = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(out.model_text.contains("《测试笔记》"));
        assert!(out.model_text.contains("a1"));
        assert!(
            !out.model_text.contains("别的项目的笔记"),
            "不得泄漏其它项目成员"
        );
        let docs = out.structured["documents"].as_array().unwrap();
        assert_eq!(docs.len(), 2, "p1 含 a1+a3，且不含 p2 的 a2");
        assert_eq!(docs[0]["articleId"], "a1");
        assert_eq!(docs[0]["title"], "测试笔记");
        assert_eq!(docs[1]["articleId"], "a3");
        assert_eq!(out.structured["projectId"], "p1");
    }

    #[tokio::test]
    async fn list_empty_project_says_so() {
        let fx = Fixture::setup("list-empty");
        let tool = ListProjectDocumentsTool::new(fx.db_path.clone(), "p-empty".into());
        let out = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(out.model_text.contains("没有文档"));
        assert!(out.structured["documents"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn read_member_returns_body_without_frontmatter_with_source() {
        let fx = Fixture::setup("read");
        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        let out = tool
            .execute(serde_json::json!({"articleId": "a1"}))
            .await
            .unwrap();
        assert!(out.model_text.contains("这是正文第一句"));
        assert!(!out.model_text.contains("title:"), "frontmatter 必须剥掉");
        assert_eq!(out.structured["title"], "测试笔记");
        assert_eq!(out.structured["projectId"], "p1");
        assert_eq!(out.structured["articleId"], "a1");
        assert_eq!(
            out.structured["version"], 1,
            "AG-24：version 是 patch baseVersion 来源"
        );
        assert_eq!(out.structured["truncated"], false);
        assert_eq!(
            out.structured["content"],
            "这是正文第一句。这是正文第二句。"
        );
        // AG-21：来源五件套贯通——provenance 指向 project-document + articleId/标题
        assert!(!out.truncated);
        let prov = &out.provenance[0];
        assert_eq!(prov.source, "project-document");
        assert_eq!(prov.source_id.as_deref(), Some("a1"));
        assert_eq!(prov.title.as_deref(), Some("测试笔记"));
        let artifact = out.ui_artifact.expect("read_document 应产 markdown 卡片");
        assert_eq!(artifact.kind, "markdown");
        assert!(artifact.fallback_markdown.contains("这是正文第一句"));
    }

    #[tokio::test]
    async fn read_supports_offset_pagination() {
        let fx = Fixture::setup("read-page");
        let long: String = (0..12000)
            .map(|i| if i % 10 == 0 { '。' } else { '字' })
            .collect();
        fs::write(
            fx.notes.join("a1.md"),
            format!("---\nid: a1\ntitle: \"测试笔记\"\n---\n\n{long}"),
        )
        .unwrap();
        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        let page1 = tool
            .execute(serde_json::json!({"articleId": "a1", "offset": 0, "maxChars": 1000}))
            .await
            .unwrap();
        assert_eq!(page1.structured["offset"], 0);
        assert_eq!(page1.structured["returnedChars"], 1000);
        assert_eq!(page1.structured["nextOffset"], 1000);
        assert!(page1.truncated);
        assert!(page1.model_text.contains("offset=1000"));

        let page2 = tool
            .execute(serde_json::json!({"articleId": "a1", "offset": 1000, "maxChars": 1000}))
            .await
            .unwrap();
        assert_eq!(page2.structured["offset"], 1000);
        assert_eq!(page2.structured["nextOffset"], 2000);
        let c1 = page1.structured["content"].as_str().unwrap();
        let c2 = page2.structured["content"].as_str().unwrap();
        assert_eq!(c1.chars().count(), 1000);
        assert_eq!(c2.chars().count(), 1000);
        // 连续切片：第 1 页末字应接在全文 offset=999，第 2 页首字为全文 offset=1000
        let full_body = crate::notes::read_article_body_in(&fx.notes, "a1")
            .unwrap()
            .trim_start_matches('\n')
            .to_string();
        let expected2: String = full_body.chars().skip(1000).take(1000).collect();
        assert_eq!(c2, expected2);
    }

    #[tokio::test]
    async fn read_cross_project_member_rejected() {
        let fx = Fixture::setup("deny-cross");
        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        // a2 属于 p2：绑定 p1 的工具必须拒绝（A13「不能跨项目越权」）
        let err = tool
            .execute(serde_json::json!({"articleId": "a2"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(err.to_string().contains("不属于当前项目"));
    }

    #[tokio::test]
    async fn read_missing_article_rejected() {
        let fx = Fixture::setup("deny-missing");
        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        let err = tool
            .execute(serde_json::json!({"articleId": "ghost"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
    }

    #[tokio::test]
    async fn read_missing_article_id_is_invalid_arguments() {
        let fx = Fixture::setup("bad-args");
        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        let err = tool.execute(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn read_member_without_file_reports_execution_error() {
        let fx = Fixture::setup("no-file");
        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        // a3 是项目成员但笔记文件缺失：错误可见，不假成功
        let err = tool
            .execute(serde_json::json!({"articleId": "a3"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        assert!(err.to_string().contains("笔记文件"));
    }

    #[tokio::test]
    async fn read_long_body_truncated_with_marker() {
        let fx = Fixture::setup("truncate");
        // 给 a1 之外的成员补一篇超长笔记：直接扩展夹具数据
        let conn = rusqlite::Connection::open(&fx.db_path).unwrap();
        conn.execute(
            "INSERT INTO articles (id, title, content, article_type) VALUES ('a-long', '长文', '', 'manual')",
            rusqlite::params![],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id, added_at) VALUES ('p1', 'a-long', 3)",
            rusqlite::params![],
        )
        .unwrap();
        drop(conn);
        let long_body: String = "字".repeat(MAX_DOCUMENT_CHARS + 500);
        fs::write(
            fx.notes.join("a-long.md"),
            format!("---\nid: a-long\n---\n\n{long_body}"),
        )
        .unwrap();

        let tool = ReadDocumentTool::new(fx.db_path.clone(), fx.notes.clone(), "p1".into());
        let out = tool
            .execute(serde_json::json!({"articleId": "a-long"}))
            .await
            .unwrap();
        assert_eq!(out.structured["truncated"], true);
        assert_eq!(
            out.structured["content"].as_str().unwrap().chars().count(),
            MAX_DOCUMENT_CHARS
        );
        assert!(out.model_text.contains("已截断"));
        // AG-21：截断标记贯通到 ToolOutput 顶层（卡片显「内容已截断」）
        assert!(out.truncated);
        assert!(out
            .ui_artifact
            .unwrap()
            .fallback_markdown
            .contains("已截断"));
    }
}
