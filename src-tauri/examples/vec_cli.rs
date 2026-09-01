// vec_cli — 语义索引的独立验证/维护工具（不经过 Tauri 前端，直接验证完整链路）
//
// 用法（在 src-tauri 目录下）：
//   cargo run --example vec_cli -- stats              # 索引统计
//   cargo run --example vec_cli -- index              # 为全部条目生成向量并建索引
//   cargo run --example vec_cli -- search "查询词"     # 语义搜索 top 10
//
// 嵌入配置读取 settings.ai_config，API Key 读取 macOS 钥匙串（com.fei.sophonote / embedding）。

use rusqlite::Connection;
use serde::Deserialize;
use std::path::PathBuf;

const KEYCHAIN_SERVICE: &str = "com.fei.sophonote";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiConfig {
    embedding: Option<EmbeddingCfg>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingCfg {
    base_url: String,
    model: String,
    protocol: Option<String>,
}

fn db_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home).join("Library/Application Support/com.fei.sophonote/sophonote.db")
}

fn register_vec() {
    unsafe {
        // AG-23：transmute 显式标注（与 src/vector.rs 同口径；
        // AG-24：c_char 而非 i8——aarch64 上 c_char=u8，硬编码 i8 编不过；
        // 类型路径走 rusqlite::ffi = libsqlite3_sys re-export）
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

fn load_embedding_cfg(conn: &Connection) -> Result<EmbeddingCfg, String> {
    let raw: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ai_config'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("读取 ai_config 失败: {}", e))?;
    let cfg: AiConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    cfg.embedding
        .ok_or_else(|| "ai_config 中未配置 embedding".to_string())
}

async fn embed(
    client: &reqwest::Client,
    cfg: &EmbeddingCfg,
    key: &str,
    text: &str,
) -> Result<Vec<f32>, String> {
    let input: String = text.chars().take(8000).collect();
    let is_dashscope = cfg.protocol.as_deref() == Some("dashscope");
    let url = if is_dashscope {
        cfg.base_url.clone()
    } else {
        format!("{}/embeddings", cfg.base_url.trim_end_matches('/'))
    };
    let body = if is_dashscope {
        serde_json::json!({ "model": cfg.model, "input": { "texts": [input] } })
    } else {
        serde_json::json!({ "model": cfg.model, "input": input })
    };
    let resp: serde_json::Value = client
        .post(&url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let vec: Option<Vec<f32>> = if is_dashscope {
        resp["output"]["embeddings"][0]["embedding"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect()
            })
    } else {
        resp["data"][0]["embedding"].as_array().map(|a| {
            a.iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect()
        })
    };
    vec.filter(|v| !v.is_empty()).ok_or_else(|| {
        format!(
            "嵌入接口返回异常: {}",
            &resp.to_string()[..resp.to_string().len().min(200)]
        )
    })
}

fn get_api_key() -> Result<String, String> {
    // 优先环境变量；其次 settings 表（开发期主存储）；兜底钥匙串（历史数据）
    if let Ok(k) = std::env::var("MB_EMBED_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    if let Ok(conn) = Connection::open(db_path()) {
        if let Ok(k) = conn.query_row(
            "SELECT value FROM settings WHERE key = 'apikey:embedding'",
            [],
            |r| r.get::<_, String>(0),
        ) {
            if !k.is_empty() {
                return Ok(k);
            }
        }
    }
    keyring::Entry::new(KEYCHAIN_SERVICE, "embedding")
        .and_then(|e| e.get_password())
        .map_err(|e| format!("未找到 embedding API Key: {}", e))
}

#[tokio::main]
async fn main() {
    register_vec();
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("stats");

    let conn = Connection::open(db_path()).expect("无法打开数据库");

    match cmd {
        "stats" => {
            let total: i64 = conn
                .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
                .unwrap_or(0);
            let dim: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key='vec_dimension'",
                    [],
                    |r| r.get(0),
                )
                .ok();
            let indexed: i64 = if vec_table_exists(&conn) {
                conn.query_row("SELECT COUNT(*) FROM vec_items", [], |r| r.get(0))
                    .unwrap_or(0)
            } else {
                0
            };
            println!(
                "条目总数: {} | 已索引: {} | 维度: {}",
                total,
                indexed,
                dim.as_deref().unwrap_or("未建表")
            );
        }
        "index" => {
            let cfg = load_embedding_cfg(&conn).expect("嵌入配置缺失");
            let key = get_api_key().expect("获取 API Key 失败");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap();

            let indexed_ids: std::collections::HashSet<String> = if vec_table_exists(&conn) {
                let mut stmt = conn.prepare("SELECT item_id FROM vec_items").unwrap();
                stmt.query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                Default::default()
            };

            let mut stmt = conn
                .prepare("SELECT id, title, COALESCE(description,''), COALESCE(ai_summary,'') FROM items")
                .unwrap();
            let items: Vec<(String, String)> = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        format!(
                            "{}\n{}\n{}",
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?
                        ),
                    ))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .filter(|(id, _)| !indexed_ids.contains(id))
                .collect();

            println!(
                "待索引 {} 条（已跳过 {} 条）",
                items.len(),
                indexed_ids.len()
            );
            let mut done = 0usize;
            let mut failed = 0usize;
            for (id, text) in &items {
                match embed(&client, &cfg, &key, text).await {
                    Ok(v) => {
                        if let Err(e) = ensure_vec_table(&conn, v.len()) {
                            eprintln!("建表失败: {}", e);
                            break;
                        }
                        match conn.execute(
                            "INSERT OR REPLACE INTO vec_items (item_id, embedding) VALUES (?1, ?2)",
                            rusqlite::params![id, vector_to_json(&v)],
                        ) {
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!("写入失败 {}: {}", id, e);
                                failed += 1;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("嵌入失败 {}: {}", id, e);
                        failed += 1;
                    }
                }
                done += 1;
                if done.is_multiple_of(20) || done == items.len() {
                    println!("进度 {}/{}（失败 {}）", done, items.len(), failed);
                }
            }
            println!(
                "✅ 索引完成：成功 {}，失败 {}",
                items.len() - failed,
                failed
            );
        }
        "search" => {
            let query = args.get(2).expect("用法: vec_cli search \"查询词\"");
            let cfg = load_embedding_cfg(&conn).expect("嵌入配置缺失");
            let key = get_api_key().expect("获取 API Key 失败");
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap();

            let v = embed(&client, &cfg, &key, query)
                .await
                .expect("查询嵌入失败");
            if !vec_table_exists(&conn) {
                println!("索引尚未建立，先运行 index");
                return;
            }
            let mut stmt = conn
                .prepare(
                    "SELECT v.item_id, v.distance, i.title, i.item_type FROM vec_items v \
                     JOIN items i ON i.id = v.item_id \
                     WHERE v.embedding MATCH ?1 AND k = 10 ORDER BY v.distance",
                )
                .unwrap();
            let hits: Vec<(String, f32, String, String)> = stmt
                .query_map(rusqlite::params![vector_to_json(&v)], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            println!("查询「{}」Top {} 结果：", query, hits.len());
            for (i, (_, dist, title, ty)) in hits.iter().enumerate() {
                println!("{:>2}. [{:.3}] [{}] {}", i + 1, dist, ty, title);
            }
        }
        other => {
            eprintln!(
                "未知命令: {}。可用: stats | index | search \"查询词\"",
                other
            );
        }
    }
}
