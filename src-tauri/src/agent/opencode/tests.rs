use super::*;
use crate::agent::{
    engine::HermesSessionBinding,
    events::{AgentEvent, AgentEventPayload, EventEmitter, EventTransport},
    pi::{tools, ExecutionRuntime, Prepared},
};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    Request, Response,
};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::path::Path;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct Capture(Mutex<Vec<AgentEvent>>);
impl EventTransport for Capture {
    fn send(&self, event: AgentEvent) -> Result<(), String> {
        if matches!(event.payload, AgentEventPayload::ApprovalRequired { .. }) {
            crate::agent::hermes::gateway_client::send_run_control(
                &event.run_id,
                crate::agent::hermes::gateway_client::GatewayControl::Approval {
                    choice: "once".into(),
                    all: false,
                },
            );
        }
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

fn stream(tool: Option<Value>, text: &str) -> String {
    let delta = if let Some(tool) = &tool {
        json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("call_{}",uuid::Uuid::new_v4()),"type":"function","function":{"name":tool["name"],"arguments":tool["input"].to_string()}}]})
    } else {
        json!({"role":"assistant","content":text})
    };
    let chunk = |delta: Value, reason: Value| {
        json!({"id":"fixture","object":"chat.completion.chunk","created":1,"model":"fixture-model","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string()
    };
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(delta, Value::Null),
        chunk(
            json!({}),
            json!(if tool.is_some() { "tool_calls" } else { "stop" })
        )
    )
}

struct Fixture {
    requests: Mutex<Vec<Value>>,
    cancellation_ready: tokio::sync::Notify,
}
async fn model(
    request: Request<Incoming>,
    fixture: Arc<Fixture>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let path = request.uri().path().to_string();
    if !path.starts_with("/v1/chat/completions") {
        return Ok(Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from("{}")))
            .unwrap());
    }
    assert_eq!(
        request.headers().get("authorization").unwrap(),
        "Bearer fixture-secret"
    );
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    if path.ends_with("count_tokens") {
        return Ok(Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from("{\"input_tokens\":1}")))
            .unwrap());
    }
    let index = {
        let mut requests = fixture.requests.lock().unwrap();
        requests.push(body);
        requests.len() - 1
    };
    let data = match index {
        0 => stream(Some(json!({"name":"sophonote_write","input":{"path":"output.txt","content":"native OpenCode fixture"}})), ""),
        1 => stream(None, "first complete"),
        2 => stream(Some(json!({"name":"sophonote_read","input":{"path":"output.txt"}})), ""),
        3 => stream(None, "history complete"),
        4 => stream(Some(json!({"name":"sophonote_write","input":{"path":"denied.txt","content":"must not exist"}})), ""),
        5 => stream(None, "plan denied"),
        6 => return Ok(Response::builder().status(400).header("content-type","application/json").body(Full::new(Bytes::from("{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"fixture rejected\"}}"))).unwrap()),
        _ => { fixture.cancellation_ready.notify_one(); std::future::pending::<()>().await; unreachable!() },
    };
    Ok(Response::builder()
        .header("content-type", "text/event-stream")
        .body(Full::new(Bytes::from(data)))
        .unwrap())
}

#[tokio::test]
#[ignore = "requires pnpm opencode:bundle; uses a local HTTP fixture, never a real provider"]
async fn native_opencode_tools_history_plan_error_and_cancel() {
    let runtime = runtime::verify(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/opencode")
            .join(runtime::target()),
    )
    .unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("opencode-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("home")).unwrap();
    let root = root.canonicalize().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Arc::new(Fixture {
        requests: Mutex::new(vec![]),
        cancellation_ready: tokio::sync::Notify::new(),
    });
    let model_fixture = fixture.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break; };
                    let fixture = model_fixture.clone();
                    connections.spawn(async move {
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream),
                            hyper::service::service_fn(move |request| model(request,fixture.clone()))).await;
                    });
                }
            }
        }
    });
    let binding = HermesSessionBinding {
        db_path: root.join("unused.db"),
        notes_dir: root.join("notes"),
        project_id: None,
        thread_id: "opencode-fixture".into(),
        run_id: format!("opencode-{}", uuid::Uuid::new_v4()),
    };
    let home = root.join("home");
    let mut prepared = Prepared {
        runtime: ExecutionRuntime::OpenCode(runtime),
        provider: crate::model::openai_compat::ProviderSnapshot {
            id: "fixture".into(),
            protocol: "openai".into(),
            base_url: format!("{endpoint}/v1"),
            model: "fixture-model".into(),
            models: vec!["fixture-model".into()],
            requires_key: true,
        },
        model: "fixture-model".into(),
        key: "fixture-secret".into(),
        home: home.clone(),
        session: home.join("session-id"),
        context: String::new(),
        prompt: "Write the fixture file".into(),
        images: vec![],
        scope: tools::Scope {
            workspace: root.clone(),
            read_paths: vec![],
            workcopy: None,
            protected: vec![home.clone()],
            mode: "ask".into(),
        },
        patch: None,
        max_turns: 6,
    };
    let capture = Arc::new(Capture(Mutex::new(vec![])));
    let events = Arc::new(EventEmitter::new(
        &binding.thread_id,
        &binding.run_id,
        capture.clone(),
    ));
    let cancel = CancellationToken::new();
    let first = tokio::time::timeout(
        Duration::from_secs(35),
        transport::run(&prepared, &binding, &events, &cancel),
    )
    .await
    .expect("native timeout")
    .unwrap();
    assert_eq!(first.0, "first complete");
    assert_eq!(
        std::fs::read_to_string(root.join("output.txt")).unwrap(),
        "native OpenCode fixture"
    );
    assert!(capture
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event.payload, AgentEventPayload::ApprovalRequired { .. })));
    prepared.session = home.join("session-id");
    assert!(
        prepared.session.is_file(),
        "OpenCode native session missing"
    );
    prepared.prompt = "Read back that file".into();
    let second = tokio::time::timeout(
        Duration::from_secs(35),
        transport::run(&prepared, &binding, &events, &cancel),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(second.0, "history complete");
    prepared.scope.mode = "plan".into();
    prepared.prompt = "Try writing another file".into();
    let third = tokio::time::timeout(
        Duration::from_secs(35),
        transport::run(&prepared, &binding, &events, &cancel),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(third.0, "plan denied");
    assert!(!root.join("denied.txt").exists());
    let error = tokio::time::timeout(
        Duration::from_secs(35),
        transport::run(&prepared, &binding, &events, &cancel),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.contains("fixture rejected"), "{error}");
    let cancelled = CancellationToken::new();
    let run = transport::run(&prepared, &binding, &events, &cancelled);
    let cancelling = async {
        fixture.cancellation_ready.notified().await;
        cancelled.cancel();
    };
    let (outcome, _) = tokio::time::timeout(Duration::from_secs(35), async {
        tokio::join!(run, cancelling)
    })
    .await
    .unwrap();
    assert!(outcome.unwrap_err().contains("取消"));
    let requests = fixture.requests.lock().unwrap().clone();
    assert!(requests[2]["messages"]
        .to_string()
        .contains("first complete"));
    assert!(
        requests[5]["messages"].to_string().contains("计划模式禁止"),
        "{}",
        requests[5]["messages"]
    );
    let names: Vec<_> = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"sophonote_write"));
    assert!(
        names.iter().all(|name| name.starts_with("sophonote_")),
        "unexpected native tool: {names:?}"
    );
    drop(requests);
    server.abort();
    let _ = server.await;
    crate::agent::hermes::gateway_client::unregister_run_control(&binding.run_id);
    drop(prepared);
    std::fs::remove_dir_all(root).unwrap();
}
