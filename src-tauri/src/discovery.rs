//! NEXT-048 发现五断面数据面：全量评分持久化（Bridge 写）+ 只读 feed（Tauri 读）。
//!
//! 边界（PRD INFO-12 / 架构发现数据面）：Hermes Skill（sophonote-ai-radar）
//! 是唯一打分/标注者，经 Bridge `save_discovery_scores` 写入；Rust 只做结构性校验
//! （分数区间、aspect 枚举、topics 数量上限）与只读查询，**不做任何打分或自然语言解析**。
//! 精选 = aspect∈五面 ∧ 近 7 天 ∧ ≥8.5 ∧ 已有深度解读；全部 AI 动态 = ≥7；两断面共用本 feed。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 精选五面（与 Skill references/aspect-rules.md 同口径，仅五值）
pub const ASPECTS: [&str; 5] = ["模型", "产品", "行业", "论文", "观点"];

pub const COMPANY_TOPICS: [&str; 15] = [
    "OpenAI / ChatGPT",
    "Anthropic / Claude",
    "Google / Gemini",
    "DeepSeek",
    "通义千问 Qwen",
    "Kimi / 月之暗面",
    "MiniMax",
    "智谱 GLM",
    "xAI / Grok",
    "Meta / Llama",
    "Microsoft / Copilot",
    "NVIDIA 英伟达",
    "Hugging Face",
    "Cursor",
    "OpenRouter",
];
pub const TECH_TOPICS: [&str; 14] = [
    "Agent 智能体",
    "AI 编码",
    "推理能力",
    "多模态",
    "图像生成",
    "AI 视频",
    "语音与音频",
    "具身智能",
    "端侧 AI",
    "开源生态",
    "部署工程",
    "数据与训练",
    "安全对齐",
    "MCP 与工具调用",
];
pub const CONTENT_TOPICS: [&str; 9] = [
    "模型发布",
    "产品更新",
    "论文研究",
    "评测基准",
    "教程实践",
    "大佬观点",
    "现象与趋势",
    "行业动态",
    "政策监管",
];

fn is_allowed_topic(topic: &str) -> bool {
    COMPANY_TOPICS.contains(&topic)
        || TECH_TOPICS.contains(&topic)
        || CONTENT_TOPICS.contains(&topic)
}

pub fn now_scored_at() -> String {
    // 与 SQLite CURRENT_TIMESTAMP 同格式（UTC），保证 datetime('now','-N days') 可比较
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ==================== 只读 feed ====================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFeedRow {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub title: String,
    pub url: Option<String>,
    pub author: Option<String>,
    pub stars: Option<i32>,
    pub published_at: Option<String>,
    pub fetched_at: Option<String>,
    pub ai_summary: Option<String>,
    pub ai_tags: Option<String>,
    pub content_status: Option<String>,
    pub quality_level: Option<i32>,
    pub ai_score: f64,
    pub ai_scored_at: String,
    pub aspect: Option<String>,
    pub ai_topics: Vec<String>,
    pub ai_reason: Option<String>,
    // 行渲染补充字段（与 ItemCard/DiscoverRow 对齐）
    pub status: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub forks: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFeedPage {
    pub rows: Vec<DiscoveryFeedRow>,
    /// keyset 游标（ai_scored_at|id）；None = 已到底
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFeedQuery {
    pub aspect: Option<String>,
    pub source: Option<String>,
    pub topic: Option<String>,
    pub min_score: Option<f64>,
    /// 打分时间窗（天）：精选=7；全部=不传
    pub window_days: Option<i64>,
    /// 精选必须已由 Hermes Skill 成功保存深度解读；全部/主题/报告不传。
    pub require_deep: Option<bool>,
    /// 深度解读补全任务专用：仅返回尚无有效 deep 的已评分条目。
    pub missing_deep: Option<bool>,
    /// 报告读取使用的闭开区间；普通前端 feed 不传。
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

fn parse_cursor(cursor: &str) -> Option<(String, String)> {
    let (ts, id) = cursor.split_once('|')?;
    if ts.is_empty() || id.is_empty() {
        return None;
    }
    Some((ts.to_string(), id.to_string()))
}

/// 全量已打分条目的只读游标分页查询。Rust 不解释分数语义，只按参数过滤。
pub fn query_discovery_feed(
    conn: &Connection,
    query: &DiscoveryFeedQuery,
) -> Result<DiscoveryFeedPage, String> {
    let limit = query.limit.unwrap_or(40).clamp(1, 500);
    let min_score = query.min_score.unwrap_or(0.0);

    let mut sql = String::from(
        "SELECT i.id, i.source_id, s.name, i.item_type, i.title, i.url, i.author, i.stars, \
                i.published_at, i.fetched_at, i.ai_summary, i.ai_tags, \
                c.status, c.quality_level, \
                i.ai_score, i.ai_scored_at, i.aspect, i.ai_topics, i.ai_reason, \
                i.status, i.description, i.language, i.forks \
         FROM items i JOIN sources s ON s.id = i.source_id \
         LEFT JOIN item_contents c ON c.item_id = i.id \
         WHERE i.ai_score IS NOT NULL AND i.ai_scored_at IS NOT NULL AND i.ai_score >= ?1 \
         AND datetime(COALESCE(i.expires_at, datetime(i.fetched_at, '+168 hours'))) > datetime('now')",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(min_score)];

    if let Some(aspect) = query.aspect.as_deref().filter(|value| !value.is_empty()) {
        if !ASPECTS.contains(&aspect) {
            return Err(format!("aspect 只能是 {} 之一", ASPECTS.join("、")));
        }
        sql.push_str(" AND i.aspect = ?2");
        params.push(Box::new(aspect.to_string()));
    }
    if let Some(source) = query.source.as_deref().filter(|value| !value.is_empty()) {
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND i.source_id = ?{idx}"));
        params.push(Box::new(source.to_string()));
    }
    if let Some(topic) = query.topic.as_deref().filter(|value| !value.is_empty()) {
        // ai_topics 存 JSON 数组（serde_json 序列化），按成员精确匹配
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND i.ai_topics LIKE ?{idx}"));
        params.push(Box::new(format!(
            "%\"{}\"%",
            topic.replace(['"', '%', '_'], "")
        )));
    }
    if let Some(days) = query.window_days.filter(|days| *days > 0) {
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND i.ai_scored_at >= datetime('now', ?{idx})"));
        params.push(Box::new(format!("-{days} days")));
    }
    if query.require_deep.unwrap_or(false) && query.missing_deep.unwrap_or(false) {
        return Err("requireDeep 与 missingDeep 不能同时为 true".into());
    }
    if query.require_deep.unwrap_or(false) {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM articles a \
             WHERE a.item_id = i.id AND a.article_type = 'deep-dive')",
        );
    }
    if query.missing_deep.unwrap_or(false) {
        sql.push_str(
            " AND NOT EXISTS (SELECT 1 FROM articles a \
             WHERE a.item_id = i.id AND a.article_type = 'deep-dive')",
        );
    }
    if let Some(from) = query.from_date.as_deref().filter(|value| !value.is_empty()) {
        chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d")
            .map_err(|_| "fromDate 必须是 YYYY-MM-DD".to_string())?;
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND i.ai_scored_at >= ?{idx}"));
        params.push(Box::new(format!("{from} 00:00:00")));
    }
    if let Some(to) = query.to_date.as_deref().filter(|value| !value.is_empty()) {
        chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d")
            .map_err(|_| "toDate 必须是 YYYY-MM-DD".to_string())?;
        let idx = params.len() + 1;
        sql.push_str(&format!(" AND i.ai_scored_at < ?{idx}"));
        params.push(Box::new(format!("{to} 00:00:00")));
    }
    if let Some(cursor) = query.cursor.as_deref().filter(|value| !value.is_empty()) {
        let (ts, id) = parse_cursor(cursor).ok_or("cursor 格式无效")?;
        let idx = params.len() + 1;
        sql.push_str(&format!(
            " AND (i.ai_scored_at < ?{idx} OR (i.ai_scored_at = ?{idx} AND i.id < ?{}))",
            idx + 1
        ));
        params.push(Box::new(ts));
        params.push(Box::new(id));
    }
    sql.push_str(" ORDER BY i.ai_scored_at DESC, i.id DESC");
    let idx = params.len() + 1;
    sql.push_str(&format!(" LIMIT ?{idx}"));
    params.push(Box::new(limit + 1));

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("查询发现 feed 失败: {error}"))?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            let topics_json: Option<String> = row.get(17)?;
            Ok(DiscoveryFeedRow {
                id: row.get(0)?,
                source_id: row.get(1)?,
                source_name: row.get(2)?,
                item_type: row.get(3)?,
                title: row.get(4)?,
                url: row.get(5)?,
                author: row.get(6)?,
                stars: row.get(7)?,
                published_at: row.get(8)?,
                fetched_at: row.get(9)?,
                ai_summary: row.get(10)?,
                ai_tags: row.get(11)?,
                content_status: row.get(12).ok().flatten(),
                quality_level: row.get(13).ok().flatten(),
                ai_score: row.get(14)?,
                ai_scored_at: row.get(15)?,
                aspect: row.get(16)?,
                ai_topics: topics_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
                    .unwrap_or_default(),
                ai_reason: row.get(18)?,
                status: row.get(19)?,
                description: row.get(20)?,
                language: row.get(21)?,
                forks: row.get(22)?,
            })
        })
        .map_err(|error| format!("查询发现 feed 失败: {error}"))?;

    let mut collected: Vec<DiscoveryFeedRow> = Vec::new();
    while let Some(row) = rows
        .next()
        .transpose()
        .map_err(|error| format!("读取发现 feed 行失败: {error}"))?
    {
        collected.push(row);
    }

    let has_more = collected.len() as i64 > limit;
    if has_more {
        collected.truncate(limit as usize);
    }
    let next_cursor = collected
        .last()
        .filter(|_| has_more)
        .map(|last| format!("{}|{}", last.ai_scored_at, last.id));
    Ok(DiscoveryFeedPage {
        rows: collected,
        next_cursor,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryTopicSummary {
    pub name: String,
    pub group: String,
    pub count: usize,
}

pub fn query_topic_summary(
    conn: &Connection,
    min_score: f64,
    window_days: Option<i64>,
) -> Result<Vec<DiscoveryTopicSummary>, String> {
    let page = query_discovery_feed(
        conn,
        &DiscoveryFeedQuery {
            min_score: Some(min_score),
            window_days,
            // 主题地图只统计实际能进入「全部 AI 动态」的已解读条目，避免点入空页。
            require_deep: Some(true),
            missing_deep: None,
            limit: Some(500),
            ..Default::default()
        },
    )?;
    let mut result = Vec::with_capacity(38);
    for (group, topics) in [
        ("公司与模型", COMPANY_TOPICS.as_slice()),
        ("技术方向", TECH_TOPICS.as_slice()),
        ("内容形态", CONTENT_TOPICS.as_slice()),
    ] {
        for topic in topics {
            result.push(DiscoveryTopicSummary {
                name: (*topic).to_string(),
                group: group.to_string(),
                count: page
                    .rows
                    .iter()
                    .filter(|row| row.ai_topics.iter().any(|value| value == topic))
                    .count(),
            });
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLeaderboardRow {
    pub date: String,
    pub model_key: String,
    pub name: String,
    pub vendor: Option<String>,
    pub rank: i64,
    pub consensus: f64,
    pub meta: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLeaderboardSnapshot {
    pub date: Option<String>,
    pub rows: Vec<ModelLeaderboardRow>,
}

pub fn save_model_leaderboard_snapshot(
    conn: &Connection,
    arguments: &Value,
) -> Result<Value, String> {
    let date = arguments
        .get("date")
        .and_then(Value::as_str)
        .ok_or("date 必填")?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| "date 必须是 YYYY-MM-DD".to_string())?;
    let entries = arguments
        .get("entries")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 100)
        .ok_or("entries 必须是 1-100 条数组")?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("写入模型榜失败: {error}"))?;
    tx.execute(
        "DELETE FROM model_leaderboard_snapshots WHERE date = ?1",
        rusqlite::params![date],
    )
    .map_err(|error| format!("覆盖模型榜失败: {error}"))?;

    for (index, entry) in entries.iter().enumerate() {
        let model_key = entry
            .get("modelKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 160)
            .ok_or("modelKey 无效")?;
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.chars().count() <= 160)
            .ok_or("name 无效")?;
        let vendor = entry
            .get("vendor")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let consensus = entry
            .get("consensus")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            .ok_or("consensus 必须在 0 到 100 之间")?;
        let rank = entry
            .get("rank")
            .and_then(Value::as_i64)
            .unwrap_or((index + 1) as i64);
        if rank != (index + 1) as i64 {
            return Err("rank 必须从 1 开始连续递增".into());
        }
        let meta = entry.get("meta").cloned().unwrap_or_else(|| json!({}));
        if !meta.is_object() {
            return Err("meta 必须是对象".into());
        }
        let id = format!("model-board-{date}-{model_key}");
        tx.execute(
            "INSERT INTO model_leaderboard_snapshots \
             (id, date, model_key, name, vendor, rank, consensus, meta_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                id,
                date,
                model_key,
                name,
                vendor,
                rank,
                (consensus * 10.0).round() / 10.0,
                serde_json::to_string(&meta).map_err(|error| error.to_string())?,
            ],
        )
        .map_err(|error| format!("写入模型榜失败: {error}"))?;
    }
    tx.commit()
        .map_err(|error| format!("提交模型榜失败: {error}"))?;
    Ok(json!({ "success": true, "date": date, "saved": entries.len() }))
}

pub fn query_model_leaderboard(
    conn: &Connection,
    requested_date: Option<&str>,
) -> Result<ModelLeaderboardSnapshot, String> {
    let date = match requested_date.filter(|value| !value.is_empty()) {
        Some(value) => {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| "date 必须是 YYYY-MM-DD".to_string())?;
            Some(value.to_string())
        }
        None => conn
            .query_row(
                "SELECT MAX(date) FROM model_leaderboard_snapshots",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| format!("查询模型榜日期失败: {error}"))?,
    };
    let Some(date) = date else {
        return Ok(ModelLeaderboardSnapshot {
            date: None,
            rows: Vec::new(),
        });
    };
    let mut stmt = conn
        .prepare(
            "SELECT date, model_key, name, vendor, rank, consensus, meta_json \
             FROM model_leaderboard_snapshots WHERE date = ?1 ORDER BY rank ASC, name ASC",
        )
        .map_err(|error| format!("查询模型榜失败: {error}"))?;
    let rows = stmt
        .query_map(rusqlite::params![date], |row| {
            let raw: Option<String> = row.get(6)?;
            Ok(ModelLeaderboardRow {
                date: row.get(0)?,
                model_key: row.get(1)?,
                name: row.get(2)?,
                vendor: row.get(3)?,
                rank: row.get(4)?,
                consensus: row.get(5)?,
                meta: raw
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok())
                    .unwrap_or_else(|| json!({})),
            })
        })
        .map_err(|error| format!("查询模型榜失败: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取模型榜失败: {error}"))?;
    Ok(ModelLeaderboardSnapshot {
        date: Some(date),
        rows,
    })
}

// ==================== Bridge 写：全量评分持久化 ====================

/// 批量持久化打分趟产物。单条结构错误只拒该条（记入 rejected），不中断整批；
/// 参数顶层非法才整体失败。条目必须已存在（打分不创建条目）。
pub fn apply_discovery_scores(conn: &Connection, arguments: &Value) -> Result<Value, String> {
    let scores = arguments
        .get("scores")
        .and_then(Value::as_array)
        .ok_or_else(|| "scores 必须是非空数组".to_string())?;
    if scores.is_empty() {
        return Err("scores 必须是非空数组".into());
    }
    if scores.len() > 200 {
        return Err("单批最多 200 条评分".into());
    }

    let scored_at = now_scored_at();
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("写入评分失败: {error}"))?;
    let mut saved = 0usize;
    let mut rejected: Vec<Value> = Vec::new();

    for entry in scores {
        let item_id = entry.get("itemId").and_then(Value::as_str).unwrap_or("");
        let reject = |reason: String| -> Value { json!({ "itemId": item_id, "reason": reason }) };
        match apply_one_score(&tx, entry, &scored_at) {
            Ok(()) => saved += 1,
            Err(reason) => rejected.push(reject(reason)),
        }
    }

    tx.commit()
        .map_err(|error| format!("提交评分失败: {error}"))?;
    Ok(json!({ "success": true, "saved": saved, "rejected": rejected }))
}

fn apply_one_score(
    tx: &rusqlite::Transaction,
    entry: &Value,
    scored_at: &str,
) -> Result<(), String> {
    let item_id = entry
        .get("itemId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| "itemId 无效".to_string())?;
    let score = entry
        .get("score")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (0.0..=10.0).contains(value))
        .ok_or_else(|| "score 必须在 0 到 10 之间".to_string())?;
    // 双刻度纪律：落库保留一位小数（0-10），前端展示按分档着色
    let score = (score * 10.0).round() / 10.0;

    let aspect = match entry.get("aspect") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else if ASPECTS.contains(&value) {
                Some(value.to_string())
            } else {
                return Err(format!("aspect 只能是 {} 之一或 null", ASPECTS.join("、")));
            }
        }
        Some(_) => return Err("aspect 类型无效".into()),
    };

    let topics: Vec<String> = match entry.get("aiTopics") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => {
            if values.len() > 3 {
                return Err("单条 aiTopics 最多 3 个".into());
            }
            let mut parsed = Vec::with_capacity(values.len());
            for value in values {
                let topic = value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && value.len() <= 60)
                    .ok_or_else(|| "aiTopics 成员必须是非空字符串".to_string())?;
                if !is_allowed_topic(topic) {
                    return Err(format!("aiTopics 包含非受控主题：{topic}"));
                }
                parsed.push(topic.to_string());
            }
            parsed
        }
        Some(_) => return Err("aiTopics 类型无效".into()),
    };

    let reason = match entry.get("reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else if value.chars().count() <= 160 {
                Some(value.to_string())
            } else {
                return Err("reason 最多 160 字".into());
            }
        }
        Some(_) => return Err("reason 类型无效".into()),
    };

    let exists = tx
        .query_row(
            "SELECT 1 FROM items WHERE id = ?1",
            rusqlite::params![item_id],
            |_| Ok(()),
        )
        .is_ok();
    if !exists {
        return Err("条目不存在（打分不创建条目）".into());
    }

    tx.execute(
        "UPDATE items SET ai_score = ?2, ai_scored_at = ?3, aspect = ?4, ai_topics = ?5, ai_reason = ?6 WHERE id = ?1",
        rusqlite::params![
            item_id,
            score,
            scored_at,
            aspect,
            serde_json::to_string(&topics).map_err(|error| format!("序列化 topics 失败: {error}"))?,
            reason
        ],
    )
    .map_err(|error| format!("写入评分失败: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_schema;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        create_schema(&conn).expect("schema");
        conn.execute(
            "INSERT INTO sources (id, name, source_type) VALUES ('github-trending', 'GitHub', 'github')",
            [],
        )
        .expect("seed source");
        conn
    }

    fn seed_item(conn: &Connection, id: &str, title: &str) {
        conn.execute(
            "INSERT INTO items (id, source_id, item_type, title, url, description, published_at, fetched_at, status) \
             VALUES (?1, 'github-trending', 'repo', ?2, 'https://example.com', '', '2026-08-17', '2026-08-17', 'unread')",
            rusqlite::params![id, title],
        )
        .expect("seed item");
    }

    #[test]
    fn apply_scores_persists_full_pass_including_rejected() {
        let conn = test_conn();
        seed_item(&conn, "i-high", "高分条目");
        seed_item(&conn, "i-mid", "七分条目");

        let result = apply_discovery_scores(
            &conn,
            &json!({
                "scores": [
                    { "itemId": "i-high", "score": 9.24, "aspect": "模型",
                      "aiTopics": ["OpenAI / ChatGPT", "模型发布"] },
                    { "itemId": "i-mid", "score": 7.0, "aspect": null,
                      "reason": "信息量一般，未达精选线" },
                    { "itemId": "i-missing", "score": 8.0 },
                    { "itemId": "i-high", "score": 11.0 }
                ]
            }),
        )
        .expect("apply");
        assert_eq!(result["saved"], json!(2));
        assert_eq!(result["rejected"].as_array().unwrap().len(), 2);

        let (score, aspect, topics): (f64, Option<String>, String) = conn
            .query_row(
                "SELECT ai_score, aspect, ai_topics FROM items WHERE id = 'i-high'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read back");
        // 一位小数纪律
        assert_eq!(score, 9.2);
        assert_eq!(aspect.as_deref(), Some("模型"));
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&topics).unwrap(),
            vec!["OpenAI / ChatGPT", "模型发布"]
        );
    }

    #[test]
    fn apply_scores_rejects_structural_violations() {
        let conn = test_conn();
        seed_item(&conn, "i-1", "条目一");
        let bad = apply_discovery_scores(
            &conn,
            &json!({
                "scores": [
                    { "itemId": "i-1", "score": 8.0, "aspect": "教程" },
                    { "itemId": "i-1", "score": 8.0,
                      "aiTopics": ["a", "b", "c", "d"] }
                ]
            }),
        )
        .expect("apply");
        assert_eq!(bad["saved"], json!(0));
        assert_eq!(bad["rejected"].as_array().unwrap().len(), 2);
        // 被拒条目不落分
        let scored: Option<f64> = conn
            .query_row("SELECT ai_score FROM items WHERE id = 'i-1'", [], |row| {
                row.get(0)
            })
            .expect("read");
        assert!(scored.is_none());
    }

    fn score_and_persist(
        conn: &Connection,
        id: &str,
        score: f64,
        aspect: Option<&str>,
        scored_at: &str,
    ) {
        conn.execute(
            "UPDATE items SET ai_score = ?2, ai_scored_at = ?3, aspect = ?4, ai_topics = '[]' WHERE id = ?1",
            rusqlite::params![id, score, scored_at, aspect],
        )
        .expect("persist score");
    }

    #[test]
    fn feed_filters_min_score_aspect_topic_and_window() {
        let conn = test_conn();
        for (id, title) in [
            ("f-1", "模型高分"),
            ("f-2", "模型低分"),
            ("f-3", "观点高分"),
            ("f-4", "过期高分"),
        ] {
            seed_item(&conn, id, title);
        }
        // 相对当前时间生成：窗口过滤使用 SQLite datetime('now')，固定日期会随日期推移失效
        let now = now_scored_at();
        let old = (chrono::Utc::now() - chrono::Duration::days(20))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        score_and_persist(&conn, "f-1", 9.0, Some("模型"), &now);
        score_and_persist(&conn, "f-2", 6.5, Some("模型"), &now);
        score_and_persist(&conn, "f-3", 8.8, Some("观点"), &now);
        score_and_persist(&conn, "f-4", 9.5, Some("模型"), &old);
        // topics 写在打分之后（score_and_persist 会重置 ai_topics）
        conn.execute(
            "UPDATE items SET ai_topics = ?1 WHERE id = 'f-1'",
            rusqlite::params![serde_json::to_string(&vec!["DeepSeek", "模型发布"]).unwrap()],
        )
        .expect("topics");

        // 全部 AI 动态：≥7，不过窗——f-2 被过滤；时间线序（ai_scored_at DESC, id DESC）
        let all = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(7.0),
                ..Default::default()
            },
        )
        .expect("all feed");
        let ids: Vec<&str> = all.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["f-3", "f-1", "f-4"]);

        // 精选：≥8.5 ∧ 近 7 天——过期 f-4 与低分 f-2 均不在；同刻按 id 倒序
        let featured = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(8.5),
                window_days: Some(7),
                ..Default::default()
            },
        )
        .expect("featured feed");
        let ids: Vec<&str> = featured.rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["f-3", "f-1"]);

        // aspect 过滤
        let models = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(8.5),
                window_days: Some(7),
                aspect: Some("模型".into()),
                ..Default::default()
            },
        )
        .expect("aspect feed");
        assert_eq!(models.rows.len(), 1);
        assert_eq!(models.rows[0].id, "f-1");

        // 受控主题过滤
        let deepseek = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(7.0),
                topic: Some("DeepSeek".into()),
                ..Default::default()
            },
        )
        .expect("topic feed");
        assert_eq!(deepseek.rows.len(), 1);
        assert_eq!(deepseek.rows[0].ai_topics, vec!["DeepSeek", "模型发布"]);

        // aspect 白名单 fail-closed
        assert!(query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                aspect: Some("教程".into()),
                ..Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn featured_feed_can_require_a_saved_deep_dive() {
        let conn = test_conn();
        seed_item(&conn, "deep-ready", "已有深度解读");
        seed_item(&conn, "deep-missing", "尚无深度解读");
        let now = now_scored_at();
        score_and_persist(&conn, "deep-ready", 9.0, Some("模型"), &now);
        score_and_persist(&conn, "deep-missing", 9.0, Some("模型"), &now);
        conn.execute(
            "INSERT INTO articles (id, item_id, title, content, article_type) \
             VALUES ('deep-1', 'deep-ready', '深度解读', '', 'deep-dive')",
            [],
        )
        .expect("seed deep dive");

        let page = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(8.5),
                window_days: Some(7),
                require_deep: Some(true),
                ..Default::default()
            },
        )
        .expect("featured feed");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, "deep-ready");
    }

    #[test]
    fn backfill_feed_returns_only_items_missing_deep_dives() {
        let conn = test_conn();
        seed_item(&conn, "deep-ready", "已有深度解读");
        seed_item(&conn, "deep-missing", "尚无深度解读");
        let now = now_scored_at();
        score_and_persist(&conn, "deep-ready", 7.0, Some("模型"), &now);
        score_and_persist(&conn, "deep-missing", 7.0, Some("模型"), &now);
        conn.execute(
            "INSERT INTO articles (id, item_id, title, content, article_type) \
             VALUES ('deep-1', 'deep-ready', '深度解读', '', 'deep-dive')",
            [],
        )
        .expect("seed deep dive");

        let page = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(7.0),
                missing_deep: Some(true),
                ..Default::default()
            },
        )
        .expect("backfill feed");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, "deep-missing");
    }

    #[test]
    fn feed_cursor_paginates_by_scored_at_desc() {
        let conn = test_conn();
        for index in 0..5 {
            let id = format!("c-{index}");
            seed_item(&conn, &id, &format!("条目 {index}"));
            let at = format!("2026-08-17 0{index}:00:00");
            score_and_persist(&conn, &id, 8.0, None, &at);
        }

        let first = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(7.0),
                limit: Some(2),
                ..Default::default()
            },
        )
        .expect("page 1");
        assert_eq!(
            first
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c-4", "c-3"]
        );
        let cursor = first.next_cursor.expect("has more");

        let second = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(7.0),
                limit: Some(2),
                cursor: Some(cursor),
                ..Default::default()
            },
        )
        .expect("page 2");
        assert_eq!(
            second
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c-2", "c-1"]
        );

        let third = query_discovery_feed(
            &conn,
            &DiscoveryFeedQuery {
                min_score: Some(7.0),
                limit: Some(2),
                cursor: second.next_cursor,
                ..Default::default()
            },
        )
        .expect("page 3");
        assert_eq!(third.rows.len(), 1);
        assert!(third.next_cursor.is_none());
    }

    #[test]
    fn scores_reject_topics_outside_controlled_taxonomy() {
        let conn = test_conn();
        seed_item(&conn, "topic-bad", "自由标签");
        let result = apply_discovery_scores(
            &conn,
            &json!({"scores": [{
                "itemId": "topic-bad",
                "score": 8.0,
                "aiTopics": ["随手发明的主题"]
            }]}),
        )
        .expect("batch result");
        assert_eq!(result["saved"], 0);
        assert_eq!(result["rejected"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn topic_summary_returns_all_38_topics_with_counts() {
        let conn = test_conn();
        seed_item(&conn, "topic-1", "DeepSeek 发布模型");
        score_and_persist(&conn, "topic-1", 9.0, Some("模型"), &now_scored_at());
        conn.execute(
            "UPDATE items SET ai_topics = ?1 WHERE id = 'topic-1'",
            rusqlite::params![serde_json::to_string(&vec!["DeepSeek", "模型发布"]).unwrap()],
        )
        .unwrap();
        // Article 正文以 Markdown 文件为真相源，SQLite 行只承担已成功写盘后的索引。
        conn.execute(
            "INSERT INTO articles (id, item_id, title, content, article_type) \
             VALUES ('topic-deep', 'topic-1', '深度解读', '', 'deep-dive')",
            [],
        )
        .unwrap();
        let rows = query_topic_summary(&conn, 7.0, Some(7)).expect("summary");
        assert_eq!(rows.len(), 38);
        assert_eq!(
            rows.iter()
                .find(|row| row.name == "DeepSeek")
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.name == "模型发布")
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            rows.iter().find(|row| row.name == "Cursor").unwrap().count,
            0
        );
    }

    #[test]
    fn model_board_snapshot_is_atomic_and_idempotent() {
        let conn = test_conn();
        let first = save_model_leaderboard_snapshot(
            &conn,
            &json!({
                "date": "2026-08-17",
                "entries": [
                    {"modelKey":"a", "name":"Model A", "vendor":"Vendor", "rank":1, "consensus":91.24, "meta":{"releaseDate":"2026-08-01"}},
                    {"modelKey":"b", "name":"Model B", "rank":2, "consensus":80.0}
                ]
            }),
        )
        .expect("first snapshot");
        assert_eq!(first["saved"], 2);
        save_model_leaderboard_snapshot(
            &conn,
            &json!({
                "date": "2026-08-17",
                "entries": [
                    {"modelKey":"a", "name":"Model A2", "rank":1, "consensus":93.0}
                ]
            }),
        )
        .expect("replace snapshot");
        let snapshot = query_model_leaderboard(&conn, None).expect("query latest");
        assert_eq!(snapshot.date.as_deref(), Some("2026-08-17"));
        assert_eq!(snapshot.rows.len(), 1);
        assert_eq!(snapshot.rows[0].name, "Model A2");
        assert_eq!(snapshot.rows[0].consensus, 93.0);
    }
}
