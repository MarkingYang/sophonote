//! NB-14 全局搜索（Track A · 只读扩展文件）：笔记 / 收件箱条目 / AI 解读三域融合检索。
//!
//! 关键词（SQLite LIKE，标题加权）+ 语义（embedding + vec 三通道）在**后端融合排序**，
//! 对外统一结果列表，**不暴露检索通道**（用户指令：对客户不展示关键词/语义区分）。
//! 嵌入未配置或失败时静默退化为纯关键词；vec 表不存在时语义通道为零。
//!
//! 定位（docs/architecture.md 轨道 A 所有权）：只读扩展文件——DB 与 vec 表只读，
//! 不碰存储写路径、无 schema 变更；嵌入与 vec 查询复用既有命令函数（ai_generate_embedding / vector::*）。

use std::collections::HashMap;

use serde::Serialize;
use tauri::AppHandle;

use crate::commands::{ai_generate_embedding, ApiResponse};
use crate::db::get_db_path;

/// 统一命中（前端按 kind 跳转对应空间，不展示通道来源）
#[derive(Serialize, Clone)]
pub struct GlobalHit {
    /// note = 笔记/日记 · article = AI 解读 · item = 收件箱条目
    pub kind: String,
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
    /// note/article 为 article_type，item 为 item_type（前端徽章用）
    pub sub_type: Option<String>,
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// 关键词片段：首命中 ±上下文，换行压平；未命中退回开头摘要
fn kw_snippet(content: &str, q: &str) -> String {
    let flat: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let ql = q.to_lowercase();
    let (start, end) = match lower.find(&ql) {
        Some(idx) => {
            let s = floor_boundary(&flat, idx.saturating_sub(40));
            let e = ceil_boundary(&flat, (idx + q.len() + 60).min(flat.len()));
            (s, e)
        }
        None => (0, ceil_boundary(&flat, 80.min(flat.len()))),
    };
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&flat[start..end]);
    if end < flat.len() {
        out.push('…');
    }
    out
}

/// 全局搜索：关键词 + 语义后端融合，统一排序，通道不外露
#[tauri::command]
pub async fn global_search(
    app: AppHandle,
    query: String,
    limit: Option<usize>,
) -> ApiResponse<Vec<GlobalHit>> {
    match global_search_inner(&app, &query, limit.unwrap_or(30)).await {
        Ok(hits) => ApiResponse::ok(hits),
        Err(e) => ApiResponse::err(e),
    }
}

async fn global_search_inner(
    app: &AppHandle,
    query: &str,
    limit: usize,
) -> Result<Vec<GlobalHit>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }

    // —— 1. 关键词通道：articles（标题加权）+ items ——
    let mut merged: HashMap<(String, String), GlobalHit> = HashMap::new();
    let like = format!("%{}%", q);
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;

    {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, content, article_type FROM articles \
                 WHERE title LIKE ?1 OR content LIKE ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&like], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            let (id, title, content, article_type) = row;
            let kind = if article_type == "manual" || article_type == "journal" {
                "note"
            } else {
                "article"
            };
            let score = if title.to_lowercase().contains(&q.to_lowercase()) {
                1.0
            } else {
                0.55
            };
            merged.insert(
                (kind.to_string(), id.clone()),
                GlobalHit {
                    kind: kind.to_string(),
                    id,
                    title,
                    snippet: kw_snippet(&content, q),
                    score,
                    sub_type: Some(article_type),
                },
            );
        }
    }
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, item_type FROM items \
                 WHERE (title LIKE ?1 OR description LIKE ?1) \
                 AND datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([&like], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            let (id, title, description, item_type) = row;
            let score = if title.to_lowercase().contains(&q.to_lowercase()) {
                1.0
            } else {
                0.55
            };
            merged.insert(
                ("item".to_string(), id.clone()),
                GlobalHit {
                    kind: "item".to_string(),
                    id,
                    title,
                    snippet: kw_snippet(&description, q),
                    score,
                    sub_type: Some(item_type),
                },
            );
        }
    }
    drop(conn);

    // —— 2. 语义通道：嵌入失败/未配置静默退化纯关键词 ——
    let vector = match ai_generate_embedding(app.clone(), q.to_string()).await {
        r if r.success => r.data,
        _ => None,
    };
    if let Some(vec) = vector {
        // 条目级 + 条目 chunk 级：同 id 取距离更小者，chunk 文本作片段
        let mut item_sem: HashMap<String, (f64, Option<String>)> = HashMap::new();
        if let Some(hits) =
            crate::vector::vec_search_chunks(app.clone(), vec.clone(), Some(20)).data
        {
            for h in hits {
                let s = 1.0 / (1.0 + h.distance as f64);
                let e = item_sem.entry(h.item.id.clone()).or_insert((0.0, None));
                if s > e.0 {
                    *e = (s, Some(h.chunk_text));
                }
            }
        }
        if let Some(hits) = crate::vector::vec_search(app.clone(), vec.clone(), Some(20)).data {
            for h in hits {
                let s = 1.0 / (1.0 + h.distance as f64);
                let e = item_sem.entry(h.item.id.clone()).or_insert((0.0, None));
                if s > e.0 {
                    *e = (s, e.1.clone());
                }
            }
        }
        for (id, (s, chunk)) in item_sem {
            let key = ("item".to_string(), id.clone());
            match merged.get_mut(&key) {
                Some(hit) => {
                    hit.score += s;
                    if hit.snippet.is_empty() {
                        if let Some(c) = chunk {
                            hit.snippet = c;
                        }
                    }
                }
                None => {
                    // 语义独中：回表取标题/类型
                    if let Ok((title, item_type)) = rusqlite::Connection::open(get_db_path(app))
                        .and_then(|c| {
                            c.query_row(
                                "SELECT title, item_type FROM items WHERE id = ?1 \
                                 AND datetime(COALESCE(expires_at, datetime(fetched_at, '+168 hours'))) > datetime('now')",
                                [&id],
                                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                            )
                        })
                    {
                        merged.insert(
                            key,
                            GlobalHit {
                                kind: "item".to_string(),
                                id,
                                title,
                                snippet: chunk.unwrap_or_default(),
                                score: s,
                                sub_type: Some(item_type),
                            },
                        );
                    }
                }
            }
        }

        // 笔记 chunk 级
        if let Some(hits) = crate::vector::vec_search_note_chunks(app.clone(), vec, Some(20)).data {
            let mut note_sem: HashMap<String, (f64, String, String, String)> = HashMap::new();
            for h in hits {
                let s = 1.0 / (1.0 + h.distance as f64);
                let e = note_sem.entry(h.note_id.clone()).or_insert((
                    0.0,
                    String::new(),
                    String::new(),
                    String::new(),
                ));
                if s > e.0 {
                    *e = (s, h.title, h.article_type, h.chunk_text);
                }
            }
            for (id, (s, title, article_type, chunk)) in note_sem {
                let kind = if article_type == "manual" || article_type == "journal" {
                    "note"
                } else {
                    "article"
                };
                let key = (kind.to_string(), id.clone());
                match merged.get_mut(&key) {
                    Some(hit) => hit.score += s,
                    None => {
                        merged.insert(
                            key,
                            GlobalHit {
                                kind: kind.to_string(),
                                id,
                                title,
                                snippet: chunk,
                                score: s,
                                sub_type: Some(article_type),
                            },
                        );
                    }
                }
            }
        }
    }

    // —— 3. 统一排序截断 ——
    let mut hits: Vec<GlobalHit> = merged.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}
