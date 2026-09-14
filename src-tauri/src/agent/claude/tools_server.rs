use crate::agent::hermes::gateway_client::{register_run_control, GatewayControl};
use crate::agent::{
    events::{AgentEventPayload, EventEmitter},
    pi::tools,
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) struct Server {
    pub url: String,
    pub bearer: String,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}
struct State {
    scope: tools::Scope,
    events: Arc<EventEmitter>,
    controls: Mutex<tokio::sync::mpsc::UnboundedReceiver<GatewayControl>>,
    cancel: CancellationToken,
    bearer: String,
}

pub(super) fn definitions() -> Value {
    let mut tools = vec![
        json!({"name":"read","description":"Read a UTF-8 file in the authorized workspace or attachments (max 1 MiB).","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}),
        json!({"name":"ls","description":"List an authorized directory.","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}),
        json!({"name":"write","description":"Write a UTF-8 file through host permissions. Parent directory must exist.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}}),
        json!({"name":"edit","description":"Replace a unique exact text occurrence through host permissions.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"oldText":{"type":"string"},"newText":{"type":"string"}},"required":["path","oldText","newText"],"additionalProperties":false}}),
    ];
    if cfg!(target_os = "macos") {
        tools.push(json!({"name":"bash","description":"Run a command in the authorized workspace, with host approval and a 60-second sandbox limit.","inputSchema":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}}));
    }
    json!(tools)
}

pub(super) async fn start(
    scope: tools::Scope,
    events: Arc<EventEmitter>,
    run: &str,
    cancel: CancellationToken,
) -> Result<Server, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "无法启动 Claude Code 工具服务")?;
    let url = format!(
        "http://{}/mcp",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    let bearer = uuid::Uuid::new_v4().to_string();
    let stop = cancel.child_token();
    let state = Arc::new(State {
        scope,
        events,
        controls: Mutex::new(register_run_control(run)),
        cancel: stop.clone(),
        bearer: bearer.clone(),
    });
    let stopping = stop.clone();
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = stopping.cancelled() => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                connection = listener.accept(), if connections.len() < 16 => {
                    let Ok((stream, _)) = connection else { break; };
                    let state = state.clone();
                    connections.spawn(async move {
                        let cancel = state.cancel.clone();
                        let service = hyper::service::service_fn(move |request| handle(request, state.clone()));
                        tokio::select! {
                            _ = cancel.cancelled() => {},
                            _ = tokio::time::timeout(Duration::from_secs(600), hyper::server::conn::http1::Builder::new()
                                .max_buf_size(32 * 1024).serve_connection(TokioIo::new(stream), service)) => {},
                        }
                    });
                }
            }
        }
    });
    Ok(Server {
        url,
        bearer,
        stop,
        task,
    })
}

fn response(status: StatusCode, body: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(if status == StatusCode::ACCEPTED {
            String::new()
        } else {
            body.to_string()
        })))
        .unwrap()
}

async fn handle(
    request: Request<Incoming>,
    state: Arc<State>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(&format!("Bearer {}", state.bearer))
        || request.headers().contains_key("origin")
    {
        return Ok(response(
            StatusCode::FORBIDDEN,
            json!({"error":"unauthorized"}),
        ));
    }
    if request.uri().path() != "/mcp" {
        return Ok(response(StatusCode::NOT_FOUND, json!({})));
    }
    if request.method() != hyper::Method::POST {
        return Ok(response(StatusCode::METHOD_NOT_ALLOWED, json!({})));
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(10),
        Limited::new(request.into_body(), 2 * 1024 * 1024).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        _ => return Ok(response(StatusCode::PAYLOAD_TOO_LARGE, json!({}))),
    };
    let Ok(frame) = serde_json::from_slice::<Value>(&body) else {
        return Ok(response(StatusCode::BAD_REQUEST, json!({})));
    };
    let Some(id) = frame.get("id") else {
        return Ok(response(StatusCode::ACCEPTED, Value::Null));
    };
    let result = match frame["method"].as_str() {
        Some("initialize") => {
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"sophonote","version":"1"}})
        }
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools":definitions()}),
        Some("tools/call") => {
            let request = tools::Request {
                name: frame["params"]["name"].as_str().unwrap_or_default().into(),
                args: frame["params"]["arguments"].clone(),
            };
            let result = invoke(&state, &request).await;
            json!({"content":[{"type":"text","text":result.as_ref().map(String::as_str).unwrap_or_else(|e| e.as_str())}],"isError":result.is_err()})
        }
        _ => {
            return Ok(response(
                StatusCode::OK,
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
            ))
        }
    };
    Ok(response(
        StatusCode::OK,
        json!({"jsonrpc":"2.0","id":id,"result":result}),
    ))
}

async fn invoke(state: &State, request: &tools::Request) -> Result<String, String> {
    // Serialize calls so an approval always applies to exactly the displayed request.
    let mut controls = tokio::select! {
        _ = state.cancel.cancelled() => return Err("操作已取消".into()),
        controls = state.controls.lock() => controls,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let emit = |payload| {
        state
            .events
            .emit(payload)
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    emit(AgentEventPayload::ToolStarted {
        call_id: id.clone(),
        name: request.name.clone(),
        arguments_json: request.args.to_string(),
    })?;
    let result = async {
        if state.scope.validate(request)? {
            while controls.try_recv().is_ok() {}
            emit(AgentEventPayload::ApprovalRequired { approval_id: id.clone(), tool_name: request.name.clone(), arguments_json: request.args.to_string(), choices: vec!["once".into(),"deny".into()] })?;
            loop {
                let control = tokio::select! {
                    _ = state.cancel.cancelled() => return Err("操作已取消".into()),
                    control = tokio::time::timeout(Duration::from_secs(600), controls.recv()) => control.map_err(|_| "审批等待超时")?,
                };
                match control {
                    Some(GatewayControl::Approval { choice, .. }) if choice == "once" => break,
                    Some(GatewayControl::Approval { choice, .. }) if choice == "deny" => return Err("用户拒绝了此操作".into()),
                    None => return Err("审批通道已关闭".into()),
                    _ => {},
                }
            }
        }
        tools::execute(&state.scope, request, &state.cancel).await
    }.await;
    emit(AgentEventPayload::ToolCompleted {
        call_id: id,
        name: request.name.clone(),
        ok: result.is_ok(),
        error: result.as_ref().err().cloned(),
        preresolved: false,
        structured: json!({"text":result.as_ref().map(String::as_str).unwrap_or_default()}),
        ui_artifact: None,
        truncated: false,
        provenance: vec![],
    })?;
    result
}
