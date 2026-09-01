// ============================================================
// Track A · NB-19（用户指令例外备案：AG-11 项目内文档组织树落地）
// Notion 式文档树：文档即树节点（project_documents.parent_id 指向同项目
// 另一 article_id，NULL = 项目根），无独立文件夹实体——「组织树不一定是
// 文件系统」，任何文档都可承载子文档，同 Notion 页面嵌套模型。
// 本文件只加「项目内置父」写原语；项目容器本体 CRUD / 归属 move 仍在
// projects.rs（Track B），两文件零耦合。
// 语义约定：
//   - 跨项目归属 move 走 project_assign_document（INSERT OR REPLACE 天然
//     重置 parent_id = NULL → 落新项目根）；其原子文档留在原项目，前端
//     建树时 parent 不在本项目文档集即自动降根，无需 Rust 清理。
//   - 父文档被删除/移出 → 子文档 parent_id 悬空，同上自动降根。
// ============================================================
use tauri::AppHandle;

use crate::commands::ApiResponse;
use crate::db::get_db_path;

/// 设置项目文档的组织树父级。`expected_project_id` 由 Agent 工具传入时，
/// 同时作为范围闸门；普通 Tauri 命令传 `None`，仍以文档真实归属为准。
pub fn set_doc_parent_in_project(
    conn: &rusqlite::Connection,
    expected_project_id: Option<&str>,
    article_id: &str,
    parent_id: Option<&str>,
) -> Result<String, String> {
    let project_id: String = conn
        .query_row(
            "SELECT project_id FROM project_documents WHERE article_id = ?1",
            rusqlite::params![article_id],
            |row| row.get(0),
        )
        .map_err(|_| "文档未归属任何项目".to_string())?;
    if expected_project_id.is_some_and(|expected| expected != project_id) {
        return Err("文档不属于当前项目".to_string());
    }

    if let Some(parent_id) = parent_id {
        if parent_id == article_id {
            return Err("不能移动到自身之下".to_string());
        }
        let parent_project: Option<String> = conn
            .query_row(
                "SELECT project_id FROM project_documents WHERE article_id = ?1",
                rusqlite::params![parent_id],
                |row| row.get(0),
            )
            .ok();
        if parent_project.as_deref() != Some(project_id.as_str()) {
            return Err("目标父级不是本项目的文档".to_string());
        }

        let mut current = Some(parent_id.to_string());
        let mut hops = 0;
        while let Some(candidate) = current {
            if candidate == article_id {
                return Err("移动会形成循环（目标文档是当前文档的子文档）".to_string());
            }
            hops += 1;
            if hops > 512 {
                return Err("文档树深度异常，已中止移动".to_string());
            }
            current = conn
                .query_row(
                    "SELECT parent_id FROM project_documents WHERE article_id = ?1",
                    rusqlite::params![candidate],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
        }
    }

    let changed = conn
        .execute(
            "UPDATE project_documents SET parent_id = ?1 WHERE article_id = ?2 AND project_id = ?3",
            rusqlite::params![parent_id, article_id, project_id],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("项目文档父级更新失败".to_string());
    }
    Ok(project_id)
}

fn open(app: &AppHandle) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())
}

/// 项目内置父（组织树移动原语）。parent_id = None → 回到项目根。
/// 校验：文档须已归属项目；父须为同项目文档；不得置自身为父；
/// 沿父祖先上溯防环（目标文档不得是被移动文档的后代）。
#[tauri::command]
pub fn project_set_doc_parent(
    app: AppHandle,
    article_id: String,
    parent_id: Option<String>,
) -> ApiResponse<String> {
    let conn = match open(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    match set_doc_parent_in_project(&conn, None, &article_id, parent_id.as_deref()) {
        Ok(_) => ApiResponse::ok("moved".to_string()),
        Err(error) => ApiResponse::err(error),
    }
}
