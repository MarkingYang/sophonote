// 条目内容层（P0-3）：按来源抓取正文证据，与轻量元数据 items 分离存储。
//
// 各来源策略：
// - github-trending：README（≤18000 字符）+ 最新 Release + 仓库元数据（License/语言/更新）
// - hackernews：外链文章正文（HTML 清洗，过短/paywall → partial，PDF/视频 → unsupported）+ 高质量评论
// - arxiv / huggingface-papers：完整摘要 + 作者 + 分类（P0 不解析 PDF）
// - huggingface-models：Model Card + 元数据（pipeline/downloads/likes/license/base model）
// - producthunt：GraphQL 详情页（简介/话题/制作者/官网 + 精选评论），无 token 时 unsupported

use serde::Serialize;
use tauri::AppHandle;

use crate::db::{get_db_path, Item, ItemContent};

const MAX_DOC_CHARS: usize = 18000; // README / Model Card 上限
const MAX_ARTICLE_CHARS: usize = 12000; // HN 外链正文上限
const MAX_COMMENT_CHARS: usize = 4000;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    id: String,
    kind: String,
    title: String,
    url: String,
    text: String,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 稳定的正文哈希（FNV-1a），用于去重与变更检测
fn content_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn make_content(
    item: &Item,
    status: &str,
    evidences: Vec<Evidence>,
    content_type: &str,
    quality_level: i32,
    error: Option<String>,
) -> ItemContent {
    let content_text = evidences
        .iter()
        .map(|e| format!("[{} {}]\n{}", e.id, e.title, e.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    let excerpt = {
        let first = evidences.first().map(|e| e.text.as_str()).unwrap_or("");
        let cleaned: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(cleaned.chars().take(300).collect())
    };
    ItemContent {
        item_id: item.id.clone(),
        status: status.to_string(),
        content_text: if content_text.is_empty() {
            None
        } else {
            Some(content_text.clone())
        },
        excerpt,
        evidence_json: serde_json::to_string(&evidences).ok(),
        content_type: Some(content_type.to_string()),
        quality_level,
        content_hash: if content_text.is_empty() {
            None
        } else {
            Some(content_hash(&content_text))
        },
        fetched_at: Some(now_rfc3339()),
        error_message: error,
    }
}

fn unsupported(item: &Item, reason: &str) -> ItemContent {
    make_content(
        item,
        "unsupported",
        vec![],
        "none",
        1,
        Some(reason.to_string()),
    )
}

// ==================== 读取/写入 ====================

fn load_content(app: &AppHandle, item_id: &str) -> Result<Option<ItemContent>, String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    let row = conn.query_row(
        "SELECT item_id, status, content_text, excerpt, evidence_json, content_type, quality_level, content_hash, fetched_at, error_message FROM item_contents WHERE item_id = ?1",
        rusqlite::params![item_id],
        |row| {
            Ok(ItemContent {
                item_id: row.get(0)?,
                status: row.get(1)?,
                content_text: row.get(2)?,
                excerpt: row.get(3)?,
                evidence_json: row.get(4)?,
                content_type: row.get(5)?,
                quality_level: row.get(6)?,
                content_hash: row.get(7)?,
                fetched_at: row.get(8)?,
                error_message: row.get(9)?,
            })
        },
    );
    match row {
        Ok(c) => Ok(Some(c)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn save_content(app: &AppHandle, c: &ItemContent) -> Result<(), String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO item_contents (item_id, status, content_text, excerpt, evidence_json, content_type, quality_level, content_hash, fetched_at, error_message)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            c.item_id, c.status, c.content_text, c.excerpt, c.evidence_json,
            c.content_type, c.quality_level, c.content_hash, c.fetched_at, c.error_message
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn load_item(app: &AppHandle, item_id: &str) -> Result<Item, String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, source_id, item_type, title, url, description, author, language, stars, forks, topics, published_at, fetched_at, status, ai_summary, ai_tags FROM items WHERE id = ?1",
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
    )
    .map_err(|e| format!("条目不存在: {}", e))
}

/// 统一入口：缓存命中直接返回，否则抓取后落库。
/// 缓存策略：ready/partial/unsupported 24h 内不重抓；failed 至少间隔 1 小时重试
pub async fn get_or_fetch_item_content(
    app: &AppHandle,
    item_id: &str,
) -> Result<ItemContent, String> {
    if let Some(existing) = load_content(app, item_id)? {
        let age_hours = existing
            .fetched_at
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| {
                chrono::Utc::now()
                    .signed_duration_since(t.with_timezone(&chrono::Utc))
                    .num_hours()
            });
        let fresh = match existing.status.as_str() {
            "ready" | "partial" | "unsupported" => age_hours.map(|h| h < 24).unwrap_or(false),
            "failed" => age_hours.map(|h| h < 1).unwrap_or(false),
            _ => false,
        };
        if fresh {
            return Ok(existing);
        }
    }

    let item = load_item(app, item_id)?;
    let content = match fetch_item_content(app, &item).await {
        Ok(c) => c,
        Err(e) => make_content(&item, "failed", vec![], "none", 1, Some(e)),
    };
    save_content(app, &content)?;

    // 目录级 → 内容级：无描述条目用正文 excerpt 回填列表摘要（HN 92% 条目受益）
    if item.description.trim().is_empty() {
        if let Some(excerpt) = &content.excerpt {
            if !excerpt.trim().is_empty() {
                if let Ok(conn) = rusqlite::Connection::open(get_db_path(app)) {
                    let _ = conn.execute(
                        "UPDATE items SET description = ?1 WHERE id = ?2 AND (description IS NULL OR description = '')",
                        rusqlite::params![excerpt, item_id],
                    );
                }
            }
        }
    }
    Ok(content)
}

/// 存量正文补抓（A2：按来源配额 + A3：优先级排序）。
/// 优先级（PRD A3）：今日候选（daily_picks）> 用户打开过/收藏（read/starred）> 关注主题（行为派生）> 普通存量（fetched_at DESC）。
/// 今日候选不受每源配额挤占、先行全选，保验收口径「今日 Top 正文覆盖 100%」；
/// 其余按来源各补抓剩余配额。旧的「全局按时间取前 N」会让最新来源（GitHub）长期独占队列、饿死 HN/arXiv/HF。
/// 500ms 节流防限流。
pub async fn backfill_contents(
    app: &AppHandle,
    per_source: i64,
) -> Result<serde_json::Value, String> {
    const SUPPORTED_SOURCES: [&str; 6] = [
        "github-trending",
        "hackernews",
        "arxiv-ai",
        "huggingface-papers",
        "huggingface-models",
        "producthunt",
    ];
    // daily_picks.date 由前端按本地时区 YYYY-MM-DD 写入，这里对齐 Local
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (ids, today_picks): (Vec<String>, usize) = {
        let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;

        // A3 第三级「关注主题」：B1「我的关注」配置页未建，过渡替代 = 打开/收藏条目的高频 topics 前 10；
        // B1 落地后把取词来源换成关注配置即可，CASE 排序结构不变。
        let mut interest_topics: Vec<String> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT topics FROM items WHERE status IN ('read','starred') AND topics IS NOT NULL AND topics != ''",
        ) {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for t in rows.flatten() {
                    for part in t.split(',') {
                        let p = part.trim().to_lowercase();
                        if !p.is_empty() {
                            *freq.entry(p).or_default() += 1;
                        }
                    }
                }
                let mut v: Vec<(String, usize)> = freq.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                interest_topics = v.into_iter().take(10).map(|(t, _)| t).collect();
            }
        }
        let interest_cond = if interest_topics.is_empty() {
            "0".to_string() // 无行为数据时该层永不命中，直接落回普通存量
        } else {
            interest_topics
                .iter()
                .map(|t| format!("LOWER(i.topics) LIKE '%{}%'", t.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(" OR ")
        };

        // A3 第一级：今日候选（daily_picks）先行，不受每源配额挤占
        let mut ids: Vec<String> = Vec::new();
        let mut picked_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut picked_by_source: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT p.item_id, i.source_id FROM daily_picks p
             JOIN items i ON i.id = p.item_id
             LEFT JOIN item_contents c ON c.item_id = p.item_id
             WHERE p.date = ?1 AND (c.item_id IS NULL OR c.status = 'failed')",
        ) {
            if let Ok(rows) = stmt.query_map(rusqlite::params![today], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) {
                for (id, src) in rows.flatten() {
                    if picked_set.insert(id.clone()) {
                        *picked_by_source.entry(src).or_default() += 1;
                        ids.push(id);
                    }
                }
            }
        }
        let today_picks = ids.len();

        // A2 配额 + A3 第二~四级：每源按「打开/收藏 > 关注主题 > 普通存量」补齐剩余配额
        for src in &SUPPORTED_SOURCES {
            let remaining = per_source - picked_by_source.get(*src).copied().unwrap_or(0);
            if remaining <= 0 {
                continue;
            }
            let sql = format!(
                "SELECT i.id FROM items i LEFT JOIN item_contents c ON c.item_id = i.id
                 WHERE (c.item_id IS NULL OR c.status = 'failed')
                   AND i.source_id = ?1
                 ORDER BY CASE
                     WHEN i.status IN ('read','starred') THEN 0
                     WHEN {interest_cond} THEN 1
                     ELSE 2
                 END, i.fetched_at DESC
                 LIMIT ?2"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![src, remaining], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for id in rows.flatten() {
                if picked_set.insert(id.clone()) {
                    ids.push(id);
                }
            }
        }
        (ids, today_picks)
    };

    let total = ids.len();
    let (mut ready, mut partial, mut failed, mut unsupported) = (0i64, 0i64, 0i64, 0i64);
    for id in &ids {
        match get_or_fetch_item_content(app, id).await {
            Ok(c) => match c.status.as_str() {
                "ready" => ready += 1,
                "partial" => partial += 1,
                "unsupported" => unsupported += 1,
                _ => failed += 1,
            },
            Err(e) => {
                eprintln!("[content] backfill failed for {}: {}", id, e);
                failed += 1;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Ok(serde_json::json!({
        "processed": total,
        "ready": ready,
        "partial": partial,
        "failed": failed,
        "unsupported": unsupported,
        "today_picks": today_picks,
    }))
}

/// 抓取覆盖率统计（P0 验收口径：GitHub README≥90% / HN≥70% / HF≥90%）
pub fn coverage_stats(app: &AppHandle) -> Result<serde_json::Value, String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;

    let total_items: i64 = conn
        .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
        .unwrap_or(0);
    let with_content: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM item_contents WHERE status IN ('ready','partial')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut by_status = serde_json::Map::new();
    if let Ok(mut stmt) = conn.prepare("SELECT status, COUNT(*) FROM item_contents GROUP BY status")
    {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        {
            for row in rows.flatten() {
                by_status.insert(row.0, serde_json::json!(row.1));
            }
        }
    }

    // 分来源覆盖率（A1 正确口径）：分母 = 该来源在 items 表的全部受支持条目，
    // 分子 = 其中已有正文（ready/partial）的条目。
    // 旧口径只统计 item_contents 已有记录的行，会把「从未尝试抓取」的条目排除在分母外、虚高成功率。
    let stats_for = |source_id: &str| -> (Option<i64>, serde_json::Value) {
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM items WHERE source_id = ?1",
                rusqlite::params![source_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if total == 0 {
            return (None, serde_json::Value::Null);
        }
        let ok: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM items i JOIN item_contents c ON c.item_id = i.id
                 WHERE i.source_id = ?1 AND c.status IN ('ready','partial')",
                rusqlite::params![source_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (
            Some(ok * 100 / total),
            serde_json::json!({ "ok": ok, "total": total, "rate": ok * 100 / total }),
        )
    };
    let (gh_rate, gh_src) = stats_for("github-trending");
    let (hn_rate, hn_src) = stats_for("hackernews");
    let (hf_rate, hf_src) = stats_for("huggingface-models");

    Ok(serde_json::json!({
        "total_items": total_items,
        "with_content": with_content,
        "coverage": if total_items > 0 { with_content * 100 / total_items } else { 0 },
        "by_status": by_status,
        "rates": {
            "github_readme": gh_rate,
            "hn_article": hn_rate,
            "hf_model_card": hf_rate,
        },
        "per_source": {
            "github_readme": gh_src,
            "hn_article": hn_src,
            "hf_model_card": hf_src,
        },
        "targets": { "github_readme": 90, "hn_article": 70, "hf_model_card": 90 },
        "health": source_health(&conn),
    }))
}

/// 源健康三件套（借鉴 ai-news-radar source-status）：成功率 / 最后成功时间 / 24h 产量，
/// 另带 tier 与 admission 供设置页健康度面板展示
fn source_health(conn: &rusqlite::Connection) -> serde_json::Value {
    let mut health = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT s.id, s.name, s.tier, s.admission, s.fetch_success_count, s.fetch_fail_count,
                s.last_success_at, s.last_error,
                (SELECT COUNT(*) FROM items i WHERE i.source_id = s.id AND datetime(i.fetched_at) >= datetime('now','-1 day')),
                (SELECT COUNT(*) FROM items i WHERE i.source_id = s.id)
         FROM sources s ORDER BY s.created_at",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| {
            let succ: i64 = r.get(4)?;
            let fail: i64 = r.get(5)?;
            let tot = succ + fail;
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "tier": r.get::<_, String>(2)?,
                "admission": r.get::<_, String>(3)?,
                "successCount": succ,
                "failCount": fail,
                "successRate": if tot > 0 { succ * 100 / tot } else { 100 },
                "lastSuccessAt": r.get::<_, Option<String>>(6)?,
                "lastError": r.get::<_, Option<String>>(7)?,
                "yield24h": r.get::<_, i64>(8)?,
                "itemsTotal": r.get::<_, i64>(9)?,
            }))
        }) {
            health = rows.filter_map(|r| r.ok()).collect();
        }
    }
    serde_json::Value::Array(health)
}

// ==================== 故事级合并（借鉴 ai-news-radar stories-merged） ====================

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Story {
    pub id: String,
    pub title: String,
    pub item_ids: Vec<String>,
    pub source_ids: Vec<String>,
    pub source_count: i64,
    pub signal_level: String,
    pub updated_at: Option<String>,
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 24h 窗口内按归一化标题聚类成 story；source_count >= 2 标 multi（多源验证）。
/// 标题过短（归一化后 < 8 字符）不参与聚类，避免误合并。
pub fn rebuild_stories(app: &AppHandle) -> Result<serde_json::Value, String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;

    let rows: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, source_id, title FROM items
                 WHERE datetime(fetched_at) >= datetime('now', '-1 day')
                    OR datetime(published_at) >= datetime('now', '-1 day')",
            )
            .map_err(|e| e.to_string())?;
        let collected = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    let norm = |t: &str| {
        t.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    };

    let mut groups: std::collections::BTreeMap<String, (String, Vec<String>, Vec<String>)> =
        std::collections::BTreeMap::new();
    for (id, src, title) in &rows {
        let key = norm(title);
        if key.len() < 8 {
            continue;
        }
        let entry = groups
            .entry(key)
            .or_insert_with(|| (title.clone(), Vec::new(), Vec::new()));
        entry.1.push(id.clone());
        if !entry.2.contains(src) {
            entry.2.push(src.clone());
        }
    }

    let (mut multi, mut total) = (0i64, 0i64);
    for (key, (title, item_ids, source_ids)) in &groups {
        total += 1;
        let is_multi = source_ids.len() >= 2;
        if is_multi {
            multi += 1;
        }
        let signal = if is_multi { "multi" } else { "single" };
        let story_id = format!("story-{:016x}", fnv1a(key.as_bytes()));
        let item_ids_json = serde_json::to_string(item_ids).unwrap_or_else(|_| "[]".to_string());
        let source_ids_json =
            serde_json::to_string(source_ids).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT INTO stories (id, title, item_ids, source_ids, source_count, signal_level, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                item_ids = excluded.item_ids,
                source_ids = excluded.source_ids,
                source_count = excluded.source_count,
                signal_level = excluded.signal_level,
                updated_at = datetime('now')",
            rusqlite::params![story_id, title, item_ids_json, source_ids_json, source_ids.len() as i64, signal],
        )
        .map_err(|e| e.to_string())?;
    }

    // 清理 7 天前的故事（派生数据，可重建）
    let _ = conn.execute(
        "DELETE FROM stories WHERE updated_at < datetime('now', '-7 day')",
        [],
    );

    Ok(serde_json::json!({ "stories": total, "multiSource": multi }))
}

pub fn get_stories(app: &AppHandle, limit: i64) -> Result<Vec<Story>, String> {
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, item_ids, source_ids, source_count, signal_level, updated_at
             FROM stories ORDER BY updated_at DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![limit], |r| {
            let item_ids: Vec<String> =
                serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default();
            let source_ids: Vec<String> =
                serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default();
            Ok(Story {
                id: r.get(0)?,
                title: r.get(1)?,
                item_ids,
                source_ids,
                source_count: r.get(4)?,
                signal_level: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 只读返回已有正文缓存（不触发抓取）：chunk 级索引只用本地已就绪的正文
pub fn get_cached_content(app: &AppHandle, item_id: &str) -> Result<Option<ItemContent>, String> {
    load_content(app, item_id)
}

async fn fetch_item_content(app: &AppHandle, item: &Item) -> Result<ItemContent, String> {
    match item.source_id.as_str() {
        "github-trending" => fetch_github_content(app, item).await,
        "hackernews" => fetch_hn_content(item).await,
        "arxiv-ai" | "huggingface-papers" => fetch_paper_content(app, item).await,
        "huggingface-models" => fetch_model_content(item).await,
        "producthunt" => fetch_producthunt_content(app, item).await,
        _ => Ok(unsupported(item, "该来源暂不支持正文获取")),
    }
}

// ==================== GitHub ====================

fn github_client() -> reqwest::Client {
    reqwest::Client::new()
}

/// GitHub 请求头：配置了 token（settings "github"）则带上，匿名时依赖 60/hr 限额
fn github_auth(app: &AppHandle) -> Option<String> {
    crate::commands::get_cached_api_key(app, "github")
        .ok()
        .filter(|t| !t.is_empty())
}

async fn fetch_github_content(app: &AppHandle, item: &Item) -> Result<ItemContent, String> {
    let client = github_client();
    let repo = &item.title;
    let token = github_auth(app);
    let mut evidences = Vec::new();
    let mut errors = Vec::new();

    // E1 README
    let mut req = client
        .get(format!("https://api.github.com/repos/{}/readme", repo))
        .header("User-Agent", "SophoNote-App")
        .header("Accept", "application/vnd.github.raw")
        .timeout(std::time::Duration::from_secs(20));
    if let Some(t) = &token {
        req = req.bearer_auth(t);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => {
            let text = r.text().await.map_err(|e| e.to_string())?;
            let truncated: String = text.chars().take(MAX_DOC_CHARS).collect();
            evidences.push(Evidence {
                id: "E1".to_string(),
                kind: "readme".to_string(),
                title: "README".to_string(),
                url: item.url.clone(),
                text: truncated,
            });
        }
        Ok(r) => errors.push(format!("README 获取失败（HTTP {}）", r.status())),
        Err(e) => errors.push(format!("README 请求错误（{}）", e)),
    }

    // E2 最新 Release
    let mut req2 = client
        .get(format!(
            "https://api.github.com/repos/{}/releases/latest",
            repo
        ))
        .header("User-Agent", "SophoNote-App")
        .timeout(std::time::Duration::from_secs(15));
    if let Some(t) = &token {
        req2 = req2.bearer_auth(t);
    }
    if let Ok(r) = req2.send().await {
        if r.status().is_success() {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                let tag = v["tag_name"].as_str().unwrap_or("");
                let published = v["published_at"].as_str().unwrap_or("");
                let body: String = v["body"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(2000)
                    .collect();
                if !tag.is_empty() {
                    evidences.push(Evidence {
                        id: "E2".to_string(),
                        kind: "release".to_string(),
                        title: format!("Latest release {}", tag),
                        url: format!("https://github.com/{}/releases", repo),
                        text: format!("版本 {} · {}\n{}", tag, published, body),
                    });
                }
            }
        }
    }

    // E3 仓库元数据
    let mut req3 = client
        .get(format!("https://api.github.com/repos/{}", repo))
        .header("User-Agent", "SophoNote-App")
        .timeout(std::time::Duration::from_secs(15));
    if let Some(t) = &token {
        req3 = req3.bearer_auth(t);
    }
    if let Ok(r) = req3.send().await {
        if r.status().is_success() {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                let license = v["license"]["spdx_id"].as_str().unwrap_or("未披露");
                let lang = v["language"].as_str().unwrap_or("未披露");
                let stars = v["stargazers_count"].as_i64().unwrap_or(0);
                let forks = v["forks_count"].as_i64().unwrap_or(0);
                let pushed = v["pushed_at"].as_str().unwrap_or("未知");
                let open_issues = v["open_issues_count"].as_i64().unwrap_or(0);
                evidences.push(Evidence {
                    id: "E3".to_string(),
                    kind: "metadata".to_string(),
                    title: "Repository metadata".to_string(),
                    url: item.url.clone(),
                    text: format!(
                        "Stars {} · Forks {} · 主要语言 {} · License {} · 最近更新 {} · Open issues {}",
                        stars, forks, lang, license, pushed, open_issues
                    ),
                });
            }
        }
    }

    if evidences.is_empty() {
        return Err(format!("GitHub 内容获取全部失败: {}", errors.join("；")));
    }
    let has_readme = evidences.iter().any(|e| e.kind == "readme");
    let quality = if evidences.len() >= 3 { 3 } else { 2 };
    Ok(make_content(
        item,
        if has_readme { "ready" } else { "partial" },
        evidences,
        "readme",
        quality,
        if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
    ))
}

// ==================== HackerNews ====================

fn strip_html(html: &str) -> String {
    use std::sync::OnceLock;
    // AG-23（审计 P1-3 性能项）：剥离模式静态化。strip_html 按条目调用，
    // 原实现每次调用都重新编译 8+2 个 Regex；模式为常量，静态缓存行为等价。
    // 注意只用 get_or_init（宿主 rustc 旧，禁 get_or_try_init）
    static STRIP_BLOCK_RES: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    static ARTICLE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static TAG_RE: OnceLock<regex::Regex> = OnceLock::new();

    let block_res = STRIP_BLOCK_RES.get_or_init(|| {
        [
            "script", "style", "nav", "footer", "header", "aside", "form", "iframe",
        ]
        .iter()
        .map(|tag| {
            regex::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>")).expect("static regex")
        })
        .collect()
    });

    // 先移除结构性噪声块：Rust regex crate 不支持反向引用 \1，按标签逐个配对移除
    let mut cleaned = html.to_string();
    for re in block_res {
        cleaned = re.replace_all(&cleaned, " ").into_owned();
    }
    // 优先 article/main 区域（同样不使用反向引用）
    let body = {
        let art_re = ARTICLE_RE.get_or_init(|| {
            regex::Regex::new(r"(?is)<(article|main)\b[^>]*>([\s\S]*?)</(?:article|main)\s*>")
                .expect("static regex")
        });
        art_re
            .captures(&cleaned)
            .and_then(|c| c.get(2).map(|m| m.as_str().to_string()))
            .unwrap_or_else(|| cleaned.to_string())
    };
    let tag_re = TAG_RE.get_or_init(|| regex::Regex::new(r"<[^>]+>").expect("static regex"));
    let text = tag_re.replace_all(&body, " ");
    text.replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// HN 外链安全校验：禁止 localhost / 内网 IP / 本地域（SSRF 防护，host 级）
fn is_safe_external_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host == "::1"
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return false;
    }
    // 内网 IP 段
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        if o[0] == 10
            || o[0] == 127
            || (o[0] == 192 && o[1] == 168)
            || (o[0] == 172 && (16..=31).contains(&o[1]))
            || (o[0] == 169 && o[1] == 254)
            || (o[0] == 0)
        {
            return false;
        }
    }
    true
}

/// 读取响应体（上限 5MB）
async fn read_body_capped(res: reqwest::Response, max_bytes: usize) -> Result<String, String> {
    use futures_util::StreamExt;
    let mut stream = res.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if buf.len() + chunk.len() > max_bytes {
            buf.extend_from_slice(&chunk[..max_bytes - buf.len()]);
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// HN 顶层高质量评论获取：先走 Algolia API（一次请求返回整棵评论树），
/// 不可达或无结果时回退官方 Firebase API（scheduler 列表抓取走该端点、可达性已验证），
/// 避免单一端点故障让 HN 评论证据全军覆没。
/// Ok(comments) 表示可达但可能无足长评论；Err 表示两条路径都网络失败。
async fn fetch_hn_comments(client: &reqwest::Client, hn_id: &str) -> Result<Vec<String>, String> {
    // 路径 1：Algolia
    if let Ok(r) = client
        .get(format!("https://hn.algolia.com/api/v1/items/{}", hn_id))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        if r.status().is_success() {
            if let Ok(data) = r.json::<serde_json::Value>().await {
                let comments: Vec<String> = data["children"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| c["text"].as_str().map(strip_html))
                            .filter(|t| t.len() > 50)
                            .take(5)
                            .collect()
                    })
                    .unwrap_or_default();
                if !comments.is_empty() {
                    return Ok(comments);
                }
            }
        }
    }
    // 路径 2：Firebase 官方 API（逐条取 story.kids 前列，跳过 deleted/dead 与过短评论）
    let story: serde_json::Value = client
        .get(format!(
            "https://hacker-news.firebaseio.com/v0/item/{}.json",
            hn_id
        ))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let kid_ids: Vec<i64> = story["kids"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|k| k.as_i64()).take(8).collect())
        .unwrap_or_default();
    let mut comments: Vec<String> = Vec::new();
    for kid in kid_ids {
        if comments.len() >= 5 {
            break;
        }
        let Ok(r) = client
            .get(format!(
                "https://hacker-news.firebaseio.com/v0/item/{}.json",
                kid
            ))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        else {
            continue;
        };
        let Ok(c) = r.json::<serde_json::Value>().await else {
            continue;
        };
        if c.get("deleted").is_some() || c.get("dead").is_some() {
            continue;
        }
        if let Some(text) = c["text"].as_str().map(strip_html) {
            if text.len() > 50 {
                comments.push(text);
            }
        }
    }
    Ok(comments)
}

async fn fetch_hn_content(item: &Item) -> Result<ItemContent, String> {
    // 安全：重定向 ≤3、单请求 20s、仅 HTTP/HTTPS、禁内网
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(3))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let hn_id = item.id.strip_prefix("hn-").unwrap_or(&item.id);
    let mut evidences = Vec::new();
    let mut errors = Vec::new();
    let mut article_state = "missing"; // ok | thin | self | unsupported | missing

    // E1 外链文章正文
    let is_hn_self = item.url.contains("news.ycombinator.com");
    if !is_hn_self && is_safe_external_url(&item.url) {
        match client
            .get(&item.url)
            // 标准浏览器 UA + Accept：不少站点对非浏览器客户端直接 403/反爬拦截，
            // 这是 HN 外链正文抓取失败率高的主因之一（A5 达标关键改动）
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            )
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
        {
            Ok(r) => {
                let ctype = r
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_lowercase();
                // 响应体预检：声明超过 5MB 直接拒绝
                let too_big = r
                    .headers()
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<usize>().ok())
                    .map(|n| n > 5 * 1024 * 1024)
                    .unwrap_or(false);
                if r.status().is_success() && ctype.contains("text/html") && !too_big {
                    let html = read_body_capped(r, 5 * 1024 * 1024).await?;
                    let text: String = strip_html(&html).chars().take(MAX_ARTICLE_CHARS).collect();
                    if text.len() >= 400 {
                        article_state = "ok";
                        evidences.push(Evidence {
                            id: "E1".to_string(),
                            kind: "article".to_string(),
                            title: "Article".to_string(),
                            url: item.url.clone(),
                            text,
                        });
                    } else {
                        article_state = "thin";
                        errors.push("外链正文过短（可能登录墙/反爬），标记 partial".to_string());
                    }
                } else if !r.status().is_success() {
                    // 403/404 等：视为瞬时或反爬失败，保持 missing → 记 partial 留重试空间，不标 unsupported
                    errors.push(format!("外链返回 {}，正文未获取到", r.status()));
                } else if too_big {
                    article_state = "unsupported";
                    errors.push("外链响应体超过 5MB，标记 unsupported".to_string());
                } else {
                    // 规格：PDF、视频等非 HTML 内容 → 先标记 unsupported
                    article_state = "unsupported";
                    errors.push(format!("外链为 PDF/视频等非 HTML 内容（{}），无法获取正文，标记 unsupported", ctype));
                }
            }
            Err(e) => errors.push(format!("外链抓取失败: {}", e)),
        }
    } else if !is_hn_self {
        errors.push("外链地址不安全（localhost/内网/非 HTTP），已拦截".to_string());
    } else {
        article_state = "self";
        // Ask HN / Tell HN 自帖：正文即 description，直接用作 E1（此前完全丢弃、只靠评论）
        let self_text: String = item
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if self_text.chars().count() >= 200 {
            evidences.push(Evidence {
                id: "E1".to_string(),
                kind: "article".to_string(),
                title: "Self post body".to_string(),
                url: item.url.clone(),
                text: self_text.chars().take(MAX_ARTICLE_CHARS).collect(),
            });
        }
    }

    // E2 HN 高质量评论（Algolia 优先，Firebase 回退）
    match fetch_hn_comments(&client, hn_id).await {
        Ok(comments) if !comments.is_empty() => {
            let joined: String = comments
                .join("\n---\n")
                .chars()
                .take(MAX_COMMENT_CHARS)
                .collect();
            evidences.push(Evidence {
                id: "E2".to_string(),
                kind: "discussion".to_string(),
                title: "HN discussion".to_string(),
                url: format!("https://news.ycombinator.com/item?id={}", hn_id),
                text: joined,
            });
        }
        Ok(_) => {
            if article_state != "ok" {
                errors.push("无足长高质量评论".to_string());
            }
        }
        Err(e) => errors.push(format!("HN 评论获取失败: {}", e)),
    }

    if evidences.is_empty() {
        let msg = if errors.is_empty() {
            "无正文与评论证据".to_string()
        } else {
            errors.join("；")
        };
        if article_state == "unsupported" {
            return Ok(make_content(
                item,
                "unsupported",
                vec![],
                "article",
                1,
                Some(msg),
            ));
        }
        // 不返回 Err：自帖无评论等确定性失败若记 failed 会被无限重试
        return Ok(make_content(
            item,
            "partial",
            vec![],
            "article",
            1,
            Some(msg),
        ));
    }
    let quality = if evidences.len() >= 2 { 3 } else { 2 };
    // 正文过短或非 HTML（但有讨论证据）→ partial；否则 ready
    let status = if article_state == "thin" || article_state == "unsupported" {
        "partial"
    } else {
        "ready"
    };
    Ok(make_content(
        item,
        status,
        evidences,
        "article",
        quality,
        if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
    ))
}

// ==================== 论文（arXiv / HF Papers）：完整摘要即有效证据 ====================

async fn fetch_paper_content(app: &AppHandle, item: &Item) -> Result<ItemContent, String> {
    // 修复历史截断：arXiv 旧条目摘要被截到 500 字符，按需按 id 重取完整 abstract
    let mut abstract_text = item.description.clone();
    if item.source_id == "arxiv-ai" && item.description.len() <= 500 {
        let pid = item.id.strip_prefix("arxiv-").unwrap_or(&item.id);
        let client = reqwest::Client::new();
        if let Ok(r) = client
            .get(format!(
                "https://export.arxiv.org/api/query?id_list={}&max_results=1",
                pid
            ))
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
        {
            if let Ok(xml) = r.text().await {
                // AG-23（审计 P1-3 性能项）：按条目抓全文时复用静态 Regex
                static ABSTRACT_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                let re = ABSTRACT_RE.get_or_init(|| {
                    regex::Regex::new(r"(?s)<summary>([\s\S]*?)</summary>").expect("static regex")
                });
                if let Some(cap) = re.captures(&xml) {
                    let full = cap[1].trim().replace('\n', " ");
                    if full.len() > abstract_text.len() {
                        // 回写完整摘要到元数据，一次性修复
                        if let Ok(conn) = rusqlite::Connection::open(get_db_path(app)) {
                            let _ = conn.execute(
                                "UPDATE items SET description = ?1 WHERE id = ?2",
                                rusqlite::params![full, item.id],
                            );
                        }
                        abstract_text = full;
                    }
                }
            }
        }
    }

    Ok(build_paper_content(item, &abstract_text))
}

/// 论文内容的离线构建（A4）：仅用已存储的摘要/作者/分类/热度组装证据，不联网。
/// fetch_paper_content 与 convert_papers_offline 共用，保证两条路径产出一致。
fn build_paper_content(item: &Item, abstract_text: &str) -> ItemContent {
    if abstract_text.trim().is_empty() {
        return make_content(
            item,
            "partial",
            vec![],
            "abstract",
            1,
            Some("摘要缺失".to_string()),
        );
    }
    let mut parts = vec![format!("摘要：{}", abstract_text)];
    if let Some(author) = &item.author {
        parts.push(format!("作者：{}", author));
    }
    if let Some(topics) = &item.topics {
        parts.push(format!("分类：{}", topics));
    }
    if let Some(upvotes) = item.stars {
        parts.push(format!("社区热度：{} upvotes", upvotes));
    }
    make_content(
        item,
        "ready",
        vec![Evidence {
            id: "E1".to_string(),
            kind: "abstract".to_string(),
            title: "Abstract".to_string(),
            url: item.url.clone(),
            text: parts.join("\n"),
        }],
        "abstract",
        2,
        None,
    )
}

/// A4 论文离线转正文：arXiv / HF Papers 的完整摘要已存于 items.description，
/// 直接生成 item_contents 记录（不联网），让论文类覆盖率立即达到 ~100%。
/// 跳过疑似截断的 arXiv 旧摘要（description ≤500 字节），留给联网路径 fetch_paper_content 按 id 重取修复。
pub fn convert_papers_offline(app: &AppHandle) -> Result<serde_json::Value, String> {
    let ids: Vec<String> = {
        let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT i.id FROM items i LEFT JOIN item_contents c ON c.item_id = i.id
                 WHERE i.source_id IN ('arxiv-ai','huggingface-papers')
                   AND (c.item_id IS NULL OR c.status = 'failed')
                   AND i.description IS NOT NULL AND i.description != ''",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut converted = 0i64;
    let mut skipped_truncated = 0i64;
    for id in &ids {
        let item = match load_item(app, id) {
            Ok(i) => i,
            Err(_) => continue,
        };
        // 疑似截断的历史 arXiv 摘要：离线阶段跳过，交给联网路径按 id 重取完整 abstract
        if item.source_id == "arxiv-ai" && item.description.len() <= 500 {
            skipped_truncated += 1;
            continue;
        }
        let content = build_paper_content(&item, &item.description);
        save_content(app, &content)?;
        converted += 1;
    }

    Ok(serde_json::json!({
        "converted": converted,
        "skipped_truncated": skipped_truncated,
    }))
}

// ==================== HuggingFace 模型 ====================

async fn fetch_model_content(item: &Item) -> Result<ItemContent, String> {
    // 重定向 ≤3（规格安全要求一致）；hf-mirror 为固定可信 host
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| e.to_string())?;
    let model_id = &item.title;
    let mut evidences = Vec::new();
    let mut errors = Vec::new();

    // E1 Model Card
    match client
        .get(format!(
            "https://hf-mirror.com/{}/raw/main/README.md",
            model_id
        ))
        .header("User-Agent", "SophoNote-App")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            // 规格：响应体 ≤5MB，超限截断
            let text = read_body_capped(r, 5 * 1024 * 1024).await?;
            let truncated: String = text.chars().take(MAX_DOC_CHARS).collect();
            evidences.push(Evidence {
                id: "E1".to_string(),
                kind: "model_card".to_string(),
                title: "Model Card".to_string(),
                url: item.url.clone(),
                text: truncated,
            });
        }
        Ok(r) => errors.push(format!(
            "Model Card 抓取返回 {}，证据不足，不生成技术能力判断",
            r.status()
        )),
        Err(e) => errors.push(format!(
            "Model Card 抓取失败: {}，证据不足，不生成技术能力判断",
            e
        )),
    }

    // E2 模型元数据
    match client
        .get(format!("https://hf-mirror.com/api/models/{}", model_id))
        .header("User-Agent", "SophoNote-App")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => {
                let likes = v["likes"].as_i64().unwrap_or(0);
                let downloads = v["downloads"].as_i64().unwrap_or(0);
                let pipeline = v["pipeline_tag"].as_str().unwrap_or("未披露");
                let card = &v["cardData"];
                let license = card["license"].as_str().unwrap_or("未披露");
                let base = card["base_model"]
                    .as_str()
                    .or_else(|| card["base_model"].as_array().and_then(|a| a[0].as_str()))
                    .unwrap_or("未披露");
                evidences.push(Evidence {
                    id: "E2".to_string(),
                    kind: "metadata".to_string(),
                    title: "Model metadata".to_string(),
                    url: item.url.clone(),
                    text: format!(
                        "任务类型 {} · 下载量 {} · 点赞 {} · License {} · 基座模型 {}",
                        pipeline, downloads, likes, license, base
                    ),
                });
            }
            Err(e) => errors.push(format!("模型元数据解析失败: {}", e)),
        },
        Ok(r) => errors.push(format!("模型元数据返回 {}", r.status())),
        Err(e) => errors.push(format!("模型元数据抓取失败: {}", e)),
    }

    if evidences.is_empty() {
        let msg = if errors.is_empty() {
            "无 Model Card 且元数据获取失败，证据不足".to_string()
        } else {
            errors.join("；")
        };
        return Ok(make_content(
            item,
            "partial",
            vec![],
            "model_card",
            1,
            Some(msg),
        ));
    }
    let has_card = evidences.iter().any(|e| e.kind == "model_card");
    Ok(make_content(
        item,
        if has_card { "ready" } else { "partial" },
        evidences,
        "model_card",
        if has_card { 3 } else { 1 },
        if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
    ))
}

// ==================== ProductHunt 详情页（补抓：简介/话题/制作者/精选评论） ====================
//
// 列表端只给 tagline，证据不足以调用 AI（曾标 unsupported）。此处用 GraphQL `post(id:)`
// 抓详情页：description（完整简介）+ topics + makers + website + 精选评论。
// token 缺失或反爬拦截时给出明确原因；有 description 或足量评论才放行 AI 解读。

async fn fetch_producthunt_content(app: &AppHandle, item: &Item) -> Result<ItemContent, String> {
    let token = crate::commands::get_cached_api_key(app, "producthunt").unwrap_or_default();
    if token.is_empty() {
        return Ok(unsupported(
            item,
            "ProductHunt 详情需 developer token：设置 → 数据源 填入后，详情页/简介/评论即可作为证据",
        ));
    }
    // 列表条目 id 形如 "ph-<graphql_id>"，去掉前缀即 GraphQL Post id
    let ph_id = item.id.strip_prefix("ph-").unwrap_or(&item.id);

    let query = format!(
        r#"{{"query":"query {{ post(id: \"{id}\") {{ name tagline description url website votesCount commentsCount createdAt topics(first: 8) {{ nodes {{ name }} }} makers {{ name username }} comments(first: 6) {{ nodes {{ body votesCount }} }} }} }}"}}"#,
        id = ph_id
    );

    let client = reqwest::Client::new();
    let res = client
        .post("https://api.producthunt.com/v2/api/graphql")
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body(query)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = res.status();
    let data: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "ProductHunt 详情 API error: {}（可能触发反爬/限流，稍后重试）",
            status
        ));
    }
    if let Some(errors) = data["errors"].as_array() {
        if !errors.is_empty() {
            return Err(format!(
                "ProductHunt GraphQL error: {}",
                errors[0]["message"].as_str().unwrap_or("unknown")
            ));
        }
    }
    let post = &data["data"]["post"];
    if post.is_null() {
        return Ok(unsupported(item, "详情页不存在或已下架"));
    }

    let mut evidences = Vec::new();
    let mut errors = Vec::new();

    let description = post["description"].as_str().unwrap_or("").trim();
    let tagline = post["tagline"].as_str().unwrap_or("").trim();
    let website = post["website"].as_str().unwrap_or("");
    let votes = post["votesCount"].as_i64().unwrap_or(0);
    let comments_count = post["commentsCount"].as_i64().unwrap_or(0);
    let created = post["createdAt"].as_str().unwrap_or("");
    let topics: Vec<String> = post["topics"]["nodes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let makers: Vec<String> = post["makers"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // E1 详情页主体：完整简介 + tagline + 话题 + 制作者 + 官网 + 热度
    let mut body_parts = Vec::new();
    if !description.is_empty() {
        body_parts.push(description.to_string());
    }
    if !tagline.is_empty() && tagline != description {
        body_parts.push(format!("Tagline: {}", tagline));
    }
    if !topics.is_empty() {
        body_parts.push(format!("Topics: {}", topics.join(", ")));
    }
    if !makers.is_empty() {
        body_parts.push(format!("Makers: {}", makers.join(", ")));
    }
    if !website.is_empty() {
        body_parts.push(format!("Website: {}", website));
    }
    body_parts.push(format!(
        "Votes {} · Comments {} · Launched {}",
        votes, comments_count, created
    ));
    let body_text: String = body_parts
        .join("\n\n")
        .chars()
        .take(MAX_DOC_CHARS)
        .collect();
    evidences.push(Evidence {
        id: "E1".to_string(),
        kind: "article".to_string(),
        title: "Product detail".to_string(),
        url: item.url.clone(),
        text: body_text,
    });

    // E2 精选评论（反映真实用户反馈）
    let comments: Vec<String> = post["comments"]["nodes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    c["body"]
                        .as_str()
                        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                })
                .filter(|t| t.len() > 30)
                .take(6)
                .collect()
        })
        .unwrap_or_default();
    if !comments.is_empty() {
        let joined: String = comments
            .join("\n---\n")
            .chars()
            .take(MAX_COMMENT_CHARS)
            .collect();
        evidences.push(Evidence {
            id: "E2".to_string(),
            kind: "discussion".to_string(),
            title: "PH comments".to_string(),
            url: item.url.clone(),
            text: joined,
        });
    } else if errors.is_empty() {
        errors.push("无足量精选评论".to_string());
    }

    // 证据门槛：只有完整简介（或简介+评论）才放行 AI；仅 tagline 仍视为不足
    let has_description = !description.is_empty();
    if !has_description && comments.is_empty() {
        return Ok(make_content(
            item,
            "partial",
            evidences,
            "article",
            1,
            Some("详情页无完整简介且无评论，证据不足".to_string()),
        ));
    }

    let quality = if has_description && !comments.is_empty() {
        3
    } else {
        2
    };
    Ok(make_content(
        item,
        "ready",
        evidences,
        "article",
        quality,
        if errors.is_empty() {
            None
        } else {
            Some(errors.join("；"))
        },
    ))
}
