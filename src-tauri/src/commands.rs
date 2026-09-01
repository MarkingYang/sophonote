use serde::Serialize;
use tauri::AppHandle;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri_plugin_updater::UpdaterExt;

use crate::db::{
    get_db_path, init_db, insert_pomodoro_session, list_pomodoro_sessions, DailyLog, Item,
    PomodoroSession, Source, Task,
};

#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }
    pub fn err(msg: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg),
        }
    }
}

// Row mappers (named functions have concrete types, avoiding closure type mismatch)
fn map_item_row(row: &rusqlite::Row) -> Result<Item, rusqlite::Error> {
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
        content_status: row.get(16).ok(),
        quality_level: row.get(17).ok(),
    })
}

fn map_source_row(row: &rusqlite::Row) -> Result<Source, rusqlite::Error> {
    Ok(Source {
        id: row.get(0)?,
        name: row.get(1)?,
        source_type: row.get(2)?,
        enabled: row.get(3)?,
        config: row.get(4)?,
        fetch_interval_minutes: row.get(5)?,
        last_fetched_at: row.get(6)?,
        tier: row.get(7).unwrap_or_else(|_| "core".to_string()),
        admission: row.get(8).unwrap_or_else(|_| "active".to_string()),
        last_success_at: row.get(9).ok(),
        last_error: row.get(10).ok(),
        fetch_success_count: row.get(11).unwrap_or(0),
        fetch_fail_count: row.get(12).unwrap_or(0),
    })
}

const SOURCE_COLS: &str = "id, name, source_type, enabled, config, fetch_interval_minutes, last_fetched_at, tier, admission, last_success_at, last_error, fetch_success_count, fetch_fail_count";

fn map_log_row(row: &rusqlite::Row) -> Result<DailyLog, rusqlite::Error> {
    Ok(DailyLog {
        id: row.get(0)?,
        date: row.get(1)?,
        log_type: row.get(2)?,
        content: row.get(3)?,
        sources: row.get(4)?,
        generated_by: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn map_task_row(row: &rusqlite::Row) -> Result<Task, rusqlite::Error> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        status: row.get(3)?,
        priority: row.get(4)?,
        due_date: row.get(5)?,
        recurring: row.get(6)?,
        tags: row.get(7)?,
        created_at: row.get(8)?,
        completed_at: row.get(9)?,
    })
}

// ==================== 数据库命令 ====================

#[tauri::command]
pub fn db_init(app: AppHandle) -> ApiResponse<String> {
    match init_db(&app) {
        Ok(_) => ApiResponse::ok("Database initialized".to_string()),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // 查询命令的可选过滤参数，聚合结构反而增加调用方负担
pub fn db_get_items(
    app: AppHandle,
    source_id: Option<String>,
    item_type: Option<String>,
    status: Option<String>,
    limit: Option<i32>,
    include_probation: Option<bool>,
    query: Option<String>,
    offset: Option<i32>,
    exclude_archived: Option<bool>,
) -> ApiResponse<Vec<Item>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let mut sql = "SELECT i.id, i.source_id, i.item_type, i.title, i.url, i.description, i.author, i.language, i.stars, i.forks, i.topics, i.published_at, i.fetched_at, i.status, i.ai_summary, i.ai_tags, c.status, c.quality_level FROM items i LEFT JOIN item_contents c ON c.item_id = i.id WHERE datetime(COALESCE(i.expires_at, datetime(i.fetched_at, '+168 hours'))) > datetime('now')".to_string();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();

    // 试用观察期/跳过的信源：参与抓取但不进默认视图（借鉴 ai-news-radar 观察区）；
    // 显式按 source_id 查看或 include_probation=true 时才展示，便于评审试用源
    if !(include_probation.unwrap_or(false) || source_id.is_some()) {
        sql.push_str(" AND i.source_id NOT IN (SELECT id FROM sources WHERE admission IN ('probation','skipped'))");
    }

    if let Some(sid) = source_id {
        values.push(sid.into());
        sql.push_str(&format!(" AND i.source_id = ?{}", values.len()));
    }
    if let Some(t) = item_type {
        values.push(t.into());
        sql.push_str(&format!(" AND i.item_type = ?{}", values.len()));
    }
    if let Some(s) = status {
        values.push(s.into());
        sql.push_str(&format!(" AND i.status = ?{}", values.len()));
    } else if exclude_archived.unwrap_or(false) {
        sql.push_str(" AND i.status != 'archived'");
    }
    if let Some(keyword) = query
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        let escaped = keyword
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.push(format!("%{escaped}%").into());
        let idx = values.len();
        sql.push_str(&format!(
            " AND (i.title LIKE ?{idx} ESCAPE '\\' OR COALESCE(i.description, '') LIKE ?{idx} ESCAPE '\\' OR COALESCE(i.ai_summary, '') LIKE ?{idx} ESCAPE '\\' OR COALESCE(i.ai_tags, '') LIKE ?{idx} ESCAPE '\\')"
        ));
    }
    sql.push_str(" ORDER BY datetime(COALESCE(i.first_fetched_at, i.fetched_at)) DESC, i.id DESC");
    if let Some(l) = limit {
        values.push(i64::from(l.clamp(1, 500)).into());
        sql.push_str(&format!(" LIMIT ?{}", values.len()));
        if let Some(start) = offset.filter(|value| *value > 0) {
            values.push(i64::from(start).into());
            sql.push_str(&format!(" OFFSET ?{}", values.len()));
        }
    }

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let items = stmt.query_map(rusqlite::params_from_iter(values.iter()), map_item_row);

    match items {
        Ok(iter) => {
            let collected: Result<Vec<_>, _> = iter.collect();
            match collected {
                Ok(data) => ApiResponse::ok(data),
                Err(e) => ApiResponse::err(e.to_string()),
            }
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_insert_item(app: AppHandle, item: Item) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let mut conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    if let Err(e) = tx.execute(
        "INSERT INTO inbox_item_ttl (item_id, first_fetched_at, last_seen_at, expires_at)
         VALUES (?1, datetime('now'), datetime('now'), datetime('now', '+168 hours'))
         ON CONFLICT(item_id) DO UPDATE SET last_seen_at = datetime('now')",
        rusqlite::params![item.id],
    ) {
        return ApiResponse::err(e.to_string());
    }
    let active: bool = tx
        .query_row(
            "SELECT datetime(expires_at) > datetime('now') FROM inbox_item_ttl WHERE item_id = ?1",
            rusqlite::params![item.id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !active {
        return match tx.commit() {
            Ok(_) => ApiResponse::ok("Expired item ignored".to_string()),
            Err(e) => ApiResponse::err(e.to_string()),
        };
    }

    if let Err(e) = tx.execute(
        "INSERT INTO items (id, source_id, item_type, title, url, description, author, language, stars, forks, topics, published_at, fetched_at, first_fetched_at, last_seen_at, expires_at, status, ai_summary, ai_tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, datetime('now'),
                 (SELECT first_fetched_at FROM inbox_item_ttl WHERE item_id = ?1),
                 (SELECT last_seen_at FROM inbox_item_ttl WHERE item_id = ?1),
                 (SELECT expires_at FROM inbox_item_ttl WHERE item_id = ?1), ?13, ?14, ?15)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            url = excluded.url,
            description = excluded.description,
            author = excluded.author,
            language = excluded.language,
            stars = excluded.stars,
            forks = excluded.forks,
            topics = excluded.topics,
            published_at = excluded.published_at,
            fetched_at = datetime('now'),
            last_seen_at = datetime('now')",
        rusqlite::params![
            item.id, item.source_id, item.item_type, item.title, item.url,
            item.description, item.author, item.language, item.stars, item.forks,
            item.topics, item.published_at, item.status,
            item.ai_summary, item.ai_tags
        ],
    ) {
        return ApiResponse::err(e.to_string());
    }
    match tx.commit() {
        Ok(_) => ApiResponse::ok("Item saved".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_update_item_status(app: AppHandle, id: String, status: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "UPDATE items SET status = ?1 WHERE id = ?2",
        rusqlite::params![status, id],
    ) {
        Ok(_) => ApiResponse::ok("Status updated".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

// 删除条目：连带清理收藏夹引用与向量索引，保证文本与向量不脱节
#[tauri::command]
pub fn db_delete_item(app: AppHandle, id: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let mut conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let steps = [
        tx.execute(
            "DELETE FROM collection_items WHERE item_id = ?1",
            rusqlite::params![id],
        ),
        tx.execute(
            "DELETE FROM articles WHERE item_id = ?1",
            rusqlite::params![id],
        ),
        tx.execute(
            "DELETE FROM item_chunks WHERE item_id = ?1",
            rusqlite::params![id],
        ),
        tx.execute("DELETE FROM items WHERE id = ?1", rusqlite::params![id]),
    ];
    for r in steps {
        if let Err(e) = r {
            return ApiResponse::err(e.to_string());
        }
    }

    // vec_items 是 sqlite-vec 虚拟表，索引尚未建立时可能不存在
    let has_vec: bool = tx
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vec_items'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if has_vec {
        if let Err(e) = tx.execute(
            "DELETE FROM vec_items WHERE item_id = ?1",
            rusqlite::params![id],
        ) {
            return ApiResponse::err(e.to_string());
        }
    }

    // chunk 级向量索引（vec_chunks 虚拟表，仅在已建立时存在）
    let has_vec_chunks: bool = tx
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vec_chunks'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if has_vec_chunks {
        if let Err(e) = tx.execute(
            "DELETE FROM vec_chunks WHERE item_id = ?1",
            rusqlite::params![id],
        ) {
            return ApiResponse::err(e.to_string());
        }
    }

    match tx.commit() {
        Ok(_) => ApiResponse::ok("Item deleted".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_get_sources(app: AppHandle) -> ApiResponse<Vec<Source>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let mut stmt = match conn.prepare(&format!(
        "SELECT {} FROM sources ORDER BY created_at",
        SOURCE_COLS
    )) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let sources = stmt.query_map([], map_source_row);

    match sources {
        Ok(iter) => {
            let collected: Result<Vec<_>, _> = iter.collect();
            match collected {
                Ok(data) => ApiResponse::ok(data),
                Err(e) => ApiResponse::err(e.to_string()),
            }
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_toggle_source(app: AppHandle, id: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "UPDATE sources SET enabled = CASE WHEN enabled = 1 THEN 0 ELSE 1 END WHERE id = ?1",
        rusqlite::params![id],
    ) {
        Ok(_) => ApiResponse::ok("Source toggled".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_update_source_interval(app: AppHandle, id: String, minutes: i32) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "UPDATE sources SET fetch_interval_minutes = ?2 WHERE id = ?1",
        rusqlite::params![id, minutes],
    ) {
        Ok(_) => ApiResponse::ok("Interval updated".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_update_source_discovery_config(
    app: AppHandle,
    id: String,
    generation_prompt: String,
    scoring_rule: String,
    min_score: f64,
) -> ApiResponse<String> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return ApiResponse::err("数据源标识无效".into());
    }
    let generation_prompt = generation_prompt.trim();
    let scoring_rule = scoring_rule.trim();
    if generation_prompt.chars().count() > 4_000 || scoring_rule.chars().count() > 2_000 {
        return ApiResponse::err("生成 Prompt 最多 4000 字，评分规则最多 2000 字".into());
    }
    if !min_score.is_finite() || !(0.0..=10.0).contains(&min_score) {
        return ApiResponse::err("最低分必须在 0 到 10 之间".into());
    }

    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(conn) => conn,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let current = match conn.query_row(
        "SELECT config FROM sources WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return ApiResponse::err("数据源不存在".into())
        }
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let mut config = current
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    config.insert(
        "generationPrompt".into(),
        serde_json::Value::String(generation_prompt.to_string()),
    );
    config.insert(
        "scoringRule".into(),
        serde_json::Value::String(scoring_rule.to_string()),
    );
    let Some(score) = serde_json::Number::from_f64(min_score) else {
        return ApiResponse::err("最低分无效".into());
    };
    config.insert("minScore".into(), serde_json::Value::Number(score));
    let encoded = match serde_json::to_string(&config) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let updated = match conn.execute(
        "UPDATE sources SET config = ?2 WHERE id = ?1",
        rusqlite::params![id, encoded],
    ) {
        Ok(1) => true,
        Ok(_) => return ApiResponse::err("数据源不存在".into()),
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    drop(conn);
    if updated {
        if let Err(error) =
            crate::sophonote_mcp::http_server::sync_discovery_policy_reference(&app, id)
        {
            return ApiResponse::err(format!("规则已保存，但同步到 Hermes Skill 失败：{error}"));
        }
    }
    ApiResponse::ok("发现策略已保存并同步到 Hermes Skill".into())
}

#[tauri::command]
pub fn db_update_item_ai(
    app: AppHandle,
    id: String,
    summary: String,
    tags: String,
    prompt_version: Option<String>,
    enrich_json: Option<String>,
) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "UPDATE items SET ai_summary = ?2, ai_tags = ?3, ai_prompt_version = ?4, ai_enrich_json = ?5 WHERE id = ?1",
        rusqlite::params![id, summary, tags, prompt_version, enrich_json],
    ) {
        Ok(_) => ApiResponse::ok("Item AI updated".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 读取速览结构化结果（ai_enrich_json），阅读视图打开条目时按需拉取
#[tauri::command]
pub fn db_get_item_enrich(app: AppHandle, id: String) -> ApiResponse<Option<String>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    match conn.query_row(
        "SELECT ai_enrich_json FROM items WHERE id = ?1",
        rusqlite::params![id],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(v) => ApiResponse::ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => ApiResponse::ok(None),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 信源分层调整（core | standard | experimental）
#[tauri::command]
pub fn db_update_source_tier(app: AppHandle, id: String, tier: String) -> ApiResponse<String> {
    if !["core", "standard", "experimental"].contains(&tier.as_str()) {
        return ApiResponse::err(format!("invalid tier: {}", tier));
    }
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    match conn.execute(
        "UPDATE sources SET tier = ?2 WHERE id = ?1",
        rusqlite::params![id, tier],
    ) {
        Ok(_) => ApiResponse::ok("Tier updated".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 信源准入状态调整（active | probation | skipped）
#[tauri::command]
pub fn db_update_source_admission(
    app: AppHandle,
    id: String,
    admission: String,
) -> ApiResponse<String> {
    if !["active", "probation", "skipped"].contains(&admission.as_str()) {
        return ApiResponse::err(format!("invalid admission: {}", admission));
    }
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    match conn.execute(
        "UPDATE sources SET admission = ?2 WHERE id = ?1",
        rusqlite::params![id, admission],
    ) {
        Ok(_) => ApiResponse::ok("Admission updated".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

// ==================== 文章（深度解读）命令 ====================

fn map_article_row(row: &rusqlite::Row) -> rusqlite::Result<crate::db::Article> {
    Ok(crate::db::Article {
        id: row.get(0)?,
        item_id: row.get(1)?,
        title: row.get(2)?,
        content: row.get(3)?,
        article_type: row.get(4)?,
        edited: row.get::<_, i32>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        prompt_version: row.get(8)?,
        blocks_json: row.get(9)?,
    })
}

const ARTICLE_COLS: &str = "id, item_id, title, content, article_type, edited, created_at, updated_at, prompt_version, blocks_json";

#[tauri::command]
pub fn db_insert_article(app: AppHandle, article: crate::db::Article) -> ApiResponse<String> {
    // N0：先写 .md 文件（真相源），再落 DB 索引；DB content 字段不再承载正文
    match crate::notes::insert_article(&app, &article) {
        Ok(_) => ApiResponse::ok("Article saved".to_string()),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub fn db_update_article(
    app: AppHandle,
    id: String,
    content: String,
    blocks_json: Option<String>,
) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    // AG-24（审计批次 5 第 1 步）：编辑器直写改道 DocumentRepository——
    // 与 Agent 写入同源经过单一底层写路径（docs/architecture.md「文档底层仓储」）：
    // 单文档锁 → 写文件（真相源）→ 清 DB content + version 递增。
    // expected_version = None：编辑器路径无条件递增，保持低延迟心跳体验。
    // （notes 目录由 write_article_file_in 内部 create_dir_all 兜底）
    let notes_dir = crate::notes::notes_dir(&app);
    if let Err(e) = crate::documents::repository::write_body(&conn, &notes_dir, &id, &content, None)
    {
        return ApiResponse::err(format!("write note file failed: {e}"));
    }

    // blocks_json 传 None 时保留原快照（COALESCE 语义；write_body 不触碰该列）
    if let Some(bj) = blocks_json {
        if let Err(e) = conn.execute(
            "UPDATE articles SET blocks_json = ?2 WHERE id = ?1",
            rusqlite::params![id, bj],
        ) {
            return ApiResponse::err(e.to_string());
        }
    }
    ApiResponse::ok("Article updated".to_string())
}

#[tauri::command]
pub fn db_delete_article(app: AppHandle, id: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    // 库行删除（含先清 project_documents 归属）收敛在 db::delete_article_rows，
    // 外键顺序由单测保证（db.rs tests）；这里只负责文件/向量索引的尽力而为清理。
    match crate::db::delete_article_rows(&conn, &id) {
        Ok(_) => {
            crate::notes::delete_article_file(&app, &id); // N0：同步删文件（尽力而为）
            crate::vector::delete_note_chunks_for(&conn, &id); // N3：同步删语义索引（尽力而为）
            ApiResponse::ok("Article deleted".to_string())
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_delete_articles(app: AppHandle, ids: Vec<String>) -> ApiResponse<Vec<String>> {
    if ids.is_empty() {
        return ApiResponse::ok(Vec::new());
    }

    let db_path = get_db_path(&app);
    let mut conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let transaction = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let deleted = match crate::db::delete_articles_rows(&transaction, &ids) {
        Ok(deleted) => deleted,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    for id in &deleted {
        crate::vector::delete_note_chunks_for(&transaction, id);
    }
    if let Err(e) = transaction.commit() {
        return ApiResponse::err(e.to_string());
    }

    // 数据库批量提交成功后再清文件；不让文件 IO 破坏数据库的原子性。
    for id in &deleted {
        crate::notes::delete_article_file(&app, id);
    }
    ApiResponse::ok(deleted)
}

#[tauri::command]
pub fn db_rename_article(app: AppHandle, id: String, title: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "UPDATE articles SET title = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        rusqlite::params![id, title],
    ) {
        Ok(_) => {
            crate::notes::rename_article_file(&app, &id, &title); // N0：同步文件 frontmatter 标题
            ApiResponse::ok("Article renamed".to_string())
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_get_articles(app: AppHandle, limit: Option<i32>) -> ApiResponse<Vec<crate::db::Article>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let sql = format!(
        "SELECT {} FROM articles ORDER BY created_at DESC LIMIT ?1",
        ARTICLE_COLS
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let collected: Vec<crate::db::Article> = stmt
        .query_map(rusqlite::params![limit.unwrap_or(100)], map_article_row)
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    // N0：正文从 .md 文件回填（DB content 已清空）；文件缺失时保留 DB 值作迁移期兜底
    let mut collected = collected;
    for a in collected.iter_mut() {
        if let Some(body) = crate::notes::read_article_body(&app, &a.id) {
            a.content = body;
        }
    }

    ApiResponse::ok(collected)
}

#[tauri::command]
pub fn db_get_deep_dive_by_item(
    app: AppHandle,
    item_id: String,
) -> ApiResponse<Option<crate::db::Article>> {
    let item_id = item_id.trim();
    if item_id.is_empty() || item_id.len() > 512 {
        return ApiResponse::err("Invalid item id".to_string());
    }
    let conn = match rusqlite::Connection::open(get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    match crate::notes::load_latest_article_for_item(
        &conn,
        &crate::notes::notes_dir(&app),
        item_id,
        "deep-dive",
    ) {
        Ok(article) => ApiResponse::ok(article),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub fn db_insert_log(app: AppHandle, log: DailyLog) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "INSERT OR REPLACE INTO daily_logs (id, date, log_type, content, sources, generated_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
        rusqlite::params![log.id, log.date, log.log_type, log.content, log.sources, log.generated_by, log.created_at],
    ) {
        Ok(_) => ApiResponse::ok("Log saved".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_get_logs(app: AppHandle, log_type: Option<String>) -> ApiResponse<Vec<DailyLog>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let sql = if log_type.is_some() {
        "SELECT id, date, log_type, content, sources, generated_by, created_at FROM daily_logs WHERE log_type = ?1 ORDER BY date DESC"
    } else {
        "SELECT id, date, log_type, content, sources, generated_by, created_at FROM daily_logs ORDER BY date DESC"
    };

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let logs = if let Some(t) = log_type {
        stmt.query_map(rusqlite::params![t], map_log_row)
    } else {
        stmt.query_map([], map_log_row)
    };

    match logs {
        Ok(iter) => {
            let collected: Result<Vec<_>, _> = iter.collect();
            match collected {
                Ok(data) => ApiResponse::ok(data),
                Err(e) => ApiResponse::err(e.to_string()),
            }
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_get_tasks(app: AppHandle, status: Option<String>) -> ApiResponse<Vec<Task>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let sql = if status.is_some() {
        "SELECT id, title, description, status, priority, due_date, recurring, tags, created_at, completed_at FROM tasks WHERE status = ?1 ORDER BY created_at DESC"
    } else {
        "SELECT id, title, description, status, priority, due_date, recurring, tags, created_at, completed_at FROM tasks ORDER BY created_at DESC"
    };

    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let tasks = if let Some(s) = status {
        stmt.query_map(rusqlite::params![s], map_task_row)
    } else {
        stmt.query_map([], map_task_row)
    };

    match tasks {
        Ok(iter) => {
            let collected: Result<Vec<_>, _> = iter.collect();
            match collected {
                Ok(data) => ApiResponse::ok(data),
                Err(e) => ApiResponse::err(e.to_string()),
            }
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_insert_task(app: AppHandle, task: Task) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute(
        "INSERT OR REPLACE INTO tasks (id, title, description, status, priority, due_date, recurring, tags, created_at, completed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            task.id, task.title, task.description, task.status, task.priority,
            task.due_date, task.recurring, task.tags, task.created_at, task.completed_at
        ],
    ) {
        Ok(_) => ApiResponse::ok("Task saved".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_delete_task(app: AppHandle, id: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match conn.execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id]) {
        Ok(_) => ApiResponse::ok("Task deleted".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_insert_pomodoro_session(app: AppHandle, session: PomodoroSession) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match insert_pomodoro_session(&conn, &session) {
        Ok(_) => ApiResponse::ok("Pomodoro session saved".to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_list_pomodoro_sessions(
    app: AppHandle,
    since: Option<String>,
) -> ApiResponse<Vec<PomodoroSession>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    match list_pomodoro_sessions(&conn, since.as_deref()) {
        Ok(sessions) => ApiResponse::ok(sessions),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn db_get_stats(app: AppHandle) -> ApiResponse<serde_json::Value> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let total_items: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let unread_items: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE status = 'unread' AND datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let starred_items: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE status = 'starred' AND datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let total_tasks: i32 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .unwrap_or(0);
    let pending_tasks: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE status != 'done'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let total_logs: i32 = conn
        .query_row("SELECT COUNT(*) FROM daily_logs", [], |row| row.get(0))
        .unwrap_or(0);

    let stats = serde_json::json!({
        "total_items": total_items,
        "unread_items": unread_items,
        "starred_items": starred_items,
        "total_tasks": total_tasks,
        "pending_tasks": pending_tasks,
        "total_logs": total_logs,
    });

    ApiResponse::ok(stats)
}

// ==================== 设置命令 ====================

#[tauri::command]
pub async fn update_setting(app: AppHandle, key: String, value: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    // 写入在独立块内完成：rusqlite 连接与 params 都不是 Send，不能跨
    // 下方 restart_bundled_hermes 的 await 存活（tauri 异步命令要求 Send）。
    let write_result = {
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => return ApiResponse::err(e.to_string()),
        };
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now'))",
            rusqlite::params![key, value],
        )
    };

    match write_result {
        Ok(_) => {
            // MODEL-11③：ai_config 落库后把免鉴权本地实例同步进 Hermes
            // config.yaml。仅当同步内容真正变化才重启 Runtime——改云供应商
            // 名称/白名单等无关字段不打断进行中的会话；重启失败不吞保存结果，
            // 与凭据保存同一套「两阶段独立」口径。
            if key == "ai_config" {
                match crate::agent::hermes::bridge_mount::sync_local_providers(&app) {
                    Ok(true) => {
                        if let Err(error) = crate::restart_bundled_hermes(&app).await {
                            eprintln!(
                                "[hermes] ai_config synced local providers, but runtime restart failed: {error}"
                            );
                            return ApiResponse::ok(
                                "Setting saved:hermes_restart_failed".to_string(),
                            );
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        eprintln!("[hermes] sync local providers failed (non-fatal): {error}")
                    }
                }
            }
            ApiResponse::ok("Setting saved".to_string())
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn get_setting(app: AppHandle, key: String) -> ApiResponse<String> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let value: Result<String, rusqlite::Error> = conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        rusqlite::params![key],
        |row| row.get(0),
    );

    match value {
        Ok(v) => ApiResponse::ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            ApiResponse::err("Setting not found".to_string())
        }
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

// ==================== 系统命令 ====================

#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 手动刷新数据源（与定时调度同一抓取入口）；source_ids 为空时刷新全部启用源
#[tauri::command]
pub async fn fetch_sources_now(
    app: AppHandle,
    source_ids: Option<Vec<String>>,
) -> ApiResponse<Vec<crate::scheduler::SourceFetchResult>> {
    ApiResponse::ok(crate::scheduler::fetch_sources(&app, source_ids).await)
}

/// 获取条目正文内容（有缓存直接返回，否则按来源抓取后落库）
#[tauri::command]
pub async fn get_item_content(
    app: AppHandle,
    item_id: String,
) -> ApiResponse<Option<crate::db::ItemContent>> {
    match crate::content::get_or_fetch_item_content(&app, &item_id).await {
        Ok(c) => ApiResponse::ok(Some(c)),
        Err(e) => ApiResponse::err(e),
    }
}

/// 存量正文补抓（A2：按来源配额）：limit 为每个来源的配额（默认 10），五个来源均衡推进
#[tauri::command]
pub async fn backfill_item_contents(
    app: AppHandle,
    limit: Option<i64>,
) -> ApiResponse<serde_json::Value> {
    match crate::content::backfill_contents(&app, limit.unwrap_or(10)).await {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}
/// 内容抓取覆盖率统计（P0 验收口径）
#[tauri::command]
pub fn content_coverage_stats(app: AppHandle) -> ApiResponse<serde_json::Value> {
    match crate::content::coverage_stats(&app) {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

/// 故事级合并：重建 24h 故事分组（多源验证信号）
#[tauri::command]
pub fn rebuild_stories(app: AppHandle) -> ApiResponse<serde_json::Value> {
    match crate::content::rebuild_stories(&app) {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

/// 故事列表（含 item_ids / source_ids / signal_level）
#[tauri::command]
pub fn get_stories(app: AppHandle, limit: Option<i64>) -> ApiResponse<Vec<crate::content::Story>> {
    match crate::content::get_stories(&app, limit.unwrap_or(100)) {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

/// 只读正文缓存（不触发抓取）：chunk 级索引只用本地已就绪正文
#[tauri::command]
pub fn get_content_cached(
    app: AppHandle,
    item_id: String,
) -> ApiResponse<Option<crate::db::ItemContent>> {
    match crate::content::get_cached_content(&app, &item_id) {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}
/// A4 论文离线转正文（不联网），让 arXiv/HF Papers 覆盖率立即达标
#[tauri::command]
pub fn convert_papers_offline(app: AppHandle) -> ApiResponse<serde_json::Value> {
    match crate::content::convert_papers_offline(&app) {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub fn get_data_dir(app: AppHandle) -> ApiResponse<String> {
    match crate::storage_layout::StorageLayout::resolve(&app) {
        Ok(layout) => ApiResponse::ok(layout.root.to_string_lossy().to_string()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

#[tauri::command]
pub fn get_storage_layout(app: AppHandle) -> ApiResponse<crate::storage_layout::StorageLayoutInfo> {
    match crate::storage_layout::StorageLayout::resolve(&app).and_then(|layout| {
        layout.ensure()?;
        Ok(layout.info())
    }) {
        Ok(layout) => ApiResponse::ok(layout),
        Err(error) => ApiResponse::err(error),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub date: Option<String>,
}

/// 检查由 Release 配置声明并使用 Tauri 公钥验签的更新。Debug 未配置
/// endpoint 时会明确返回错误，不回退到任意下载源。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
pub async fn app_update_check(app: AppHandle) -> ApiResponse<AppUpdateInfo> {
    let current = app.package_info().version.to_string();
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => return ApiResponse::err(format!("更新器配置不可用: {error}")),
    };
    match updater.check().await {
        Ok(Some(update)) => ApiResponse::ok(AppUpdateInfo {
            available: true,
            current_version: update.current_version,
            version: Some(update.version),
            notes: update.body,
            date: update.date.map(|value| value.to_string()),
        }),
        Ok(None) => ApiResponse::ok(AppUpdateInfo {
            available: false,
            current_version: current,
            version: None,
            notes: None,
            date: None,
        }),
        Err(error) => ApiResponse::err(format!("检查更新失败: {error}")),
    }
}

/// 重新检查、下载并验签安装更新。仅在验证成功后触发 Tauri 重启。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
pub async fn app_update_install(app: AppHandle) -> ApiResponse<String> {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => return ApiResponse::err(format!("更新器配置不可用: {error}")),
    };
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return ApiResponse::err("当前已是最新版本".into()),
        Err(error) => return ApiResponse::err(format!("检查更新失败: {error}")),
    };
    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        return ApiResponse::err(format!("下载或验签安装更新失败: {error}"));
    }
    app.restart();
}

/// 读取当前运行与待启用的 Hermes Sidecar 版本。
#[tauri::command]
pub async fn hermes_sidecar_status(
    app: AppHandle,
) -> ApiResponse<crate::agent::hermes::sidecar_update::HermesSidecarStatus> {
    let bundled_root = match crate::agent::hermes::bundled_runtime::bundled_resource_root(&app) {
        Ok(root) => root,
        Err(error) => return ApiResponse::err(error),
    };
    match crate::agent::hermes::sidecar_update::status(&app, &bundled_root) {
        Ok(status) => ApiResponse::ok(status),
        Err(error) => ApiResponse::err(error),
    }
}

/// 拉取官方 stable Release 并构建应用私有 Runtime 槽。
/// 成功只登记 pending，当前会话不热替换。
#[tauri::command]
pub async fn hermes_sidecar_pull(
    app: AppHandle,
) -> ApiResponse<crate::agent::hermes::sidecar_update::HermesSidecarStatus> {
    let bundled_root = match crate::agent::hermes::bundled_runtime::bundled_resource_root(&app) {
        Ok(root) => root,
        Err(error) => return ApiResponse::err(error),
    };
    match crate::agent::hermes::sidecar_update::pull_latest(app, bundled_root).await {
        Ok(status) => ApiResponse::ok(status),
        Err(error) => ApiResponse::err(error),
    }
}

// ==================== 钥匙串（API Key 安全存储） ====================

const KEYCHAIN_SERVICE: &str = "com.fei.sophonote";

// 进程级 Key 缓存：每个进程每个 provider 只触发一次钥匙串授权弹窗。
// dev 模式二进制每次重编译签名都变，弹窗不可避免，缓存把影响压到每进程一次。
static KEY_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::OnceLock::new();

fn key_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    KEY_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 写入进程缓存（保存/迁移 Key 时同步，避免缓存了空串导致后续读不到）
pub fn set_cached_api_key(provider: &str, key: &str) {
    if let Ok(mut c) = key_cache().lock() {
        c.insert(provider.to_string(), key.to_string());
    }
}

/// 仅供旧版本安全迁移与包内 Runtime 的降级读取；不跨 IPC 返回。
/// 旧值在 Keychain 写入并回读成功前必须保留，避免用户凭据丢失。
pub(crate) fn get_legacy_api_key(app: &AppHandle, provider: &str) -> Result<String, String> {
    let settings_key = format!("apikey:{provider}");
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    Ok(conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![settings_key],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default())
}

#[cfg(debug_assertions)]
fn save_debug_api_key_fallback(
    app: &AppHandle,
    provider: &str,
    api_key: &str,
) -> Result<(), String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![format!("apikey:{provider}"), api_key],
    )
    .map_err(|error| format!("写入 Debug API Key 回退失败: {error}"))?;
    Ok(())
}

/// 读取 Key：进程缓存 → Keychain → 旧 SQLite settings 一次性迁移。
/// 迁移只有在“写 Keychain 成功且回读一致”后才删除明文，任何失败都保留旧值并明确报错。
pub fn get_cached_api_key(app: &AppHandle, provider: &str) -> Result<String, String> {
    if let Some(k) = key_cache().lock().map_err(|e| e.to_string())?.get(provider) {
        return Ok(k.clone());
    }

    // 未签名的 tauri dev 二进制可能无法取得既有 Keychain ACL。Debug 保存
    // 失败时会显式写入开发回退；优先读取它，避免每次调用重复触发授权。
    #[cfg(debug_assertions)]
    {
        let debug_key = get_legacy_api_key(app, provider)?;
        if !debug_key.is_empty() {
            set_cached_api_key(provider, &debug_key);
            return Ok(debug_key);
        }
    }

    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, provider)
        .map_err(|error| format!("初始化 Keychain 失败: {error}"))?;
    match entry.get_password() {
        Ok(key) if !key.is_empty() => {
            set_cached_api_key(provider, &key);
            return Ok(key);
        }
        Ok(_) | Err(keyring::Error::NoEntry) => {}
        Err(error) => return Err(format!("读取 Keychain 失败: {error}")),
    }

    // Release 中 OpenRouter 排名凭据始终 Keychain-only；Debug 开发回退已在上方读取。
    if provider == crate::openrouter_rankings::KEYCHAIN_PROVIDER {
        set_cached_api_key(provider, "");
        return Ok(String::new());
    }

    // 旧开发版本可能把其它 Provider Key 写入 settings；仅作为一次性迁移来源。
    let settings_key = format!("apikey:{provider}");
    let legacy_key = get_legacy_api_key(app, provider)?;
    if legacy_key.is_empty() {
        set_cached_api_key(provider, "");
        return Ok(String::new());
    }

    entry
        .set_password(&legacy_key)
        .map_err(|error| format!("迁移写入 Keychain 失败，旧明文已保留: {error}"))?;
    let verified = entry
        .get_password()
        .map_err(|error| format!("迁移回读 Keychain 失败，旧明文已保留: {error}"))?;
    if verified != legacy_key {
        return Err("迁移回读 Keychain 不一致，旧明文已保留".into());
    }
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM settings WHERE key = ?1",
        rusqlite::params![settings_key],
    )
    .map_err(|error| format!("Keychain 已写入，但删除旧明文失败: {error}"))?;
    set_cached_api_key(provider, &verified);
    eprintln!("[keychain] provider={provider} legacy credential migrated");
    Ok(verified)
}

#[tauri::command]
pub async fn keychain_save_api_key(
    app: AppHandle,
    provider: String,
    api_key: String,
) -> ApiResponse<String> {
    if api_key.is_empty() {
        return keychain_delete_api_key(app, provider).await;
    }
    let keychain_result = (|| -> Result<(), String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &provider)
            .map_err(|error| format!("初始化 Keychain 失败: {error}"))?;
        entry
            .set_password(&api_key)
            .map_err(|error| format!("写入 Keychain 失败: {error}"))?;
        let verified = entry
            .get_password()
            .map_err(|error| format!("回读 Keychain 失败: {error}"))?;
        if verified != api_key {
            return Err("Keychain 回读不一致".into());
        }
        let conn = rusqlite::Connection::open(get_db_path(&app)).map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![format!("apikey:{}", provider)],
        )
        .map_err(|error| format!("删除旧明文失败: {error}"))?;
        set_cached_api_key(&provider, &api_key);
        Ok(())
    })();
    let storage = match keychain_result {
        Ok(()) => "keychain",
        Err(error) => {
            #[cfg(debug_assertions)]
            {
                if let Err(fallback_error) = save_debug_api_key_fallback(&app, &provider, &api_key)
                {
                    return ApiResponse::err(format!("{error}; {fallback_error}"));
                }
                set_cached_api_key(&provider, &api_key);
                eprintln!(
                    "[keychain] provider={provider} unavailable in Debug; using local development fallback: {error}"
                );
                "debug_fallback"
            }
            #[cfg(not(debug_assertions))]
            {
                return ApiResponse::err(error);
            }
        }
    };

    // OpenRouter 排名凭据由 SophoNote Bridge 在每次刷新时直接从 Host 读取，
    // 不注入 Hermes Provider 环境；保存它不应中断正在运行的会话或计划任务。
    if provider == crate::openrouter_rankings::KEYCHAIN_PROVIDER {
        return ApiResponse::ok(format!("saved:{storage}"));
    }

    // 凭据保存与 Runtime 热重启是两个独立阶段。第二阶段失败不能让前端
    // 抹掉已配置标记，否则会出现“保存失败”与“未填写”同时显示的假象。
    match crate::restart_bundled_hermes(&app).await {
        Ok(()) => ApiResponse::ok(format!("saved:{storage}")),
        Err(error) => {
            eprintln!(
                "[hermes] provider={provider} credential saved via {storage}, but runtime restart failed: {error}"
            );
            ApiResponse::ok(format!("saved:{storage}:hermes_restart_failed"))
        }
    }
}

#[tauri::command]
pub fn keychain_get_api_key(app: AppHandle, provider: String) -> ApiResponse<String> {
    match get_cached_api_key(&app, &provider) {
        // Never return an existing secret across IPC into the WebView. The UI
        // only needs configured/not-configured; model calls read Keychain in Rust.
        Ok(key) if !key.is_empty() => ApiResponse::ok("configured".to_string()),
        Ok(_) => ApiResponse::err("not_found".to_string()),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn keychain_delete_api_key(app: AppHandle, provider: String) -> ApiResponse<String> {
    if let Ok(conn) = rusqlite::Connection::open(get_db_path(&app)) {
        let _ = conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            rusqlite::params![format!("apikey:{}", provider)],
        );
    }
    // 顺手清理钥匙串里的历史残留（不弹窗：删除不存在条目时静默处理）
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, &provider) {
        if let Err(error) = entry.delete_credential() {
            if !matches!(error, keyring::Error::NoEntry) {
                return ApiResponse::err(format!("删除 Keychain 失败: {error}"));
            }
        }
    }
    let _ = key_cache().lock().map(|mut c| c.remove(&provider));
    match crate::restart_bundled_hermes(&app).await {
        Ok(()) => ApiResponse::ok("deleted".to_string()),
        Err(error) => {
            eprintln!(
                "[hermes] provider={provider} credential deleted, but runtime restart failed: {error}"
            );
            ApiResponse::ok("deleted:hermes_restart_failed".to_string())
        }
    }
}

// ==================== 每日 Top5 推荐（发现页数据层） ====================

/// 发现页类别 → 来源 id 映射（HuggingFace 含模型榜与每日论文两个源）
fn category_sources(category: &str) -> Result<Vec<&'static str>, String> {
    Ok(match category {
        "github" => vec!["github-trending"],
        "arxiv" => vec!["arxiv-ai"],
        "hackernews" => vec!["hackernews"],
        "producthunt" => vec!["producthunt"],
        "huggingface" => vec!["huggingface-models", "huggingface-papers"],
        "aihot" => vec!["aihot"],
        _ => return Err(format!("unknown category: {}", category)),
    })
}

/// 前端 LLM 打分后回传的单条入选结果
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickInput {
    pub item_id: String,
    pub rank: i32,
    pub heat_score: Option<i32>,
    pub ai_score: Option<f64>,
    pub reason: Option<String>,
}

/// 历史入选记录（跨天去重用；heat_score 为入选时热度快照，判断「迭代再入选」）
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedRef {
    pub item_id: String,
    pub date: String,
    pub heat_score: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickCandidates {
    pub candidates: Vec<Item>,
    pub picked: Vec<PickedRef>,
}

/// 推荐候选：近 7 天该类别条目按热度排序（Top 40），附历史入选记录供前端去重。
/// 打分与筛选由前端 LLM 完成（多供应商配置在前端），Rust 只负责数据存取。
#[tauri::command]
pub fn db_get_pick_candidates(app: AppHandle, category: String) -> ApiResponse<PickCandidates> {
    let sources = match category_sources(&category) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e),
    };
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let in_clause = sources
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");

    let sql = format!(
        "SELECT i.id, i.source_id, i.item_type, i.title, i.url, i.description, i.author, i.language,
                i.stars, i.forks, i.topics, i.published_at, i.fetched_at, i.status, i.ai_summary, i.ai_tags,
                c.status, c.quality_level
         FROM items i LEFT JOIN item_contents c ON c.item_id = i.id
         WHERE i.source_id IN ({}) AND i.status != 'archived'
           AND datetime(COALESCE(i.expires_at, datetime(i.fetched_at, '+168 hours'))) > datetime('now')
         ORDER BY COALESCE(i.stars, 0) DESC, i.fetched_at DESC
         LIMIT 40",
        in_clause
    );
    let candidates: Vec<Item> = match conn.prepare(&sql) {
        Ok(mut stmt) => stmt
            .query_map([], map_item_row)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default(),
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    // 每条目最近一次入选记录（date 最大），供前端「热度增长 >20% 视为迭代」判断
    let picked: Vec<PickedRef> = match conn.prepare(
        "SELECT p.item_id, p.date, p.heat_score FROM daily_picks p
         WHERE p.category = ?1
           AND p.date = (SELECT MAX(p2.date) FROM daily_picks p2 WHERE p2.category = ?1 AND p2.item_id = p.item_id)",
    ) {
        Ok(mut stmt) => stmt
            .query_map(rusqlite::params![category], |r| {
                Ok(PickedRef {
                    item_id: r.get(0)?,
                    date: r.get(1)?,
                    heat_score: r.get(2)?,
                })
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default(),
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    ApiResponse::ok(PickCandidates { candidates, picked })
}

/// 保存某天某类别的 Top5 入选结果（整体替换：同日同类别先清后插）
#[tauri::command]
pub fn db_save_daily_picks(
    app: AppHandle,
    date: String,
    category: String,
    picks: Vec<PickInput>,
) -> ApiResponse<String> {
    if category_sources(&category).is_err() {
        return ApiResponse::err(format!("unknown category: {}", category));
    }
    let db_path = get_db_path(&app);
    let mut conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    if let Err(e) = tx.execute(
        "DELETE FROM daily_picks WHERE date = ?1 AND category = ?2",
        rusqlite::params![date, category],
    ) {
        return ApiResponse::err(e.to_string());
    }
    for p in &picks {
        if let Err(e) = tx.execute(
            "INSERT INTO daily_picks (id, date, category, item_id, rank, heat_score, ai_score, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'))",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                date,
                category,
                p.item_id,
                p.rank,
                p.heat_score,
                p.ai_score,
                p.reason,
            ],
        ) {
            return ApiResponse::err(e.to_string());
        }
    }
    match tx.commit() {
        Ok(_) => ApiResponse::ok(format!("{} picks saved", picks.len())),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 入选记录 + 关联条目（时间线展示：按日期倒序、排名升序）
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPick {
    pub id: String,
    pub date: String,
    pub category: String,
    pub rank: i32,
    pub heat_score: Option<i32>,
    pub ai_score: Option<f64>,
    pub reason: Option<String>,
    pub selection_lane: String,
    pub created_at: String,
    pub item: Item,
}

#[tauri::command]
pub fn db_get_daily_picks(
    app: AppHandle,
    category: Option<String>,
    limit: Option<i64>,
) -> ApiResponse<Vec<DailyPick>> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    let (sql, use_cat) = if category.is_some() {
        (
            "SELECT p.id, p.date, p.category, p.rank, p.heat_score, p.ai_score, p.reason, p.selection_lane, p.created_at,
                    i.id, i.source_id, i.item_type, i.title, i.url, i.description, i.author, i.language,
                    i.stars, i.forks, i.topics, i.published_at, i.fetched_at, i.status, i.ai_summary, i.ai_tags,
                    c.status, c.quality_level
             FROM daily_picks p
             JOIN items i ON i.id = p.item_id
             LEFT JOIN item_contents c ON c.item_id = i.id
             WHERE p.category = ?1 AND p.selection_lane IS NOT NULL
             ORDER BY p.date DESC, p.rank ASC
             LIMIT ?2"
                .to_string(),
            true,
        )
    } else {
        (
            "SELECT p.id, p.date, p.category, p.rank, p.heat_score, p.ai_score, p.reason, p.selection_lane, p.created_at,
                    i.id, i.source_id, i.item_type, i.title, i.url, i.description, i.author, i.language,
                    i.stars, i.forks, i.topics, i.published_at, i.fetched_at, i.status, i.ai_summary, i.ai_tags,
                    c.status, c.quality_level
             FROM daily_picks p
             JOIN items i ON i.id = p.item_id
             LEFT JOIN item_contents c ON c.item_id = i.id
             WHERE p.selection_lane IS NOT NULL
             ORDER BY p.date DESC, p.rank ASC
             LIMIT ?1"
                .to_string(),
            false,
        )
    };

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let limit = limit.unwrap_or(100);

    let rows: Result<Vec<DailyPick>, rusqlite::Error> = if use_cat {
        let cat = category.clone().unwrap_or_default();
        stmt.query_map(rusqlite::params![cat, limit], |r| {
            Ok(DailyPick {
                id: r.get(0)?,
                date: r.get(1)?,
                category: r.get(2)?,
                rank: r.get(3)?,
                heat_score: r.get(4)?,
                ai_score: r.get(5)?,
                reason: r.get(6)?,
                selection_lane: r.get(7)?,
                created_at: r.get(8)?,
                item: map_item_row_offset(r)?,
            })
        })
        .and_then(|rows| rows.collect())
    } else {
        stmt.query_map(rusqlite::params![limit], |r| {
            Ok(DailyPick {
                id: r.get(0)?,
                date: r.get(1)?,
                category: r.get(2)?,
                rank: r.get(3)?,
                heat_score: r.get(4)?,
                ai_score: r.get(5)?,
                reason: r.get(6)?,
                selection_lane: r.get(7)?,
                created_at: r.get(8)?,
                item: map_item_row_offset(r)?,
            })
        })
        .and_then(|rows| rows.collect())
    };

    match rows {
        Ok(data) => ApiResponse::ok(data),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// NEXT-048 发现五断面：只读 feed（精选/全部共用）。
/// 精选 = 前端传 minScore=8.5 + windowDays=7 + requireDeep（+ aspect chip）；全部同样 requireDeep + minScore=7 + 游标分页。
/// 打分语义归 Skill（sophonote-ai-radar），Rust 只做过滤与分页，见 src/discovery.rs。
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri 参数名即前端 invoke 合同，不能包成内部查询对象。
pub fn db_discovery_feed(
    app: AppHandle,
    aspect: Option<String>,
    source: Option<String>,
    topic: Option<String>,
    min_score: Option<f64>,
    window_days: Option<i64>,
    require_deep: Option<bool>,
    missing_deep: Option<bool>,
    cursor: Option<String>,
    limit: Option<i64>,
) -> ApiResponse<crate::discovery::DiscoveryFeedPage> {
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let query = crate::discovery::DiscoveryFeedQuery {
        aspect,
        source,
        topic,
        min_score,
        window_days,
        require_deep,
        missing_deep,
        from_date: None,
        to_date: None,
        cursor,
        limit,
    };
    match crate::discovery::query_discovery_feed(&conn, &query) {
        Ok(page) => ApiResponse::ok(page),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub fn db_discovery_topics_summary(
    app: AppHandle,
    min_score: Option<f64>,
    window_days: Option<i64>,
) -> ApiResponse<Vec<crate::discovery::DiscoveryTopicSummary>> {
    let conn = match rusqlite::Connection::open(get_db_path(&app)) {
        Ok(conn) => conn,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    match crate::discovery::query_topic_summary(&conn, min_score.unwrap_or(7.0), window_days) {
        Ok(rows) => ApiResponse::ok(rows),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub fn db_model_leaderboard(
    app: AppHandle,
    date: Option<String>,
) -> ApiResponse<crate::discovery::ModelLeaderboardSnapshot> {
    let conn = match rusqlite::Connection::open(get_db_path(&app)) {
        Ok(conn) => conn,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    match crate::discovery::query_model_leaderboard(&conn, date.as_deref()) {
        Ok(snapshot) => ApiResponse::ok(snapshot),
        Err(error) => ApiResponse::err(error),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryReportView {
    id: String,
    title: String,
    period: String,
    period_key: String,
    content: String,
    created_at: String,
    updated_at: Option<String>,
}

#[tauri::command]
pub fn db_discovery_reports(app: AppHandle) -> ApiResponse<Vec<DiscoveryReportView>> {
    let conn = match rusqlite::Connection::open(get_db_path(&app)) {
        Ok(conn) => conn,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, title, created_at, updated_at, prompt_version \
         FROM articles WHERE article_type = 'report' ORDER BY created_at DESC",
    ) {
        Ok(stmt) => stmt,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let indexed = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    let mut reports = Vec::new();
    for row in indexed {
        let (id, title, created_at, updated_at, marker) = match row {
            Ok(value) => value,
            Err(error) => return ApiResponse::err(error.to_string()),
        };
        let marker = marker.unwrap_or_default();
        let mut parts = marker.splitn(3, ':');
        let _prefix = parts.next();
        let period = parts.next().unwrap_or("daily").to_string();
        let period_key = parts.next().unwrap_or_default().to_string();
        reports.push(DiscoveryReportView {
            content: crate::notes::read_article_body(&app, &id).unwrap_or_default(),
            id,
            title,
            period,
            period_key,
            created_at,
            updated_at,
        });
    }
    ApiResponse::ok(reports)
}

/// Item 行映射（联表查询中 items 列从第 10 列开始：前 9 列是 daily_picks 字段）
fn map_item_row_offset(row: &rusqlite::Row) -> Result<Item, rusqlite::Error> {
    Ok(Item {
        id: row.get(9)?,
        source_id: row.get(10)?,
        item_type: row.get(11)?,
        title: row.get(12)?,
        url: row.get(13)?,
        description: row.get(14)?,
        author: row.get(15)?,
        language: row.get(16)?,
        stars: row.get(17)?,
        forks: row.get(18)?,
        topics: row.get(19)?,
        published_at: row.get(20)?,
        fetched_at: row.get(21)?,
        status: row.get(22)?,
        ai_summary: row.get(23)?,
        ai_tags: row.get(24)?,
        content_status: row.get(25).ok(),
        quality_level: row.get(26).ok(),
    })
}

// ==================== 嵌入生成（Rust 侧发起，规避 webview 跨域限制） ====================

/// 生成文本向量：配置读 settings.ai_config.embedding，Key 读钥匙串 "embedding"。
/// 前端 fetch 在 WKWebView 中受跨域限制会 Load failed，因此统一走 reqwest。
#[tauri::command]
pub async fn ai_generate_embedding(app: AppHandle, text: String) -> ApiResponse<Vec<f32>> {
    // 1. 读取嵌入配置
    let db_path = get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };
    let raw: String = match conn.query_row(
        "SELECT value FROM settings WHERE key = 'ai_config'",
        [],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(format!("读取 AI 配置失败: {}", e)),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(format!("AI 配置解析失败: {}", e)),
    };
    let cfg = &parsed["embedding"];
    let base_url = cfg["baseUrl"].as_str().unwrap_or("").to_string();
    let model = cfg["model"].as_str().unwrap_or("").to_string();
    let is_dashscope = cfg["protocol"].as_str() == Some("dashscope");
    if base_url.is_empty() || model.is_empty() {
        return ApiResponse::err(
            "未配置嵌入模型，请到 设置 → AI 配置 → 向量嵌入 填写接口地址和模型".to_string(),
        );
    }

    // 2. 读取 API Key（settings 存储 + 进程缓存，开发期免钥匙串授权）
    let api_key = match get_cached_api_key(&app, "embedding") {
        Ok(k) if !k.is_empty() => k,
        Ok(_) => {
            return ApiResponse::err(
                "未配置嵌入模型 API Key，请到 设置 → AI 配置 → 向量嵌入 填写".to_string(),
            )
        }
        Err(e) => return ApiResponse::err(format!("读取钥匙串失败: {}", e)),
    };

    // 3. 调用嵌入接口
    let input: String = text.chars().take(8000).collect();
    let url = if is_dashscope {
        base_url.clone()
    } else {
        format!("{}/embeddings", base_url.trim_end_matches('/'))
    };
    let body = if is_dashscope {
        serde_json::json!({ "model": model, "input": { "texts": [input] } })
    } else {
        serde_json::json!({ "model": model, "input": input })
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(e.to_string()),
    };

    // 传输层失败自动重试一次：实测多为瞬时 DNS/代理/睡眠唤醒抖动（端点本身在线）。
    // HTTP 层错误（401/429/...）不重试，直接走下方状态码分支。
    let send_once =
        |client: &reqwest::Client| client.post(&url).bearer_auth(&api_key).json(&body).send();
    let resp = match send_once(&client).await {
        Ok(r) => r,
        Err(first) => {
            println!(
                "[embedding] send failed (will retry once): {} url={}",
                first, url
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match send_once(&client).await {
                Ok(r) => r,
                Err(e) => {
                    // 带上底层原因（dns/connect/timeout 区分），前端与 dev.log 同源可见
                    let cause = std::error::Error::source(&e)
                        .map(|s| format!("（底层: {}）", s))
                        .unwrap_or_default();
                    println!("[embedding] retry failed: {}{} url={}", e, cause, url);
                    return ApiResponse::err(format!("Embedding API 请求失败: {}{}", e, cause));
                }
            }
        }
    };
    let status = resp.status();
    let data: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(format!("Embedding API 响应解析失败: {}", e)),
    };
    if !status.is_success() {
        let msg = data.to_string();
        return ApiResponse::err(format!(
            "Embedding API error: {} - {}",
            status.as_u16(),
            &msg[..msg.len().min(200)]
        ));
    }

    let arr = if is_dashscope {
        data["output"]["embeddings"][0]["embedding"].as_array()
    } else {
        data["data"][0]["embedding"].as_array()
    };
    match arr {
        Some(a) if !a.is_empty() => {
            let vec: Vec<f32> = a
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            ApiResponse::ok(vec)
        }
        _ => ApiResponse::err("Embedding API 返回格式异常".to_string()),
    }
}
