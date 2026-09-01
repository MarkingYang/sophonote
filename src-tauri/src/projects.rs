// ============================================================
// Track B · 智能体演进（AG-02 · AI 工作室试验田 · 项目容器）
// 设计基线：docs/architecture.md
// 扁平分组容器（parent_id 预留升级文件夹树）；文档单一归属（move 语义），
// 是未来智能体「文件夹归属整理」能力的数据底座。
// 本轮写操作只触碰项目元数据与成员关系，零文档正文写入。
// ============================================================
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::commands::ApiResponse;
use crate::db::get_db_path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 扁平阶段恒为 NULL；有值即代表升级成文件夹树（设计文档 §三）
    #[serde(default)]
    pub parent_id: Option<String>,
    /// 派生字段：成员文档数（project_list 子查询带出；create 入参忽略）
    #[serde(default)]
    pub doc_count: i64,
    #[serde(default)]
    pub pinned: bool,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// 项目 ↔ 文档成员关系（前端派生 documentId → projectId 归属 map）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    pub project_id: String,
    pub article_id: String,
    /// NB-19（AG-11 落地）：项目内父文档（同项目另一 article_id）；NULL = 项目根。
    /// 跨项目 move 经 project_assign_document 的 INSERT OR REPLACE 天然重置为 NULL。
    #[serde(default)]
    pub parent_id: Option<String>,
}

fn open(app: &AppHandle) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())
}

/// 项目列表（含文档数），创建时间升序
#[tauri::command]
pub fn project_list(app: AppHandle) -> ApiResponse<Vec<Project>> {
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    let mut stmt = match conn.prepare(
        "SELECT p.id, p.name, p.description, p.parent_id, p.created_at, p.updated_at,
                CASE WHEN p.sort_order > 0 THEN 1 ELSE 0 END AS pinned,
                (SELECT COUNT(*) FROM project_documents d WHERE d.project_id = p.id) AS doc_count
         FROM projects p
         ORDER BY pinned DESC, p.created_at ASC",
    ) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let mapped = stmt.query_map([], |r| {
        Ok(Project {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            parent_id: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            pinned: r.get(6)?,
            doc_count: r.get(7)?,
        })
    });
    match mapped {
        Ok(rows) => ApiResponse::ok(rows.filter_map(|r| r.ok()).collect()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 新建项目（与 db_insert_item 同范式：前端生成 uuid 与 createdAt）
#[tauri::command]
pub fn project_create(app: AppHandle, project: Project) -> ApiResponse<Project> {
    let name = project.name.trim().to_string();
    if name.is_empty() {
        return ApiResponse::err("项目名称不能为空".to_string());
    }
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    match conn.execute(
        "INSERT INTO projects (id, name, description, parent_id, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULL, 0, ?4, NULL)",
        rusqlite::params![project.id, name, project.description, project.created_at],
    ) {
        Ok(_) => ApiResponse::ok(Project {
            name,
            doc_count: 0,
            pinned: false,
            parent_id: None,
            updated_at: None,
            ..project
        }),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 重命名项目
#[tauri::command]
pub fn project_rename(app: AppHandle, id: String, name: String) -> ApiResponse<String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return ApiResponse::err("项目名称不能为空".to_string());
    }
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    match conn.execute(
        "UPDATE projects SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![name, id],
    ) {
        Ok(n) if n > 0 => ApiResponse::ok("renamed".to_string()),
        Ok(_) => ApiResponse::err("项目不存在".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 置顶/取消置顶项目；不改 updated_at，避免影响项目自身更新时间语义。
#[tauri::command]
pub fn project_set_pinned(app: AppHandle, id: String, pinned: bool) -> ApiResponse<String> {
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    match conn.execute(
        "UPDATE projects SET sort_order = ?1 WHERE id = ?2",
        rusqlite::params![if pinned { 1 } else { 0 }, id],
    ) {
        Ok(n) if n > 0 => ApiResponse::ok(if pinned { "pinned" } else { "unpinned" }.to_string()),
        Ok(_) => ApiResponse::err("项目不存在".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 设置项目描述/目标（AG-03：AI 归属整理与未来项目 Chat 的上下文来源；空串 = 清除）
#[tauri::command]
pub fn project_set_description(
    app: AppHandle,
    id: String,
    description: String,
) -> ApiResponse<String> {
    let desc = description.trim().to_string();
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    let value: Option<&str> = if desc.is_empty() {
        None
    } else {
        Some(desc.as_str())
    };
    match conn.execute(
        "UPDATE projects SET description = ?1, updated_at = datetime('now') WHERE id = ?2",
        rusqlite::params![value, id],
    ) {
        Ok(n) if n > 0 => ApiResponse::ok("updated".to_string()),
        Ok(_) => ApiResponse::err("项目不存在".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// DEC-036：项目只组织本地工作区与会话，不再拥有 Article/Markdown 正文。
/// 移除项目只解除 SophoNote 内的关联并删除项目元数据；本地目录与正文均不参与。
fn delete_project_rows(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<()>, rusqlite::Error> {
    let project_exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        rusqlite::params![id],
        |row| row.get::<_, bool>(0),
    )?;
    if !project_exists {
        return Ok(None);
    }

    // 会话与运行记录继续保留，解除项目范围后可在全局会话中访问。
    conn.execute(
        "UPDATE agent_threads SET project_id = NULL WHERE project_id = ?1",
        rusqlite::params![id],
    )?;
    conn.execute(
        "UPDATE agent_runs SET project_id = NULL WHERE project_id = ?1",
        rusqlite::params![id],
    )?;

    // 历史 project_documents 只解除关系；正文、revision、operation、索引与 Markdown 全部保留。
    conn.execute(
        "DELETE FROM project_documents WHERE project_id = ?1",
        rusqlite::params![id],
    )?;
    conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])?;

    Ok(Some(()))
}

#[tauri::command]
pub fn project_delete(app: AppHandle, id: String) -> ApiResponse<()> {
    let mut conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };

    let transaction = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    // 项目元数据与 SophoNote 内关联在同一事务中解除；本地工作区和正文不参与该事务。
    match delete_project_rows(&transaction, &id) {
        Ok(Some(())) => {}
        Ok(None) => return ApiResponse::err("项目不存在".to_string()),
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    if let Err(e) = transaction.commit() {
        return ApiResponse::err(e.to_string());
    }
    ApiResponse::ok(())
}

/// 全量成员关系（前端派生归属 map；孤儿行由 articles JOIN/过滤天然兜底）
#[tauri::command]
pub fn project_list_memberships(app: AppHandle) -> ApiResponse<Vec<Membership>> {
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    let mut stmt =
        match conn.prepare("SELECT project_id, article_id, parent_id FROM project_documents") {
            Ok(s) => s,
            Err(e) => return ApiResponse::err(e.to_string()),
        };
    let mapped = stmt.query_map([], |r| {
        Ok(Membership {
            project_id: r.get(0)?,
            article_id: r.get(1)?,
            parent_id: r.get(2)?,
        })
    });
    match mapped {
        Ok(rows) => ApiResponse::ok(rows.filter_map(|r| r.ok()).collect()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 归属/移动文档到项目（单一归属：PK=article_id，INSERT OR REPLACE 天然实现 move）。
/// 这是未来智能体「整理文件夹归属」的写入原语。
#[tauri::command]
pub fn project_assign_document(
    app: AppHandle,
    project_id: String,
    article_id: String,
) -> ApiResponse<String> {
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    // 校验双方存在，避免孤儿归属
    let project_exists: bool = conn
        .query_row(
            "SELECT 1 FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !project_exists {
        return ApiResponse::err("项目不存在".to_string());
    }
    let article_exists: bool = conn
        .query_row(
            "SELECT 1 FROM articles WHERE id = ?1",
            rusqlite::params![article_id],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !article_exists {
        return ApiResponse::err("文档不存在".to_string());
    }
    match conn.execute(
        "INSERT OR REPLACE INTO project_documents (project_id, article_id, added_at)
         VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![project_id, article_id],
    ) {
        Ok(_) => ApiResponse::ok("assigned".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 解除文档归属（文档本身不删）
#[tauri::command]
pub fn project_remove_document(app: AppHandle, article_id: String) -> ApiResponse<String> {
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    match conn.execute(
        "DELETE FROM project_documents WHERE article_id = ?1",
        rusqlite::params![article_id],
    ) {
        Ok(_) => ApiResponse::ok("removed".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(conn: &rusqlite::Connection, sql: &str, id: &str) -> i64 {
        conn.query_row(sql, rusqlite::params![id], |row| row.get(0))
            .expect("count")
    }

    #[test]
    fn deleting_project_preserves_articles_and_only_removes_legacy_memberships() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        crate::db::create_schema(&conn).expect("create schema");
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', 'Project')",
            [],
        )
        .expect("insert project");
        conn.execute(
            "INSERT INTO articles (id, title, content) VALUES ('member', 'Member', '[[Linked]]')",
            [],
        )
        .expect("insert member");
        conn.execute(
            "INSERT INTO articles (id, title, content) VALUES ('linked', 'Linked', '[[Member]]')",
            [],
        )
        .expect("insert linked note");
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id) VALUES ('p1', 'member')",
            [],
        )
        .expect("insert membership");

        delete_project_rows(&conn, "p1")
            .expect("delete project")
            .expect("project exists");

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM projects WHERE id = ?1", "p1"),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM articles WHERE id = ?1",
                "member"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM articles WHERE id = ?1",
                "linked"
            ),
            1
        );
    }

    #[test]
    fn deleting_empty_project_removes_the_project() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        crate::db::create_schema(&conn).expect("create schema");
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('empty', 'Test22')",
            [],
        )
        .expect("insert empty project");

        delete_project_rows(&conn, "empty")
            .expect("delete empty project")
            .expect("project exists");

        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                "empty"
            ),
            0
        );
    }

    #[test]
    fn deleting_project_preserves_articles_revisions_and_operations() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        crate::db::create_schema(&conn).expect("create schema");
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', 'Cascade')",
            [],
        )
        .expect("insert project");
        for (id, title) in [("a1", "One"), ("a2", "Two")] {
            conn.execute(
                "INSERT INTO articles (id, title, content) VALUES (?1, ?2, 'body')",
                rusqlite::params![id, title],
            )
            .expect("insert article");
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id) VALUES ('p1', ?1)",
                rusqlite::params![id],
            )
            .expect("insert membership");
            conn.execute(
                "INSERT INTO document_revisions
                 (id, document_id, version, content_hash, content_snapshot, created_at)
                 VALUES (?1, ?2, 1, 'h', 'old', 1)",
                rusqlite::params![format!("rev-{id}"), id],
            )
            .expect("insert revision");
            conn.execute(
                "INSERT INTO document_operations
                 (id, idempotency_key, document_id, operation_type, base_version, status,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'patch', 0, 'committed', 1, 1)",
                rusqlite::params![format!("op-{id}"), format!("key-{id}"), id],
            )
            .expect("insert operation");
        }

        delete_project_rows(&conn, "p1")
            .expect("delete project")
            .expect("project exists");

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM projects WHERE id = ?1", "p1"),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1",
                "p1"
            ),
            0
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a1"),
            1
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a2"),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM document_revisions WHERE document_id = ?1",
                "a1"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM document_operations WHERE document_id = ?1",
                "a2"
            ),
            1
        );
    }

    #[test]
    fn deleting_missing_project_returns_none() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        crate::db::create_schema(&conn).expect("create schema");
        let deleted = delete_project_rows(&conn, "missing").expect("query ok");
        assert!(deleted.is_none());
    }

    #[test]
    fn removing_project_detaches_and_preserves_conversation_history() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        crate::db::create_schema(&conn).expect("create schema");
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('p1', 'Project')",
            [],
        )
        .expect("insert project");
        conn.execute(
            "INSERT INTO agent_threads
             (id, title, status, project_id, created_at, updated_at)
             VALUES ('thread-1', 'History', 'completed', 'p1', 1, 1)",
            [],
        )
        .expect("insert thread");
        conn.execute(
            "INSERT INTO agent_runs
             (id, thread_id, project_id, status, created_at, updated_at)
             VALUES ('run-1', 'thread-1', 'p1', 'completed', 1, 1)",
            [],
        )
        .expect("insert run");

        delete_project_rows(&conn, "p1")
            .expect("remove project")
            .expect("project exists");

        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM agent_threads WHERE id = ?1 AND project_id IS NULL",
                "thread-1"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM agent_runs WHERE id = ?1 AND project_id IS NULL",
                "run-1"
            ),
            1
        );
    }
}
