//! Bridge HTTP + 附着 Hermes 活测（默认 ignore）。
//!
//! ```bash
//! export SOPHONOTE_HERMES_BASE_URL=http://127.0.0.1:18642
//! export SOPHONOTE_HERMES_API_KEY='…'
//! export SOPHONOTE_HERMES_HOME="$HOME/.hermes"
//! cargo test --test hermes_attached_bridge_live -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sophonote_lib::agent::hermes::configure_sophonote_surface;
use sophonote_lib::db::create_schema;
use sophonote_lib::sophonote_mcp::{
    bridge_patch_registry, ensure_bridge_http, issue_lease, ModelRoute, BRIDGE_TOOL_NAMES,
};
use serde_json::json;

#[tokio::test]
async fn bridge_http_tools_call_list_with_lease() {
    let dir = std::env::temp_dir().join(format!(
        "mb-bridge-http-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let notes = dir.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let db = dir.join("sophonote.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    create_schema(&conn).unwrap();
    conn.execute("INSERT INTO projects (id, name) VALUES ('p1', '项目')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO articles (id, title, content, article_type) VALUES ('a1', '笔记', '', 'manual')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO project_documents (project_id, article_id, added_at) VALUES ('p1', 'a1', 1)",
        [],
    )
    .unwrap();
    std::fs::write(
        notes.join("a1.md"),
        "---\nid: a1\ntitle: \"笔记\"\n---\n\nhello bridge\n",
    )
    .unwrap();

    let rt = ensure_bridge_http().expect("listen");
    let lease = issue_lease(
        "run-bridge-1",
        "p1",
        BRIDGE_TOOL_NAMES.iter().map(|s| (*s).to_string()),
        ModelRoute::deepseek_default(),
        60_000,
    );
    let lid = lease.lease_id.clone();
    let tools = Arc::new(bridge_patch_registry(
        db.clone(),
        notes.clone(),
        "p1",
        "run-bridge-1",
    ));
    rt.register_run(lease, tools);

    // loopback 必须绕过系统代理（同 bundled_runtime 健康检查口径）：宿主开代理时
    // 裸 Client::new() 会被截获返回 502，而 bridge 自身只能发 200/202/401/404/405。
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("no-proxy client");
    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "list_project_documents",
            "arguments": {}
        }
    });
    let resp = client
        .post(rt.mcp_url())
        .header("Authorization", format!("Bearer {}", rt.bearer))
        .header("X-SophoNote-Lease-Id", &lid)
        .json(&call)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "status={}", resp.status());
    let v: serde_json::Value = resp.json().await.unwrap();
    let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("a1") || text.contains("笔记") || text.contains("articleId"),
        "unexpected tool text: {text}"
    );
    assert_eq!(v["result"]["isError"], false);

    // 无 lease → 错误
    let bad = client
        .post(rt.mcp_url())
        .header("Authorization", format!("Bearer {}", rt.bearer))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "list_project_documents", "arguments": {} }
        }))
        .send()
        .await
        .unwrap();
    // active_lease 可能仍在，先 finish
    rt.finish_run(&lid);
    let bad2 = client
        .post(rt.mcp_url())
        .header("Authorization", format!("Bearer {}", rt.bearer))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "list_project_documents", "arguments": {} }
        }))
        .send()
        .await
        .unwrap();
    let v2: serde_json::Value = bad2.json().await.unwrap();
    assert_eq!(v2["result"]["isError"], true);
    let _ = bad;

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "需要本机 Hermes gateway + SOPHONOTE_HERMES_*"]
async fn attached_upsert_mcp_config() {
    let rt = ensure_bridge_http().expect("listen");
    let workspace = std::env::temp_dir().join(format!(
        "mb-attached-workspace-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let path = configure_sophonote_surface(&rt.mcp_url(), &rt.bearer, &workspace)
        .expect("upsert")
        .0;
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("sophonote-bridge"));
    assert!(raw.contains(&rt.mcp_url()) || raw.contains("127.0.0.1"));
    println!("wrote {}", path.display());
    let _ = std::fs::remove_dir_all(workspace);
}
