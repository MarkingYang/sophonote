//! Read-only usage adapters. Native ledgers remain the source of truth.
use crate::{commands::ApiResponse, storage_layout::StorageLayout};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Numbers {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    total_tokens: u64,
    sessions: u64,
    api_calls: u64,
    estimated_cost: Option<f64>,
    partial_cost: bool,
}
impl Numbers {
    fn empty() -> Self {
        Self {
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
            reasoning_tokens: Some(0),
            ..Self::default()
        }
    }
    fn add(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        self.api_calls += other.api_calls;
        self.sessions += other.sessions;
        self.cache_read_tokens = self
            .cache_read_tokens
            .zip(other.cache_read_tokens)
            .map(|(a, b)| a + b);
        self.cache_write_tokens = self
            .cache_write_tokens
            .zip(other.cache_write_tokens)
            .map(|(a, b)| a + b);
        self.reasoning_tokens = self
            .reasoning_tokens
            .zip(other.reasoning_tokens)
            .map(|(a, b)| a + b);
        self.partial_cost |=
            other.partial_cost || (other.api_calls > 0 && other.estimated_cost.is_none());
        if let Some(cost) = other.estimated_cost {
            *self.estimated_cost.get_or_insert(0.0) += cost;
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    day: String,
    #[serde(flatten)]
    numbers: Numbers,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    model: String,
    #[serde(flatten)]
    numbers: Numbers,
    daily: Option<Vec<Day>>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    engine: String,
    summary: Numbers,
    daily: Vec<Day>,
    by_model: Vec<Model>,
    error: Option<String>,
}
impl Source {
    fn empty(engine: &str) -> Self {
        Self {
            engine: engine.into(),
            summary: Numbers::empty(),
            daily: vec![],
            by_model: vec![],
            error: None,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    sources: Vec<Source>,
    period_days: u16,
}

fn cost(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && *n > 0.0)
}
fn timestamp(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| {
        DateTime::parse_from_rfc3339(v.as_str()?)
            .ok()
            .map(|d| d.timestamp_millis())
    })
}
#[derive(Debug)]
struct Call {
    session: String,
    id: String,
    model: String,
    at: i64,
    numbers: Numbers,
}
fn json_call(engine: &str, v: &Value, session: &str) -> Option<Call> {
    let m = &v["message"];
    if m["role"] != "assistant" {
        return None;
    }
    let u = &m["usage"];
    if !u.is_object() {
        return None;
    }
    let pi = engine == "pi";
    let input = u[if pi { "input" } else { "input_tokens" }].as_u64()?;
    let output = u[if pi { "output" } else { "output_tokens" }].as_u64()?;
    let cache = u[if pi {
        "cacheRead"
    } else {
        "cache_read_input_tokens"
    }]
    .as_u64();
    let write = u[if pi {
        "cacheWrite"
    } else {
        "cache_creation_input_tokens"
    }]
    .as_u64();
    let reasoning = if pi {
        u["reasoning"].as_u64()
    } else {
        u["output_tokens_details"]["thinking_tokens"]
            .as_u64()
            .or_else(|| u["output_tokens_details"]["reasoning_tokens"].as_u64())
    };
    let id = if pi {
        v["id"].as_str()
    } else {
        m["id"].as_str()
    }?;
    let at = timestamp(&v["timestamp"]).or_else(|| timestamp(&m["timestamp"]))?;
    Some(Call {
        session: session.into(),
        id: id.into(),
        model: m["model"].as_str().unwrap_or("未知模型").into(),
        at,
        numbers: Numbers {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache,
            cache_write_tokens: write,
            reasoning_tokens: reasoning,
            total_tokens: if pi { u["totalTokens"].as_u64() } else { None }
                .unwrap_or(input + output + cache.unwrap_or(0) + write.unwrap_or(0)),
            sessions: 0,
            api_calls: 1,
            estimated_cost: if pi { cost(&u["cost"]["total"]) } else { None },
            partial_cost: false,
        },
    })
}
fn collect_files(root: &Path, depth: usize, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if !root.exists() || fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(());
    }
    if depth > 8 {
        return Err("会话目录层级超出读取范围".into());
    }
    for entry in fs::read_dir(root).map_err(|_| "无法读取会话目录")? {
        let entry = entry.map_err(|_| "无法读取会话文件")?;
        let kind = entry.file_type().map_err(|_| "无法读取会话文件类型")?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(&entry.path(), depth + 1, files)?;
        } else if kind.is_file()
            && (entry.path().extension().is_some_and(|e| e == "jsonl")
                || entry.file_name() == "opencode.db")
        {
            files.push(entry.path());
            if files.len() > 20_000 {
                return Err("会话文件过多，请缩小本地历史范围".into());
            }
        }
    }
    Ok(())
}
fn read_jsonl(
    path: &Path,
    engine: &str,
    calls: &mut HashMap<(String, String), Call>,
) -> Result<(), String> {
    let file = fs::File::open(path).map_err(|_| "无法读取用量会话")?;
    let session = path.to_string_lossy();
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    let mut invalid = false;
    loop {
        bytes.clear();
        // Bound each frame; do not allocate unbounded transcript content.
        let n = reader
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|_| "读取用量会话失败")?;
        if n == 0 {
            break;
        }
        if n > 8 * 1024 * 1024 {
            return Err("会话记录过大，部分用量未读取".into());
        }
        if bytes.last() != Some(&b'\n') {
            break;
        } // live writer's unfinished tail
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => {
                if let Some(call) = json_call(engine, &v, &session) {
                    insert_call(calls, call);
                } else if v["message"]["role"] == "assistant"
                    && v["message"]["model"] != "<synthetic>"
                {
                    invalid = true;
                }
            }
            Err(_) => invalid = true,
        }
    }
    if invalid {
        Err("部分会话记录损坏，统计可能不完整".into())
    } else {
        Ok(())
    }
}
fn insert_call(calls: &mut HashMap<(String, String), Call>, call: Call) {
    let key = (call.session.clone(), call.id.clone());
    // Claude writes multiple content blocks for one API message. Usage is cumulative.
    // Retain the largest finalized snapshot, never sum those blocks.
    match calls.get(&key) {
        Some(old) if old.numbers.total_tokens > call.numbers.total_tokens => {}
        _ => {
            calls.insert(key, call);
        }
    }
}
fn read_opencode(
    path: &Path,
    calls: &mut HashMap<(String, String), Call>,
    cutoff: i64,
) -> Result<(), String> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "无法读取 OpenCode 用量账本")?;
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|_| "OpenCode 账本忙碌")?;
    let mut stmt = conn.prepare("SELECT p.id, p.session_id, p.time_created, p.data, m.data FROM part p JOIN message m ON m.id = p.message_id WHERE p.time_created > ?1 AND json_extract(p.data, '$.type') = 'step-finish'").map_err(|_|"OpenCode 用量账本格式不支持")?;
    let rows = stmt
        .query_map([cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|_| "OpenCode 用量查询失败")?;
    for row in rows {
        let (id, session, at, part, message) = row.map_err(|_| "OpenCode 用量记录无效")?;
        let p: Value = serde_json::from_str(&part).map_err(|_| "OpenCode 用量记录损坏")?;
        let m: Value = serde_json::from_str(&message).map_err(|_| "OpenCode 模型记录损坏")?;
        let t = &p["tokens"];
        let input = t["input"].as_u64().ok_or("OpenCode 缺少输入 Token")?;
        let reasoning = t["reasoning"].as_u64();
        // OpenCode's output bucket excludes reasoning; normalize to inclusive output.
        let output =
            t["output"].as_u64().ok_or("OpenCode 缺少输出 Token")? + reasoning.unwrap_or(0);
        let cache = t["cache"]["read"].as_u64();
        let write = t["cache"]["write"].as_u64();
        insert_call(
            calls,
            Call {
                session: format!("{}:{session}", path.display()),
                id,
                model: m["modelID"].as_str().unwrap_or("未知模型").into(),
                at,
                numbers: Numbers {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache,
                    cache_write_tokens: write,
                    reasoning_tokens: reasoning,
                    total_tokens: t["total"]
                        .as_u64()
                        .unwrap_or(input + output + cache.unwrap_or(0) + write.unwrap_or(0)),
                    api_calls: 1,
                    sessions: 0,
                    estimated_cost: cost(&p["cost"]),
                    partial_cost: false,
                },
            },
        );
    }
    Ok(())
}
#[derive(Default)]
struct Bucket {
    numbers: Option<Numbers>,
    sessions: HashSet<String>,
}
impl Bucket {
    fn push(&mut self, call: &Call) {
        self.numbers
            .get_or_insert_with(Numbers::empty)
            .add(&call.numbers);
        self.sessions.insert(call.session.clone());
    }
    fn finish(self) -> Numbers {
        let mut n = self.numbers.unwrap_or_else(Numbers::empty);
        n.sessions = self.sessions.len() as u64;
        n
    }
}
fn aggregate(engine: &str, calls: impl Iterator<Item = Call>, cutoff: i64, now: i64) -> Source {
    let mut all = Bucket::default();
    let mut daily: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut models: BTreeMap<String, (Bucket, BTreeMap<String, Bucket>)> = BTreeMap::new();
    for call in calls.filter(|c| c.at > cutoff && c.at <= now) {
        let Some(date) = DateTime::from_timestamp_millis(call.at) else {
            continue;
        };
        let day = date.format("%Y-%m-%d").to_string();
        all.push(&call);
        daily.entry(day.clone()).or_default().push(&call);
        let model = models.entry(call.model.clone()).or_default();
        model.0.push(&call);
        model.1.entry(day).or_default().push(&call);
    }
    let days = |rows: BTreeMap<String, Bucket>| {
        rows.into_iter()
            .map(|(day, b)| Day {
                day,
                numbers: b.finish(),
            })
            .collect()
    };
    Source {
        engine: engine.into(),
        summary: all.finish(),
        daily: days(daily),
        by_model: models
            .into_iter()
            .map(|(model, (b, d))| Model {
                model,
                numbers: b.finish(),
                daily: Some(days(d)),
            })
            .collect(),
        error: None,
    }
}
pub(super) fn native_source(root: &Path, engine: &str, days: u16, now: i64) -> Source {
    let mut files = vec![];
    let sessions = root.join(engine).join("sessions");
    if sessions.exists()
        && !root
            .canonicalize()
            .ok()
            .zip(sessions.canonicalize().ok())
            .is_some_and(|(base, path)| path.starts_with(base))
    {
        let mut source = Source::empty(engine);
        source.error = Some("会话目录位于应用数据范围之外，未读取".into());
        return source;
    }
    let mut error = collect_files(&sessions, 0, &mut files).err();
    let cutoff = now - i64::from(days) * 86_400_000;
    let mut calls = HashMap::new();
    let mut scanned = 0u64;
    for path in files {
        scanned = scanned.saturating_add(fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
        if scanned > 512 * 1024 * 1024 {
            error = Some("用量历史超过单次读取范围，统计可能不完整".into());
            break;
        }
        let result = if engine == "opencode" {
            if path.file_name().is_none_or(|n| n != "opencode.db") {
                continue;
            }
            read_opencode(&path, &mut calls, cutoff)
        } else {
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            read_jsonl(&path, engine, &mut calls)
        };
        if let Err(e) = result {
            error = Some(e);
        }
    }
    let mut source = aggregate(engine, calls.into_values(), cutoff, now);
    source.error = error;
    source
}
fn hermes_source(report: super::commands::HermesUsageReport) -> Source {
    let numbers =
        |input, output, cache: Option<u64>, reasoning, cost: f64, sessions, calls| Numbers {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache,
            cache_write_tokens: None,
            reasoning_tokens: reasoning,
            total_tokens: input + output + cache.unwrap_or(0),
            sessions,
            api_calls: calls,
            estimated_cost: if cost > 0.0 { Some(cost) } else { None },
            partial_cost: false,
        };
    let t = report.totals;
    Source {
        engine: "hermes".into(),
        summary: numbers(
            t.total_input,
            t.total_output,
            Some(t.total_cache_read),
            Some(t.total_reasoning),
            t.total_estimated_cost,
            t.total_sessions,
            t.total_api_calls,
        ),
        daily: report
            .daily
            .into_iter()
            .map(|r| Day {
                day: r.day,
                numbers: numbers(
                    r.input_tokens,
                    r.output_tokens,
                    Some(r.cache_read_tokens),
                    Some(r.reasoning_tokens),
                    r.estimated_cost,
                    r.sessions,
                    r.api_calls,
                ),
            })
            .collect(),
        by_model: report
            .by_model
            .into_iter()
            .map(|r| Model {
                model: r.model,
                numbers: numbers(
                    r.input_tokens,
                    r.output_tokens,
                    None,
                    None,
                    r.estimated_cost,
                    r.sessions,
                    r.api_calls,
                ),
                daily: None,
            })
            .collect(),
        error: None,
    }
}
#[tauri::command]
pub async fn agent_usage(app: tauri::AppHandle, days: u16) -> ApiResponse<Report> {
    if !matches!(days, 7 | 30 | 90) {
        return ApiResponse::err("仅支持近 7、30、90 天用量".into());
    }
    let layout = match StorageLayout::resolve(&app) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(e),
    };
    let native = tokio::task::spawn_blocking(move || {
        let now = Utc::now().timestamp_millis();
        ["pi", "claude_code", "opencode"]
            .map(|engine| native_source(&layout.root, engine, days, now))
    });
    let (hermes, native) = tokio::join!(super::commands::agent_hermes_usage(days), native);
    let mut h = hermes
        .data
        .map(hermes_source)
        .unwrap_or_else(|| Source::empty("hermes"));
    h.error = hermes.error;
    let mut sources = vec![h];
    match native {
        Ok(v) => sources.extend(v),
        Err(_) => {
            for engine in ["pi", "claude_code", "opencode"] {
                let mut s = Source::empty(engine);
                s.error = Some("原生用量读取失败，请刷新重试".into());
                sources.push(s);
            }
        }
    }
    ApiResponse::ok(Report {
        sources,
        period_days: days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn pi(id: &str, at: i64, model: &str) -> Value {
        json!({"id":id,"timestamp":at,"message":{"role":"assistant","model":model,"usage":{"input":100,"output":20,"cacheRead":10,"cacheWrite":5,"reasoning":8,"totalTokens":135,"cost":{"total":0.01}}}})
    }
    #[test]
    fn native_dedup_window_model_and_session_counts() {
        let mut calls = HashMap::new();
        for (id, at, model) in [
            ("a", 1001, "m1"),
            ("a", 1001, "m1"),
            ("b", 1002, "m2"),
            ("old", 1000, "m1"),
            ("future", 3000, "m1"),
        ] {
            insert_call(
                &mut calls,
                json_call("pi", &pi(id, at, model), "session").unwrap(),
            );
        }
        let r = aggregate("pi", calls.into_values(), 1000, 2000);
        assert_eq!(r.summary.api_calls, 2);
        assert_eq!(r.summary.sessions, 1);
        assert_eq!(r.summary.total_tokens, 270);
        assert_eq!(r.summary.reasoning_tokens, Some(16));
        assert_eq!(r.by_model.len(), 2);
        assert_eq!(r.daily[0].numbers.sessions, 1);
        assert_eq!(
            r.by_model[0].daily.as_ref().unwrap()[0].numbers.api_calls,
            1
        );
    }
    #[test]
    fn claude_blocks_do_not_double_count_and_cost_is_unknown() {
        let mut calls = HashMap::new();
        for output in [5, 100, 20] {
            let v = json!({"timestamp":"2026-09-22T10:00:00Z","message":{"id":"msg1","role":"assistant","model":"model","usage":{"input_tokens":10,"output_tokens":output,"cache_read_input_tokens":80,"cache_creation_input_tokens":20,"output_tokens_details":{"thinking_tokens":4}}}});
            insert_call(&mut calls, json_call("claude_code", &v, "session").unwrap());
        }
        let r = aggregate("claude_code", calls.into_values(), 0, i64::MAX);
        assert_eq!(r.summary.total_tokens, 210);
        assert_eq!(r.summary.api_calls, 1);
        assert_eq!(r.summary.estimated_cost, None);
        assert!(r.summary.partial_cost);
        assert_eq!(r.summary.reasoning_tokens, Some(4));
    }
    #[test]
    fn missing_fields_are_unknown_and_zero_price_is_not_free() {
        let mut v = pi("x", 1001, "m");
        v["message"]["usage"]["cost"]["total"] = json!(0);
        v["message"]["usage"]
            .as_object_mut()
            .unwrap()
            .remove("reasoning");
        let c = json_call("pi", &v, "s").unwrap();
        assert_eq!(c.numbers.reasoning_tokens, None);
        assert_eq!(c.numbers.estimated_cost, None);
        let r = aggregate("pi", [c].into_iter(), 0, 2000);
        assert!(r.summary.partial_cost);
    }
    fn fixture_dir() -> PathBuf {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("usage-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn transcript_damage_is_partial_and_unfinished_tail_is_safe() {
        let root = fixture_dir();
        let dir = root.join("pi/sessions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("one.jsonl"),
            format!("{}\ninvalid\n{{", pi("a", 1001, "m")),
        )
        .unwrap();
        let r = native_source(&root, "pi", 7, 2000);
        assert_eq!(r.summary.api_calls, 1);
        assert!(r.error.is_some());
        fs::write(dir.join("one.jsonl"), format!("{}\n{{", pi("a", 1001, "m"))).unwrap();
        assert!(native_source(&root, "pi", 7, 2000).error.is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn opencode_native_steps_use_model_and_inclusive_output_without_duplicates() {
        let root = fixture_dir();
        let path = root.join("opencode.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,data TEXT); CREATE TABLE part(id TEXT PRIMARY KEY,session_id TEXT,message_id TEXT,time_created INTEGER,data TEXT);").unwrap();
        db.execute(
            "INSERT INTO message VALUES ('m',?1)",
            [json!({"modelID":"deepseek","role":"assistant"}).to_string()],
        )
        .unwrap();
        let p = json!({"type":"step-finish","tokens":{"input":20,"output":10,"reasoning":5,"cache":{"read":30,"write":40}},"cost":0.001});
        db.execute(
            "INSERT INTO part VALUES ('p','s','m',1001,?1)",
            [p.to_string()],
        )
        .unwrap();
        let mut calls = HashMap::new();
        read_opencode(&path, &mut calls, 0).unwrap();
        read_opencode(&path, &mut calls, 0).unwrap();
        let r = aggregate("opencode", calls.into_values(), 1000, 2000);
        assert_eq!(r.summary.api_calls, 1);
        assert_eq!(r.summary.total_tokens, 105);
        assert_eq!(r.summary.output_tokens, 15);
        assert_eq!(r.by_model[0].model, "deepseek");
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn hermes_reasoning_is_included_in_output_and_model_daily_is_unavailable() {
        let r = hermes_source(super::super::commands::HermesUsageReport {
            totals: super::super::commands::HermesUsageTotals {
                total_input: 100,
                total_output: 20,
                total_cache_read: 30,
                total_reasoning: 10,
                ..Default::default()
            },
            by_model: vec![super::super::commands::HermesUsageModel {
                model: "m".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        assert_eq!(r.summary.total_tokens, 150);
        assert_eq!(r.summary.cache_write_tokens, None);
        assert!(r.by_model[0].daily.is_none());
        let json = serde_json::to_value(r).unwrap();
        assert_eq!(json["summary"]["totalTokens"], 150);
        assert!(json["byModel"][0]["cacheReadTokens"].is_null());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_do_not_import_personal_history() {
        let root = fixture_dir();
        let sessions = root.join("pi/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let outside = root.join("personal.jsonl");
        fs::write(&outside, format!("{}\n", pi("a", 1001, "m"))).unwrap();
        std::os::unix::fs::symlink(outside, sessions.join("linked.jsonl")).unwrap();
        assert_eq!(native_source(&root, "pi", 7, 2000).summary.api_calls, 0);
        fs::remove_dir_all(root).unwrap();
    }
}
