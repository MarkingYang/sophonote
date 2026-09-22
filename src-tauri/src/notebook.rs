//! 笔记目录只拥有组织关系；正文仍由 notes / DocumentService 管理。
use crate::commands::ApiResponse;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::AppHandle;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookFolder {
    id: String,
    name: String,
    parent_id: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookEntry {
    article_id: String,
    folder_id: String,
}
#[derive(Debug, Serialize)]
pub struct NotebookTree {
    folders: Vec<NotebookFolder>,
    entries: Vec<NotebookEntry>,
}

pub fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS notebook_folders (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT REFERENCES notebook_folders(id)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS notebook_folder_sibling_name
            ON notebook_folders(COALESCE(parent_id, ''), name COLLATE NOCASE);
         CREATE TABLE IF NOT EXISTS notebook_entries (
            article_id TEXT PRIMARY KEY REFERENCES articles(id),
            folder_id TEXT NOT NULL REFERENCES notebook_folders(id)
         );
         CREATE INDEX IF NOT EXISTS notebook_entries_folder ON notebook_entries(folder_id);",
    )
    .map_err(|e| e.to_string())
}

pub fn validate_folder(conn: &Connection, id: Option<&str>) -> Result<(), String> {
    if let Some(id) = id {
        let found: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM notebook_folders WHERE id=?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !found {
            return Err("文件夹不存在，请刷新后重试".into());
        }
    }
    Ok(())
}

fn read_tree(conn: &Connection) -> Result<NotebookTree, String> {
    let mut stmt = conn
        .prepare("SELECT id,name,parent_id FROM notebook_folders ORDER BY name COLLATE NOCASE,id")
        .map_err(|e| e.to_string())?;
    let folders = stmt
        .query_map([], |row| {
            Ok(NotebookFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT e.article_id,e.folder_id FROM notebook_entries e JOIN articles a ON a.id=e.article_id WHERE a.article_type IN ('manual','journal')").map_err(|e| e.to_string())?;
    let entries = stmt
        .query_map([], |row| {
            Ok(NotebookEntry {
                article_id: row.get(0)?,
                folder_id: row.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(NotebookTree { folders, entries })
}

fn save_folder(
    conn: &mut Connection,
    id: Option<&str>,
    name: &str,
    parent_id: Option<&str>,
) -> Result<NotebookTree, String> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > 120
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || matches!(name, "." | "..")
    {
        return Err("文件夹名称须为 1–120 个字符，不能包含斜线或控制字符".into());
    }
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    validate_folder(&tx, parent_id)?;
    if let Some(id) = id {
        validate_folder(&tx, Some(id))?;
        let mut current = parent_id.map(str::to_owned);
        let mut visited = std::collections::HashSet::new();
        while let Some(parent) = current {
            if parent == id || !visited.insert(parent.clone()) {
                return Err("不能将文件夹移入自身或子文件夹".into());
            }
            current = tx
                .query_row(
                    "SELECT parent_id FROM notebook_folders WHERE id=?1",
                    [&parent],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    let duplicate: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM notebook_folders WHERE parent_id IS ?1 AND name=?2 COLLATE NOCASE AND (?3 IS NULL OR id<>?3))",
        params![parent_id, name, id], |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    if duplicate {
        return Err("当前目录已有同名文件夹".into());
    }
    match id {
        Some(id) => tx.execute(
            "UPDATE notebook_folders SET name=?2,parent_id=?3 WHERE id=?1",
            params![id, name, parent_id],
        ),
        None => tx.execute(
            "INSERT INTO notebook_folders(id,name,parent_id) VALUES(?1,?2,?3)",
            params![uuid::Uuid::new_v4().to_string(), name, parent_id],
        ),
    }
    .map_err(|e| e.to_string())?;
    let tree = read_tree(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(tree)
}

pub fn assign_note(
    conn: &Connection,
    article_id: &str,
    folder_id: Option<&str>,
) -> Result<(), String> {
    validate_folder(conn, folder_id)?;
    let kind: Option<String> = conn
        .query_row(
            "SELECT article_type FROM articles WHERE id=?1",
            [article_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if !matches!(kind.as_deref(), Some("manual" | "journal")) {
        return Err("只能移动笔记本中的笔记".into());
    }
    if let Some(folder_id) = folder_id {
        conn.execute("INSERT INTO notebook_entries(article_id,folder_id) VALUES(?1,?2) ON CONFLICT(article_id) DO UPDATE SET folder_id=excluded.folder_id", params![article_id,folder_id]).map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM notebook_entries WHERE article_id=?1",
            [article_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn delete_folder(conn: &mut Connection, id: &str) -> Result<NotebookTree, String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    validate_folder(&tx, Some(id))?;
    let used: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM notebook_folders WHERE parent_id=?1) OR EXISTS(SELECT 1 FROM notebook_entries WHERE folder_id=?1)", [id], |row| row.get(0)).map_err(|e| e.to_string())?;
    if used {
        return Err("文件夹不为空，请先移动其中的笔记和子文件夹".into());
    }
    tx.execute("DELETE FROM notebook_folders WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    let tree = read_tree(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(tree)
}

fn with_connection<T>(
    app: &AppHandle,
    run: impl FnOnce(&mut Connection) -> Result<T, String>,
) -> ApiResponse<T> {
    let result = Connection::open(crate::db::get_db_path(app))
        .map_err(|e| e.to_string())
        .and_then(|mut conn| run(&mut conn));
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}
#[tauri::command]
pub fn notebook_tree(app: AppHandle) -> ApiResponse<NotebookTree> {
    with_connection(&app, |conn| read_tree(conn))
}
#[tauri::command]
pub fn notebook_folder_save(
    app: AppHandle,
    id: Option<String>,
    name: String,
    parent_id: Option<String>,
) -> ApiResponse<NotebookTree> {
    with_connection(&app, |conn| {
        save_folder(conn, id.as_deref(), &name, parent_id.as_deref())
    })
}
#[tauri::command]
pub fn notebook_folder_delete(app: AppHandle, id: String) -> ApiResponse<NotebookTree> {
    with_connection(&app, |conn| delete_folder(conn, &id))
}
#[tauri::command]
pub fn notebook_note_move(
    app: AppHandle,
    article_id: String,
    folder_id: Option<String>,
) -> ApiResponse<NotebookTree> {
    with_connection(&app, |conn| {
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        assign_note(&tx, &article_id, folder_id.as_deref())?;
        let tree = read_tree(&tx)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(tree)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::create_schema(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; INSERT INTO articles(id,title,content,article_type,created_at) VALUES('note','笔记','','manual','2026-09-22'),('report','报告','','deep-dive','2026-09-22');").unwrap();
        conn
    }
    #[test]
    fn nested_folders_reject_duplicates_cycles_and_invalid_parents() {
        let mut conn = setup();
        let root = save_folder(&mut conn, None, "研究", None)
            .unwrap()
            .folders
            .remove(0)
            .id;
        assert!(save_folder(&mut conn, None, "研究", None).is_err());
        let tree = save_folder(&mut conn, None, "资料", Some(&root)).unwrap();
        let child = tree
            .folders
            .iter()
            .find(|f| f.parent_id.is_some())
            .unwrap()
            .id
            .clone();
        assert!(save_folder(&mut conn, Some(&root), "研究", Some(&child)).is_err());
        assert!(save_folder(&mut conn, None, "孤儿", Some("missing")).is_err());
        assert!(save_folder(&mut conn, None, "../危险", None).is_err());
        save_folder(&mut conn, Some(&child), "资料改名", None).unwrap();
        assert_eq!(read_tree(&conn).unwrap().folders.len(), 2);
    }
    #[test]
    fn moving_notes_and_deleting_folders_never_deletes_body_metadata() {
        let mut conn = setup();
        let id = save_folder(&mut conn, None, "资料", None)
            .unwrap()
            .folders
            .remove(0)
            .id;
        assign_note(&conn, "note", Some(&id)).unwrap();
        assert!(delete_folder(&mut conn, &id).is_err());
        assert!(assign_note(&conn, "report", Some(&id)).is_err());
        assert!(assign_note(&conn, "note", Some("missing")).is_err());
        assert_eq!(read_tree(&conn).unwrap().entries[0].folder_id, id);
        assign_note(&conn, "note", None).unwrap();
        delete_folder(&mut conn, &id).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn migration_is_idempotent_and_note_deletion_removes_membership() {
        let mut conn = setup();
        create_schema(&conn).unwrap();
        let id = save_folder(&mut conn, None, "资料", None)
            .unwrap()
            .folders
            .remove(0)
            .id;
        assign_note(&conn, "note", Some(&id)).unwrap();
        crate::db::delete_article_rows(&conn, "note").unwrap();
        assert!(read_tree(&conn).unwrap().entries.is_empty());
        delete_folder(&mut conn, &id).unwrap();
    }
}
