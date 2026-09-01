// 语义搜索：基于 sqlite-vec 的向量索引
//
// 设计要点：
// - vec_items 虚拟表在首次写入向量时按实际维度创建，维度记录在 settings.vec_dimension
// - 若新向量维度与已建表不一致，自动重建虚拟表（embedding 是派生数据，可重算）
// - 每次命令独立打开连接，auto_extension 在 lib.rs 启动时注册一次，对全部连接生效

use rusqlite::Connection;
use serde::Serialize;
use tauri::AppHandle;

use crate::commands::ApiResponse;
use crate::db::{get_db_path, Item};

/// 注册 sqlite-vec 为自动扩展（进程启动时调用一次，须在打开任何连接之前）
pub fn register_vec_extension() {
    unsafe {
        // AG-23：transmute 显式标注目标类型（Clippy missing_transmute_annotations）——
        // sqlite3_vec_init 签名：fn(*mut sqlite3, *mut *mut c_char, *const sqlite3_api_routines) -> i32
        // AG-24：i8 → std::ffi::c_char——c_char 在 aarch64（Linux/macOS）上是 u8、
        // x86_64 上是 i8，硬编码 i8 只能编过 x86_64；c_char 两平台同口径
        // 类型路径走 rusqlite::ffi（= libsqlite3_sys re-export，本文件已在用）；
        // rusqlite::libsqlite3_sys 不是公开路径（clippy 诊断路径≠可解析路径）
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const std::ffi::c_void,
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const std::ffi::c_void,
        )));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub item: Item,
    pub distance: f32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub indexed_count: i64,
    pub dimension: Option<i64>,
    pub total_items: i64,
}

fn open_conn(app: &AppHandle) -> Result<Connection, String> {
    Connection::open(get_db_path(app)).map_err(|e| e.to_string())
}

/// 确保 vec_items 虚拟表存在且维度与 `dim` 一致；不一致时重建
fn ensure_vec_table(conn: &Connection, dim: usize) -> Result<(), String> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'vec_dimension'",
            [],
            |row| row.get::<_, String>(0).map(|v| v.parse().unwrap_or(0)),
        )
        .ok();

    if let Some(d) = existing {
        if d as usize != dim {
            // 维度变化（切换了嵌入模型）→ 重建虚拟表，旧向量作废
            conn.execute_batch("DROP TABLE IF EXISTS vec_items;")
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM settings WHERE key = 'vec_dimension'", [])
                .map_err(|e| e.to_string())?;
        } else {
            return Ok(());
        }
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_items USING vec0(item_id TEXT PRIMARY KEY, embedding float[{}]);",
        dim
    ))
    .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('vec_dimension', ?1, datetime('now'))",
        rusqlite::params![dim.to_string()],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

fn vec_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vec_items'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// 把 f32 向量序列化为 sqlite-vec 可接受的 JSON 文本
fn vector_to_json(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

#[tauri::command]
pub fn vec_upsert_embedding(
    app: AppHandle,
    item_id: String,
    vector: Vec<f32>,
) -> ApiResponse<String> {
    if vector.is_empty() {
        return ApiResponse::err("embedding vector is empty".to_string());
    }
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if let Err(e) = ensure_vec_table(&conn, vector.len()) {
        return ApiResponse::err(e);
    }
    match conn.execute(
        "INSERT OR REPLACE INTO vec_items (item_id, embedding) VALUES (?1, ?2)",
        rusqlite::params![item_id, vector_to_json(&vector)],
    ) {
        Ok(_) => ApiResponse::ok("embedding saved".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn vec_search(
    app: AppHandle,
    vector: Vec<f32>,
    limit: Option<i64>,
) -> ApiResponse<Vec<SearchHit>> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if !vec_table_exists(&conn) {
        return ApiResponse::ok(vec![]);
    }

    let k = limit.unwrap_or(20).clamp(1, 100);
    let mut stmt = match conn.prepare(
        "SELECT item_id, distance FROM vec_items WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let hits: Vec<(String, f32)> = match stmt
        .query_map(rusqlite::params![vector_to_json(&vector), k], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
        }) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    // 按命中 id 批量取回条目（保持相似度顺序）
    let mut results = Vec::with_capacity(hits.len());
    for (id, distance) in hits {
        let item = conn.query_row(
            "SELECT id, source_id, item_type, title, url, description, author, language, stars, forks, topics, published_at, fetched_at, status, ai_summary, ai_tags
             FROM items WHERE id = ?1
             AND datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            rusqlite::params![id],
            |row| {
                Ok(Item {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    item_type: row.get(2)?,
                    title: row.get(3)?,
                    url: row.get(4)?,
                    description: row.get(5)?,
                    author: row.get(6)?,
                    language: row.get(7)?,
                    stars: row.get(8)?,
                    forks: row.get(9)?,
                    topics: row.get(10)?,
                    published_at: row.get(11)?,
                    fetched_at: row.get(12)?,
                    status: row.get(13)?,
                    ai_summary: row.get(14)?,
                    ai_tags: row.get(15)?,
                    content_status: None,
                    quality_level: None,
                })
            },
        );
        if let Ok(item) = item {
            results.push(SearchHit { item, distance });
        }
    }

    ApiResponse::ok(results)
}

#[tauri::command]
pub fn vec_index_stats(app: AppHandle) -> ApiResponse<IndexStats> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };

    let total_items: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM items
             WHERE datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let dimension: Option<i64> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'vec_dimension'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok());

    let indexed_count: i64 = if vec_table_exists(&conn) {
        conn.query_row("SELECT COUNT(*) FROM vec_items", [], |row| row.get(0))
            .unwrap_or(0)
    } else {
        0
    };

    ApiResponse::ok(IndexStats {
        indexed_count,
        dimension,
        total_items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_roundtrip() {
        register_vec_extension();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP);
             CREATE TABLE items (id TEXT PRIMARY KEY, source_id TEXT, item_type TEXT, title TEXT, url TEXT, description TEXT, author TEXT, language TEXT, stars INTEGER, forks INTEGER, topics TEXT, published_at TEXT, fetched_at TEXT, status TEXT, user_notes TEXT, ai_summary TEXT, ai_tags TEXT, embedding BLOB);
             INSERT INTO items (id, source_id, item_type, title, url, description, published_at, fetched_at, status) VALUES
               ('a', 's', 'repo', 'vector database', '', 'embedding search engine', '2026-01-01', '2026-01-01', 'unread'),
               ('b', 's', 'paper', 'cooking recipe', '', 'how to make pasta', '2026-01-01', '2026-01-01', 'unread');",
        )
        .unwrap();

        // 建表 + 写入
        ensure_vec_table(&conn, 3).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO vec_items (item_id, embedding) VALUES (?1, ?2)",
            rusqlite::params!["a", vector_to_json(&[1.0, 0.0, 0.0])],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO vec_items (item_id, embedding) VALUES (?1, ?2)",
            rusqlite::params!["b", vector_to_json(&[0.0, 1.0, 0.0])],
        )
        .unwrap();

        // MATCH + LIMIT 检索
        let mut stmt = conn
            .prepare("SELECT item_id, distance FROM vec_items WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2")
            .unwrap();
        let hits: Vec<(String, f32)> = stmt
            .query_map(
                rusqlite::params![vector_to_json(&[0.9, 0.1, 0.0]), 2i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?)),
            )
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "a", "最近邻应为 a");

        // 维度不一致时自动重建
        ensure_vec_table(&conn, 4).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "重建后索引应清空");

        // 重复调用同维度不重建
        ensure_vec_table(&conn, 4).unwrap();
        let dim: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'vec_dimension'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dim, "4");
    }
}

#[tauri::command]
pub fn vec_indexed_ids(app: AppHandle) -> ApiResponse<Vec<String>> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if !vec_table_exists(&conn) {
        return ApiResponse::ok(vec![]);
    }
    let mut stmt = match conn.prepare("SELECT item_id FROM vec_items") {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let ids = stmt.query_map([], |row| row.get::<_, String>(0));
    match ids {
        Ok(iter) => ApiResponse::ok(iter.filter_map(|r| r.ok()).collect()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn vec_delete_embedding(app: AppHandle, item_id: String) -> ApiResponse<String> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if !vec_table_exists(&conn) {
        return ApiResponse::ok("no index".to_string());
    }
    match conn.execute(
        "DELETE FROM vec_items WHERE item_id = ?1",
        rusqlite::params![item_id],
    ) {
        Ok(_) => ApiResponse::ok("embedding deleted".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

// ==================== chunk 级语义索引（借鉴 khoj chunk+embedding 管线） ====================
//
// 与条目级 vec_items 并存：正文/证据分片入 item_chunks + vec_chunks，
// 语义搜索可命中证据片段并溯源到条目，AI 解读可定位到具体片段。

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkInput {
    pub idx: i64,
    pub text: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkHit {
    pub item: Item,
    pub distance: f32,
    pub chunk_text: String,
    pub chunk_idx: i64,
}

fn vec_chunks_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vec_chunks'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// 确保 vec_chunks 虚拟表存在且维度一致；不一致时重建（派生数据可重算）
fn ensure_vec_chunks_table(conn: &Connection, dim: usize) -> Result<(), String> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'vec_chunk_dimension'",
            [],
            |row| row.get::<_, String>(0).map(|v| v.parse().unwrap_or(0)),
        )
        .ok();

    if let Some(d) = existing {
        if d as usize != dim {
            conn.execute_batch("DROP TABLE IF EXISTS vec_chunks;")
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM settings WHERE key = 'vec_chunk_dimension'", [])
                .map_err(|e| e.to_string())?;
        } else {
            return Ok(());
        }
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(chunk_id TEXT PRIMARY KEY, item_id TEXT, embedding float[{}]);",
        dim
    ))
    .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('vec_chunk_dimension', ?1, datetime('now'))",
        rusqlite::params![dim.to_string()],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// 全量替换某条目的 chunks（文本表 + 向量表），保证文本与向量不脱节
#[tauri::command]
pub fn vec_upsert_chunks(
    app: AppHandle,
    item_id: String,
    chunks: Vec<ChunkInput>,
) -> ApiResponse<String> {
    if chunks.is_empty() {
        return ApiResponse::err("chunks is empty".to_string());
    }
    if chunks
        .iter()
        .any(|c| c.vector.len() != chunks[0].vector.len())
    {
        return ApiResponse::err("chunk vector dimension mismatch".to_string());
    }
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if let Err(e) = ensure_vec_chunks_table(&conn, chunks[0].vector.len()) {
        return ApiResponse::err(e);
    }

    let mut clear = conn.execute(
        "DELETE FROM item_chunks WHERE item_id = ?1",
        rusqlite::params![item_id],
    );
    if clear.is_ok() {
        clear = conn.execute(
            "DELETE FROM vec_chunks WHERE item_id = ?1",
            rusqlite::params![item_id],
        );
    }
    if let Err(e) = clear {
        return ApiResponse::err(e.to_string());
    }

    for c in &chunks {
        let chunk_id = format!("{}#{}", item_id, c.idx);
        if let Err(e) = conn.execute(
            "INSERT INTO item_chunks (chunk_id, item_id, chunk_idx, text) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![chunk_id, item_id, c.idx, c.text],
        ) {
            return ApiResponse::err(e.to_string());
        }
        if let Err(e) = conn.execute(
            "INSERT INTO vec_chunks (chunk_id, item_id, embedding) VALUES (?1, ?2, ?3)",
            rusqlite::params![chunk_id, item_id, vector_to_json(&c.vector)],
        ) {
            return ApiResponse::err(e.to_string());
        }
    }

    ApiResponse::ok(format!("{} chunks saved", chunks.len()))
}

/// chunk 级语义搜索：命中证据片段并带回所属条目
#[tauri::command]
pub fn vec_search_chunks(
    app: AppHandle,
    vector: Vec<f32>,
    limit: Option<i64>,
) -> ApiResponse<Vec<ChunkHit>> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if !vec_chunks_table_exists(&conn) {
        return ApiResponse::ok(vec![]);
    }

    let k = limit.unwrap_or(20).clamp(1, 100);
    // 先 vec 后回表（CLAUDE.md：vec0 MATCH 带 JOIN 不能只靠 LIMIT）
    let mut stmt = match conn.prepare(
        "SELECT chunk_id, item_id, distance FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let hits: Vec<(String, String, f32)> =
        match stmt.query_map(rusqlite::params![vector_to_json(&vector), k], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f32>(2)?,
            ))
        }) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => return ApiResponse::err(e.to_string()),
        };

    let mut results = Vec::with_capacity(hits.len());
    for (chunk_id, item_id, distance) in hits {
        let (chunk_text, chunk_idx) = match conn.query_row(
            "SELECT text, chunk_idx FROM item_chunks WHERE chunk_id = ?1",
            rusqlite::params![chunk_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let item = conn.query_row(
            "SELECT id, source_id, item_type, title, url, description, author, language, stars, forks, topics, published_at, fetched_at, status, ai_summary, ai_tags
             FROM items WHERE id = ?1
             AND datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            rusqlite::params![item_id],
            |row| {
                Ok(Item {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    item_type: row.get(2)?,
                    title: row.get(3)?,
                    url: row.get(4)?,
                    description: row.get(5)?,
                    author: row.get(6)?,
                    language: row.get(7)?,
                    stars: row.get(8)?,
                    forks: row.get(9)?,
                    topics: row.get(10)?,
                    published_at: row.get(11)?,
                    fetched_at: row.get(12)?,
                    status: row.get(13)?,
                    ai_summary: row.get(14)?,
                    ai_tags: row.get(15)?,
                    content_status: None,
                    quality_level: None,
                })
            },
        );
        if let Ok(item) = item {
            results.push(ChunkHit {
                item,
                distance,
                chunk_text,
                chunk_idx,
            });
        }
    }

    ApiResponse::ok(results)
}

/// 已做 chunk 索引的条目 id（增量索引用）
#[tauri::command]
pub fn vec_chunk_indexed_ids(app: AppHandle) -> ApiResponse<Vec<String>> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if !vec_chunks_table_exists(&conn) {
        return ApiResponse::ok(vec![]);
    }
    let mut stmt = match conn.prepare("SELECT DISTINCT item_id FROM vec_chunks") {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let ids = stmt.query_map([], |row| row.get::<_, String>(0));
    match ids {
        Ok(iter) => ApiResponse::ok(iter.filter_map(|r| r.ok()).collect()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

// ==================== N3：笔记/文档 chunk 级语义索引 ====================
//
// 与条目 chunk（item_chunks / vec_chunks）平行：note_id = articles.id，
// 命中回表 articles 取标题与类型；删除文章时同步清理（见 delete_note_chunks_for）。

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteChunkHit {
    pub note_id: String,
    pub title: String,
    pub article_type: String,
    pub distance: f32,
    pub chunk_text: String,
    pub chunk_idx: i64,
}

fn vec_note_chunks_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vec_note_chunks'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

/// 确保 vec_note_chunks 虚拟表存在且维度一致；不一致时重建（派生数据可重算）
fn ensure_vec_note_chunks_table(conn: &Connection, dim: usize) -> Result<(), String> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'vec_note_chunk_dimension'",
            [],
            |row| row.get::<_, String>(0).map(|v| v.parse().unwrap_or(0)),
        )
        .ok();

    if let Some(d) = existing {
        if d as usize != dim {
            conn.execute_batch("DROP TABLE IF EXISTS vec_note_chunks;")
                .map_err(|e| e.to_string())?;
            conn.execute(
                "DELETE FROM settings WHERE key = 'vec_note_chunk_dimension'",
                [],
            )
            .map_err(|e| e.to_string())?;
        } else {
            return Ok(());
        }
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_note_chunks USING vec0(chunk_id TEXT PRIMARY KEY, note_id TEXT, embedding float[{}]);",
        dim
    ))
    .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('vec_note_chunk_dimension', ?1, datetime('now'))",
        rusqlite::params![dim.to_string()],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// 全量替换某篇笔记/文档的 chunks（文本表 + 向量表），保证文本与向量不脱节
#[tauri::command]
pub fn vec_upsert_note_chunks(
    app: AppHandle,
    note_id: String,
    chunks: Vec<ChunkInput>,
) -> ApiResponse<String> {
    if chunks.is_empty() {
        return ApiResponse::err("chunks is empty".to_string());
    }
    if chunks
        .iter()
        .any(|c| c.vector.len() != chunks[0].vector.len())
    {
        return ApiResponse::err("chunk vector dimension mismatch".to_string());
    }
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if let Err(e) = ensure_vec_note_chunks_table(&conn, chunks[0].vector.len()) {
        return ApiResponse::err(e);
    }

    if let Err(e) = conn.execute(
        "DELETE FROM note_chunks WHERE note_id = ?1",
        rusqlite::params![note_id],
    ) {
        return ApiResponse::err(e.to_string());
    }
    if let Err(e) = conn.execute(
        "DELETE FROM vec_note_chunks WHERE note_id = ?1",
        rusqlite::params![note_id],
    ) {
        return ApiResponse::err(e.to_string());
    }

    for c in &chunks {
        let chunk_id = format!("{}#{}", note_id, c.idx);
        if let Err(e) = conn.execute(
            "INSERT INTO note_chunks (chunk_id, note_id, chunk_idx, text) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![chunk_id, note_id, c.idx, c.text],
        ) {
            return ApiResponse::err(e.to_string());
        }
        if let Err(e) = conn.execute(
            "INSERT INTO vec_note_chunks (chunk_id, note_id, embedding) VALUES (?1, ?2, ?3)",
            rusqlite::params![chunk_id, note_id, vector_to_json(&c.vector)],
        ) {
            return ApiResponse::err(e.to_string());
        }
    }

    ApiResponse::ok(format!("{} note chunks saved", chunks.len()))
}

/// 删除某笔记的全部 chunk 索引（文章删除时同步调用；表不存在时尽力清理文本侧）
pub fn delete_note_chunks_for(conn: &Connection, note_id: &str) {
    let _ = conn.execute(
        "DELETE FROM note_chunks WHERE note_id = ?1",
        rusqlite::params![note_id],
    );
    if vec_note_chunks_table_exists(conn) {
        let _ = conn.execute(
            "DELETE FROM vec_note_chunks WHERE note_id = ?1",
            rusqlite::params![note_id],
        );
    }
}

/// 笔记 chunk 级语义搜索：命中片段并带回所属文档（标题/类型取自 articles）
#[tauri::command]
pub fn vec_search_note_chunks(
    app: AppHandle,
    vector: Vec<f32>,
    limit: Option<i64>,
) -> ApiResponse<Vec<NoteChunkHit>> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e),
    };
    if !vec_note_chunks_table_exists(&conn) {
        return ApiResponse::ok(vec![]);
    }

    let k = limit.unwrap_or(20).clamp(1, 100);
    // 先 vec 后回表（CLAUDE.md：vec0 MATCH 带 JOIN 不能只靠 LIMIT）
    let mut stmt = match conn.prepare(
        "SELECT chunk_id, note_id, distance FROM vec_note_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    ) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let hits: Vec<(String, String, f32)> =
        match stmt.query_map(rusqlite::params![vector_to_json(&vector), k], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f32>(2)?,
            ))
        }) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => return ApiResponse::err(e.to_string()),
        };

    let mut results = Vec::with_capacity(hits.len());
    for (chunk_id, note_id, distance) in hits {
        let (chunk_text, chunk_idx) = match conn.query_row(
            "SELECT text, chunk_idx FROM note_chunks WHERE chunk_id = ?1",
            rusqlite::params![chunk_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let meta = conn.query_row(
            "SELECT title, article_type FROM articles WHERE id = ?1",
            rusqlite::params![note_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        if let Ok((title, article_type)) = meta {
            results.push(NoteChunkHit {
                note_id,
                title,
                article_type,
                distance,
                chunk_text,
                chunk_idx,
            });
        }
    }

    ApiResponse::ok(results)
}

/// 已做 chunk 索引的笔记 id（增量索引用）
#[tauri::command]
pub fn vec_note_chunk_indexed_ids(app: AppHandle) -> ApiResponse<Vec<String>> {
    let conn = match open_conn(&app) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    if !vec_note_chunks_table_exists(&conn) {
        return ApiResponse::ok(vec![]);
    }
    let mut stmt = match conn.prepare("SELECT DISTINCT note_id FROM vec_note_chunks") {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let ids = stmt.query_map([], |row| row.get::<_, String>(0));
    match ids {
        Ok(iter) => ApiResponse::ok(iter.filter_map(|r| r.ok()).collect()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}
