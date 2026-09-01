//! OpenRouter 官方模型榜数据面（DEC-023 / NEXT-052）。
//!
//! Rust 独占 Keychain、第三方 HTTP 与 SQLite；Hermes Skill 只能经受限 Bridge
//! 触发完整刷新，React 只能读取已验证快照。

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::ApiResponse;

pub const KEYCHAIN_PROVIDER: &str = "openrouter-rankings";
const SOURCE_URL: &str = "https://openrouter.ai/rankings";
const API_BASE: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterRankingSnapshot {
    pub as_of: String,
    pub fetched_at: String,
    pub citation: String,
    pub source_url: String,
    pub models: Value,
    pub rankings_daily: Value,
    pub task_classifications: Value,
    pub session_cost: Value,
    pub benchmarks: Value,
}

fn payload_data(payload: &Value, label: &str) -> Result<Value, String> {
    payload
        .get("data")
        .cloned()
        .filter(|value| value.is_array() || value.is_object())
        .ok_or_else(|| format!("OpenRouter {label} 响应缺少 data"))
}

fn payload_as_of(payloads: &[&Value]) -> String {
    payloads
        .iter()
        .filter_map(|payload| {
            payload
                .pointer("/meta/as_of")
                .or_else(|| payload.pointer("/data/as_of"))
                .and_then(Value::as_str)
        })
        .min()
        .map(str::to_string)
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
}

async fn fetch_json(
    client: &reqwest::Client,
    key: &str,
    path: &str,
    label: &str,
) -> Result<Value, String> {
    let response = client
        .get(format!("{API_BASE}{path}"))
        .bearer_auth(key)
        .header("HTTP-Referer", "https://sophonote.local")
        .header("X-OpenRouter-Title", "SophoNote")
        .send()
        .await
        .map_err(|error| format!("OpenRouter {label} 请求失败: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => "OpenRouter API Key 无效或无权读取排名数据".into(),
            429 => "OpenRouter 排名 API 已限流，请稍后再试".into(),
            code => format!("OpenRouter {label} 返回 HTTP {code}"),
        });
    }
    let payload = response
        .json::<Value>()
        .await
        .map_err(|error| format!("OpenRouter {label} 返回无效 JSON: {error}"))?;
    payload_data(&payload, label)?;
    Ok(payload)
}

fn save_snapshot(conn: &Connection, snapshot: &OpenRouterRankingSnapshot) -> Result<(), String> {
    let payload = json!({
        "sourceUrl": snapshot.source_url,
        "models": snapshot.models,
        "rankingsDaily": snapshot.rankings_daily,
        "taskClassifications": snapshot.task_classifications,
        "sessionCost": snapshot.session_cost,
        "benchmarks": snapshot.benchmarks,
    });
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("开始保存 OpenRouter 快照失败: {error}"))?;
    tx.execute("DELETE FROM openrouter_ranking_snapshots", [])
        .map_err(|error| format!("替换 OpenRouter 快照失败: {error}"))?;
    tx.execute(
        "INSERT INTO openrouter_ranking_snapshots (id, as_of, fetched_at, citation, payload_json) \
         VALUES ('latest', ?1, ?2, ?3, ?4)",
        rusqlite::params![
            snapshot.as_of,
            snapshot.fetched_at,
            snapshot.citation,
            serde_json::to_string(&payload).map_err(|error| error.to_string())?,
        ],
    )
    .map_err(|error| format!("保存 OpenRouter 快照失败: {error}"))?;
    tx.commit()
        .map_err(|error| format!("提交 OpenRouter 快照失败: {error}"))
}

pub fn read_snapshot(conn: &Connection) -> Result<Option<OpenRouterRankingSnapshot>, String> {
    let row = conn.query_row(
        "SELECT as_of, fetched_at, citation, payload_json FROM openrouter_ranking_snapshots \
         WHERE id = 'latest' LIMIT 1",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    );
    let (as_of, fetched_at, citation, raw) = match row {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(format!("读取 OpenRouter 快照失败: {error}")),
    };
    let payload: Value =
        serde_json::from_str(&raw).map_err(|error| format!("OpenRouter 快照已损坏: {error}"))?;
    Ok(Some(OpenRouterRankingSnapshot {
        as_of,
        fetched_at,
        citation,
        source_url: payload
            .get("sourceUrl")
            .and_then(Value::as_str)
            .unwrap_or(SOURCE_URL)
            .to_string(),
        models: payload.get("models").cloned().unwrap_or(Value::Null),
        rankings_daily: payload.get("rankingsDaily").cloned().unwrap_or(Value::Null),
        task_classifications: payload
            .get("taskClassifications")
            .cloned()
            .unwrap_or(Value::Null),
        session_cost: payload.get("sessionCost").cloned().unwrap_or(Value::Null),
        benchmarks: payload.get("benchmarks").cloned().unwrap_or(Value::Null),
    }))
}

pub async fn refresh_snapshot(app: &AppHandle) -> Result<OpenRouterRankingSnapshot, String> {
    let key = crate::commands::get_cached_api_key(app, KEYCHAIN_PROVIDER)?;
    if key.trim().is_empty() {
        return Err("尚未配置 OpenRouter 模型榜 API Key".into());
    }
    let client = reqwest::Client::builder()
        .user_agent("SophoNote/0.1 OpenRouter Rankings")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("创建 OpenRouter 客户端失败: {error}"))?;
    let (models, rankings, tasks, costs, benchmarks) = tokio::join!(
        fetch_json(&client, &key, "/models?sort=top-weekly", "模型目录"),
        fetch_json(
            &client,
            &key,
            "/datasets/rankings-daily?period=day",
            "使用排名"
        ),
        fetch_json(&client, &key, "/classifications/task?window=7d", "任务分类"),
        fetch_json(
            &client,
            &key,
            "/datasets/session-cost?limit=500",
            "会话成本"
        ),
        fetch_json(
            &client,
            &key,
            "/benchmarks?source=artificial-analysis",
            "基准"
        ),
    );
    let (models, rankings, tasks, costs, benchmarks) =
        (models?, rankings?, tasks?, costs?, benchmarks?);
    let as_of = payload_as_of(&[&rankings, &tasks, &costs, &benchmarks]);
    let snapshot = OpenRouterRankingSnapshot {
        citation: format!("Source: OpenRouter (openrouter.ai/rankings), as of {as_of}."),
        as_of,
        fetched_at: chrono::Utc::now().to_rfc3339(),
        source_url: SOURCE_URL.into(),
        models: payload_data(&models, "模型目录")?,
        rankings_daily: payload_data(&rankings, "使用排名")?,
        task_classifications: payload_data(&tasks, "任务分类")?,
        session_cost: payload_data(&costs, "会话成本")?,
        benchmarks: payload_data(&benchmarks, "基准")?,
    };
    let conn = Connection::open(crate::db::get_db_path(app))
        .map_err(|error| format!("打开 OpenRouter 快照数据库失败: {error}"))?;
    save_snapshot(&conn, &snapshot)?;
    Ok(snapshot)
}

#[tauri::command]
pub fn db_openrouter_rankings(app: AppHandle) -> ApiResponse<Option<OpenRouterRankingSnapshot>> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(conn) => conn,
        Err(error) => return ApiResponse::err(error.to_string()),
    };
    match read_snapshot(&conn) {
        Ok(snapshot) => ApiResponse::ok(snapshot),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn openrouter_rankings_refresh(app: AppHandle) -> ApiResponse<OpenRouterRankingSnapshot> {
    match refresh_snapshot(&app).await {
        Ok(snapshot) => ApiResponse::ok(snapshot),
        Err(error) => ApiResponse::err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE openrouter_ranking_snapshots (
                id TEXT PRIMARY KEY, as_of TEXT NOT NULL, fetched_at TEXT NOT NULL,
                citation TEXT NOT NULL, payload_json TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn snapshot_round_trip_keeps_official_sections() {
        let conn = conn();
        let snapshot = OpenRouterRankingSnapshot {
            as_of: "2026-08-17".into(),
            fetched_at: "2026-08-18T00:00:00Z".into(),
            citation: "Source: OpenRouter".into(),
            source_url: SOURCE_URL.into(),
            models: json!([{"id":"openai/gpt"}]),
            rankings_daily: json!([{"date":"2026-08-17","total_tokens":"9"}]),
            task_classifications: json!({"classifications":[]}),
            session_cost: json!([]),
            benchmarks: json!([]),
        };
        save_snapshot(&conn, &snapshot).unwrap();
        let saved = read_snapshot(&conn).unwrap().unwrap();
        assert_eq!(saved.as_of, "2026-08-17");
        assert_eq!(saved.models[0]["id"], "openai/gpt");
        assert_eq!(saved.rankings_daily[0]["total_tokens"], "9");
    }

    #[test]
    fn empty_database_has_no_snapshot() {
        assert!(read_snapshot(&conn()).unwrap().is_none());
    }
}
