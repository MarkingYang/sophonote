use super::*;
use crate::agent::{
    events::EventTransport,
    pi::{runtime, tools},
    types::ThreadListScope,
};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    Request, Response,
};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

struct Model {
    roots: AtomicUsize,
    children: AtomicUsize,
    requests: Mutex<Vec<Value>>,
    cancel: bool,
    ready: tokio::sync::Notify,
}
fn sse(tool: Option<(&str, Value)>, answer: &str) -> String {
    let delta = match &tool {
        Some((name, args)) => {
            json!({"tool_calls":[{"index":0,"id":format!("call-{}",uuid::Uuid::new_v4()),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
        }
        None => json!({"content":answer}),
    };
    let chunk = |delta: Value, finish: Value| json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(delta, Value::Null),
        chunk(
            json!({}),
            json!(if tool.is_some() { "tool_calls" } else { "stop" })
        )
    )
}
async fn respond(
    request: Request<Incoming>,
    model: Arc<Model>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let bytes = request.into_body().collect().await.unwrap().to_bytes();
    let request: Value = serde_json::from_slice(&bytes).unwrap();
    let serialized = request.to_string();
    let root = serialized.contains("ROOT_WORKFLOW_FIXTURE");
    model.requests.lock().unwrap().push(request.clone());
    let response = if root {
        match model.roots.fetch_add(1, Ordering::SeqCst) {
            0 => sse(
                Some((
                    "delegate_task",
                    json!({"profileId":"reviewer","objective":"读取资料并核对标记","contextRefIds":["ref-1"],"requiredCapabilities":["file.read"],"outputSchemaId":"evidence-review.v1"}),
                )),
                "",
            ),
            1 => {
                let start = serialized
                    .find("execution-")
                    .expect("parent received child identity");
                let id: String = serialized[start..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                sse(Some(("await_task", json!({"executionId":id}))), "")
            }
            2 => {
                // A tool reply can race the cancellation signal. The synthetic
                // provider must stay pending, not pretend this is a success run.
                if model.cancel {
                    std::future::pending::<()>().await;
                }
                assert!(serialized.contains("verified_evidence"));
                sse(None, "root completed with verified evidence")
            }
            _ => panic!("unexpected parent replay"),
        }
    } else {
        let names: Vec<_> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["read"],
            "child has no delegation, network, shell or write tools"
        );
        match model.children.fetch_add(1, Ordering::SeqCst) {
            0 => {
                if model.cancel {
                    model.ready.notify_one();
                    std::future::pending::<()>().await;
                }
                let texts: Vec<String> = request["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|m| m["role"] == "user")
                    .flat_map(|m| {
                        m["content"]
                            .as_str()
                            .map(|text| vec![text.to_owned()])
                            .unwrap_or_else(|| {
                                m["content"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .filter_map(|p| p["text"].as_str().map(str::to_owned))
                                    .collect()
                            })
                    })
                    .collect();
                let package = texts
                    .iter()
                    .filter_map(|text| serde_json::from_str::<Value>(text).ok())
                    .find(|v| v.get("sources").is_some())
                    .unwrap_or_else(|| panic!("missing synthetic task package: {texts:?}"));
                sse(
                    Some(("read", json!({"path":package["sources"][0]["path"]}))),
                    "",
                )
            }
            1 => {
                assert!(serialized.contains("immutable_evidence"));
                sse(
                    None,
                    r#"{"summary":"verified_evidence","findings":[],"evidenceRefs":["ref-1"],"unresolved":[]}"#,
                )
            }
            _ => panic!("unexpected child replay"),
        }
    };
    Ok(Response::builder()
        .header("Content-Type", "text/event-stream")
        .body(Full::new(Bytes::from(response)))
        .unwrap())
}

#[tokio::test]
#[ignore = "requires bundled real Pi binary; loopback synthetic model only"]
async fn native_parent_delegates_reads_awaits_and_synthesizes_without_history_leak() {
    exercise(false).await;
}

#[tokio::test]
#[ignore = "requires bundled real Pi binary; loopback synthetic model only"]
async fn native_parent_stop_cancels_real_child_and_releases_both_slots() {
    exercise(true).await;
}

async fn exercise(cancel_test: bool) {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = GATE
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("collaboration-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("agent")).unwrap();
    let root = root.canonicalize().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let model = Arc::new(Model {
        roots: AtomicUsize::new(0),
        children: AtomicUsize::new(0),
        requests: Mutex::new(vec![]),
        cancel: cancel_test,
        ready: tokio::sync::Notify::new(),
    });
    let captured = model.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (socket, _) = tokio::select! {
                connection=listener.accept()=>connection.unwrap(),
                ended=connections.join_next(),if !connections.is_empty()=>{
                    ended.unwrap().expect("synthetic provider assertion failed");
                    continue;
                }
            };
            let model = captured.clone();
            connections.spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(socket),
                        hyper::service::service_fn(move |r| respond(r, model.clone())),
                    )
                    .await;
            });
        }
    });
    let db = root.join("state.db");
    let mut conn = Connection::open(&db).unwrap();
    crate::db::create_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO agent_threads(id,title,status,created_at,updated_at,engine) VALUES('root','验收','\"running\"',0,0,'pi'); INSERT INTO agent_runs(id,thread_id,status,provider,model,created_at,updated_at) VALUES('parent','root','\"running\"','fixture','fixture',0,0);").unwrap();
    let file = root.join("evidence.txt");
    std::fs::write(&file, "immutable_evidence").unwrap();
    let scope = tools::Scope {
        read_lease: None,
        readonly: None,
        workspace: root.clone(),
        read_paths: vec![file.clone()],
        workcopy: None,
        protected: vec![db.clone(), root.join("agent")],
        mode: "plan".into(),
    };
    let time = now();
    let scope_id = super::super::scope::persist_readonly(
        &conn,
        &scope,
        std::slice::from_ref(&file),
        false,
        0,
        time + 600_000,
        time,
    )
    .unwrap();
    let evidence =
        super::super::evidence::FrozenEvidence::capture_file(&scope, "ref-1", &file).unwrap();
    super::super::evidence::persist(&conn, &scope_id, &evidence, time).unwrap();
    let policy = Policy {
        context: BTreeMap::from([("ref-1".into(), evidence.reference())]),
        capabilities: BTreeSet::from([super::super::contracts::Capability::FileRead]),
        scope_snapshot_id: scope_id,
        scope_generation: 0,
        provider_id: "fixture".into(),
        model_id: "fixture".into(),
    };
    let workflow = WorkflowStore::new(&mut conn)
        .begin("parent", "request", &policy, now())
        .unwrap();
    let prepared = Prepared {
        runtime: ExecutionRuntime::Pi(
            runtime::verify(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("resources/pi")
                    .join(runtime::target()),
            )
            .unwrap(),
        ),
        provider: crate::model::openai_compat::ProviderSnapshot {
            id: "fixture".into(),
            protocol: "openai".into(),
            base_url: endpoint,
            model: "fixture".into(),
            models: vec!["fixture".into()],
            requires_key: false,
        },
        model: "fixture".into(),
        key: String::new(),
        home: root.join("agent"),
        session: root.join("root.jsonl"),
        context: String::new(),
        prompt: "ROOT_WORKFLOW_FIXTURE".into(),
        images: vec![],
        scope,
        patch: None,
        max_turns: 8,
    };
    let binding = HermesSessionBinding {
        db_path: db,
        notes_dir: root.join("notes"),
        project_id: None,
        thread_id: "root".into(),
        run_id: "parent".into(),
    };
    let transport: Arc<dyn EventTransport> = Arc::new(RunStoreTransport::new(
        binding.db_path.to_string_lossy().into_owned(),
    ));
    let events = Arc::new(EventEmitter::new("root", "parent", transport));
    let cancel = CancellationToken::new();
    crate::agent::commands::global_cancel_registry().register("parent", cancel.clone());
    let run = crate::agent::pi::transport::run(&prepared, &binding, &events, &cancel);
    tokio::pin!(run);
    if cancel_test {
        tokio::select! {result=&mut run=>panic!("parent unexpectedly ended: {result:?}"),ready=tokio::time::timeout(std::time::Duration::from_secs(20),model.ready.notified())=>{ready.expect("child never reached provider");}}
        assert_eq!(model.children.load(Ordering::SeqCst), 1);
        let signals = super::super::supervisor::cancel_for_run(&mut conn, "parent", now()).unwrap();
        assert_eq!(signals.len(), 2);
        super::super::supervisor::signal(signals);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut run)
                .await
                .unwrap()
                .is_err()
        );
        conn.execute(
            "UPDATE agent_runs SET status='\"cancelled\"' WHERE id='parent'",
            [],
        )
        .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            super::super::supervisor::reconcile(&mut conn, "parent", now()).unwrap();
            if WorkflowStore::new(&mut conn)
                .get(&workflow.id)
                .unwrap()
                .state
                == "cancelled"
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "native child did not confirm termination"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(
            crate::agent::submission::occupied_native_slots(&conn).unwrap(),
            0
        );
        assert_eq!(
            WorkflowStore::new(&mut conn).children("parent").unwrap()[0].state,
            "cancelled"
        );
    } else {
        let result = tokio::time::timeout(std::time::Duration::from_secs(45), &mut run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.0, "root completed with verified evidence");
        assert_eq!(result.1, 3);
        conn.execute(
            "UPDATE agent_runs SET status='\"completed\"' WHERE id='parent'",
            [],
        )
        .unwrap();
        super::super::supervisor::reconcile(&mut conn, "parent", now()).unwrap();
        assert_eq!(
            WorkflowStore::new(&mut conn)
                .get(&workflow.id)
                .unwrap()
                .state,
            "succeeded"
        );
        let children = WorkflowStore::new(&mut conn).children("parent").unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].state, "succeeded");
        assert_eq!(model.roots.load(Ordering::SeqCst), 3);
        assert_eq!(model.children.load(Ordering::SeqCst), 2);
    }
    crate::agent::commands::global_cancel_registry().remove("parent");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "immutable_evidence");
    let store = RunStore::new(conn);
    assert_eq!(
        store
            .list_threads(None, ThreadListScope::Active)
            .unwrap()
            .len(),
        1
    );
    assert!(store.get_messages("root").unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|p| p.ok())
            .filter(|p| p.file_name().to_string_lossy().starts_with("internal-"))
            .count(),
        1
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled(), "provider failed");
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
