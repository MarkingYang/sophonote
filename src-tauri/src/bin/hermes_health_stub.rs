//! H2/H3/H4 协议 stub：Hermes API Server health + Runs API + SSE id/续传。
//! 不是生产 Hermes Runtime。
//!
//! 环境变量：`API_SERVER_HOST` / `API_SERVER_PORT` / `API_SERVER_KEY`
//! H4：`HERMES_STUB_DROP_AFTER` = 本连接发送 N 条事件后强制断流（测重连）

use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const WRITE_TOOLS: &[&str] = &["create_document", "propose_document_patch", "move_document"];

#[derive(Clone)]
struct AppState {
    api_key: String,
    runs: Arc<Mutex<HashMap<String, RunState>>>,
}

struct RunState {
    status: String,
    #[allow(dead_code)]
    input: String,
    context_pack: Option<serde_json::Value>,
    events: Vec<serde_json::Value>,
    /// 脚本阶段：0=await list result, 1=await read result, 2=done
    phase: u8,
    pending_call_id: Option<String>,
    list_result_text: String,
    article_id_hint: Option<String>,
    final_answer: String,
    stop_requested: bool,
}

struct HttpRequest {
    method: String,
    path: String,
    auth: Option<String>,
    body: String,
    last_event_id: Option<String>,
}

fn main() {
    let host = env::var("API_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var("API_SERVER_PORT").unwrap_or_else(|_| {
        eprintln!("hermes_health_stub: API_SERVER_PORT is required");
        std::process::exit(2);
    });
    let key = env::var("API_SERVER_KEY").unwrap_or_else(|_| {
        eprintln!("hermes_health_stub: API_SERVER_KEY is required");
        std::process::exit(2);
    });

    let state = AppState {
        api_key: key,
        runs: Arc::new(Mutex::new(HashMap::new())),
    };

    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        eprintln!("hermes_health_stub: bind {addr} failed: {e}");
        std::process::exit(1);
    });
    eprintln!("hermes_health_stub listening on http://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream, &state) {
                        eprintln!("hermes_health_stub: request error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("hermes_health_stub: accept error: {e}"),
        }
    }
}

fn handle_client(mut stream: TcpStream, state: &AppState) -> std::io::Result<()> {
    let HttpRequest {
        method,
        path,
        auth,
        body,
        last_event_id,
    } = read_http_request(&mut stream)?;

    let need_auth = path != "/health" && path != "/v1/health";
    if need_auth {
        let expected = format!("Bearer {}", state.api_key);
        match auth.as_deref() {
            Some(a) if a == expected => {}
            _ => {
                write_response(
                    &mut stream,
                    401,
                    "application/json",
                    r#"{"error":"unauthorized"}"#,
                )?;
                let _ = stream.shutdown(Shutdown::Both);
                return Ok(());
            }
        }
    }

    if method == "GET" && (path == "/health" || path == "/v1/health") {
        write_response(&mut stream, 200, "application/json", r#"{"status":"ok"}"#)?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }

    if method == "GET" && path == "/health/detailed" {
        write_response(
            &mut stream,
            200,
            "application/json",
            r#"{"status":"ok","readiness":{"checks":[]}}"#,
        )?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }

    if method == "POST" && path == "/v1/runs" {
        return handle_create_run(&mut stream, state, &body);
    }

    if method == "GET" {
        if let Some(id) = path.strip_prefix("/v1/runs/") {
            if let Some(run_id) = id.strip_suffix("/events") {
                return handle_sse(&mut stream, state, run_id, last_event_id.as_deref());
            }
            if !id.contains('/') {
                return handle_get_run(&mut stream, state, id);
            }
        }
    }

    if method == "POST" {
        if let Some(rest) = path.strip_prefix("/v1/runs/") {
            if let Some(run_id) = rest.strip_suffix("/stop") {
                return handle_stop(&mut stream, state, run_id);
            }
            if let Some(run_id) = rest.strip_suffix("/tool_results") {
                return handle_tool_result(&mut stream, state, run_id, &body);
            }
        }
    }

    write_response(
        &mut stream,
        404,
        "application/json",
        r#"{"error":"not_found"}"#,
    )?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<HttpRequest> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(buf.len());
    let header_text = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut auth = None;
    let mut content_length = 0usize;
    let mut last_event_id = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line
            .strip_prefix("Authorization:")
            .or_else(|| line.strip_prefix("authorization:"))
        {
            auth = Some(rest.trim().to_string());
        }
        if let Some(rest) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = rest.trim().parse().unwrap_or(0);
        }
        if let Some(rest) = line
            .strip_prefix("Last-Event-ID:")
            .or_else(|| line.strip_prefix("last-event-id:"))
            .or_else(|| line.strip_prefix("Last-Event-Id:"))
        {
            last_event_id = Some(rest.trim().to_string());
        }
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    let body = String::from_utf8_lossy(&body).to_string();
    Ok(HttpRequest {
        method,
        path,
        auth,
        body,
        last_event_id,
    })
}

fn handle_create_run(stream: &mut TcpStream, state: &AppState, body: &str) -> std::io::Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let input = parsed
        .get("input")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let context_pack = parsed.get("context_pack").cloned();
    let article_id_hint = parsed
        .get("article_id_hint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let run_id = format!("run-{}", simple_id());
    let call_id = "call-list-1".to_string();
    let mut events = Vec::new();
    events.push(serde_json::json!({
        "type": "run.started",
        "run_id": run_id,
        "input": input,
    }));
    events.push(serde_json::json!({
        "type": "tool.started",
        "call_id": call_id,
        "name": "list_project_documents",
        "arguments": {},
    }));

    let run = RunState {
        status: "running".into(),
        input,
        context_pack,
        events,
        phase: 0,
        pending_call_id: Some(call_id),
        list_result_text: String::new(),
        article_id_hint,
        final_answer: String::new(),
        stop_requested: false,
    };
    state.runs.lock().unwrap().insert(run_id.clone(), run);

    let resp = serde_json::json!({"run_id": run_id, "status": "started"});
    write_response(stream, 200, "application/json", &resp.to_string())?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn handle_get_run(stream: &mut TcpStream, state: &AppState, run_id: &str) -> std::io::Result<()> {
    let runs = state.runs.lock().unwrap();
    let Some(run) = runs.get(run_id) else {
        write_response(stream, 404, "application/json", r#"{"error":"not_found"}"#)?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    };
    let resp = serde_json::json!({
        "object": "hermes.run",
        "run_id": run_id,
        "status": run.status,
        "output": run.final_answer,
    });
    write_response(stream, 200, "application/json", &resp.to_string())?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn handle_stop(stream: &mut TcpStream, state: &AppState, run_id: &str) -> std::io::Result<()> {
    let mut runs = state.runs.lock().unwrap();
    let Some(run) = runs.get_mut(run_id) else {
        write_response(stream, 404, "application/json", r#"{"error":"not_found"}"#)?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    };
    run.stop_requested = true;
    if run.status == "running" {
        run.status = "cancelled".into();
        run.events.push(serde_json::json!({
            "type": "run.cancelled",
            "reason": "stop_requested",
        }));
        run.pending_call_id = None;
        run.phase = 2;
    }
    write_response(stream, 200, "application/json", r#"{"status":"stopping"}"#)?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn handle_tool_result(
    stream: &mut TcpStream,
    state: &AppState,
    run_id: &str,
    body: &str,
) -> std::io::Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::json!({}));
    let call_id = parsed.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
    let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let output_text = parsed
        .get("output_text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if WRITE_TOOLS.contains(&name) {
        write_response(
            stream,
            400,
            "application/json",
            r#"{"error":"write_tool_forbidden"}"#,
        )?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }

    let mut runs = state.runs.lock().unwrap();
    let Some(run) = runs.get_mut(run_id) else {
        write_response(stream, 404, "application/json", r#"{"error":"not_found"}"#)?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    };

    if run.pending_call_id.as_deref() != Some(call_id) {
        write_response(
            stream,
            409,
            "application/json",
            r#"{"error":"unexpected_call_id"}"#,
        )?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }

    run.events.push(serde_json::json!({
        "type": "tool.completed",
        "call_id": call_id,
        "name": name,
        "ok": ok,
        "output_text": output_text,
    }));
    run.pending_call_id = None;

    if run.stop_requested {
        run.status = "cancelled".into();
        run.events.push(serde_json::json!({
            "type": "run.cancelled",
            "reason": "stop_requested",
        }));
        run.phase = 2;
        write_response(stream, 200, "application/json", r#"{"status":"ok"}"#)?;
        let _ = stream.shutdown(Shutdown::Both);
        return Ok(());
    }

    if run.phase == 0 {
        run.list_result_text = output_text.clone();
        let article_id = run
            .article_id_hint
            .clone()
            .or_else(|| extract_article_id(&output_text))
            .unwrap_or_else(|| "missing".into());
        let call2 = "call-read-1".to_string();
        run.events.push(serde_json::json!({
            "type": "tool.started",
            "call_id": call2,
            "name": "read_document",
            "arguments": {"articleId": article_id},
        }));
        run.pending_call_id = Some(call2);
        run.phase = 1;
    } else if run.phase == 1 {
        let mut answer = format!(
            "基于只读工具完成：list 后 read。\nlist摘要：{}\nread摘要：{}",
            truncate(&run.list_result_text, 200),
            truncate(&output_text, 400)
        );
        if let Some(pack) = &run.context_pack {
            answer.push_str("\n[context_pack]");
            answer.push_str(&pack.to_string());
            answer.push_str("[/context_pack]");
        }
        run.final_answer = answer.clone();
        // H4：拆成 message.delta + message.completed
        let (d1, d2) = split_two(&answer);
        run.events.push(serde_json::json!({
            "type": "message.delta",
            "text": d1,
            "index": 0,
        }));
        run.events.push(serde_json::json!({
            "type": "message.delta",
            "text": d2,
            "index": 1,
        }));
        run.events.push(serde_json::json!({
            "type": "message.completed",
            "text": answer,
        }));
        run.events.push(serde_json::json!({
            "type": "run.completed",
            "outcome": "completed",
            "final_answer": run.final_answer,
        }));
        run.status = "completed".into();
        run.phase = 2;
    }

    write_response(stream, 200, "application/json", r#"{"status":"ok"}"#)?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn handle_sse(
    stream: &mut TcpStream,
    state: &AppState,
    run_id: &str,
    last_event_id: Option<&str>,
) -> std::io::Result<()> {
    {
        let runs = state.runs.lock().unwrap();
        if !runs.contains_key(run_id) {
            write_response(stream, 404, "application/json", r#"{"error":"not_found"}"#)?;
            let _ = stream.shutdown(Shutdown::Both);
            return Ok(());
        }
    }

    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
    stream.write_all(header.as_bytes())?;
    stream.flush()?;

    let mut next_id = last_event_id
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.saturating_add(1))
        .unwrap_or(0);
    let drop_after = env::var("HERMES_STUB_DROP_AFTER")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let mut sent_this_conn = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (chunk, terminal) = {
            let runs = state.runs.lock().unwrap();
            let Some(run) = runs.get(run_id) else {
                break;
            };
            if next_id > run.events.len() {
                next_id = run.events.len();
            }
            let slice = run.events[next_id..].to_vec();
            let terminal = run.phase >= 2
                || run.status == "completed"
                || run.status == "cancelled"
                || run.status == "failed";
            (slice, terminal)
        };
        let idle_close = chunk.is_empty() && env::var("HERMES_STUB_IDLE_CLOSE").is_ok();
        for ev in chunk {
            let frame = format!("id: {next_id}\ndata: {ev}\n\n");
            stream.write_all(frame.as_bytes())?;
            stream.flush()?;
            next_id += 1;
            sent_this_conn += 1;
            if let Some(limit) = drop_after {
                if sent_this_conn >= limit {
                    // 强制断流：不发终态，供客户端 Last-Event-ID 重连
                    let _ = stream.shutdown(Shutdown::Both);
                    return Ok(());
                }
            }
        }
        if terminal {
            break;
        }
        // H4：无新事件时立即断流（测对账 interrupted；默认仍等 deadline）
        if idle_close {
            let _ = stream.shutdown(Shutdown::Both);
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            let id = next_id;
            let data = format!(
                "id: {id}\ndata: {{\"type\":\"run.failed\",\"outcome\":\"timeout\",\"error\":\"sse_timeout\"}}\n\n"
            );
            stream.write_all(data.as_bytes())?;
            stream.flush()?;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

fn split_two(s: &str) -> (String, String) {
    let mid = s.chars().count() / 2;
    let mut first = String::new();
    let mut second = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i < mid {
            first.push(ch);
        } else {
            second.push(ch);
        }
    }
    if first.is_empty() {
        (second, String::new())
    } else {
        (first, second)
    }
}

fn extract_article_id(text: &str) -> Option<String> {
    for part in text.split("articleId:") {
        let id = part
            .split_whitespace()
            .next()?
            .trim_matches(|c| c == '）' || c == ')' || c == ',' || c == '。' || c == '\n');
        if id.len() >= 8 {
            return Some(id.to_string());
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

fn simple_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{n:x}")
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}
