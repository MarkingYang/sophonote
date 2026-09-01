//! H2 / NEXT-019：Hermes Supervisor 契约集成测试（协议 stub，非生产 Runtime）。
#![cfg(feature = "legacy-hermes-fixture")]

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sophonote_lib::agent::hermes::{
    file_sha256_hex, start_sidecar, HermesClientError, HermesHttpClient, HermesSidecarConfig,
};

fn stub_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hermes_health_stub"))
}

fn temp_hermes_home(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sophonote-hermes-h2-{tag}-{nanos}"));
    fs::create_dir_all(&dir).expect("create hermes_home");
    dir
}

#[tokio::test]
async fn supervisor_health_detailed_ready() {
    let home = temp_hermes_home("ready");
    let config = HermesSidecarConfig::for_binary(stub_bin(), home.clone()).expect("config");
    let handle = start_sidecar(&config).await.expect("start sidecar");

    let live = handle.client().health().await.expect("health");
    assert_eq!(live.status, "ok");

    handle.health_detailed_ok().await.expect("detailed");
    handle.shutdown().expect("shutdown");
    let _ = fs::remove_dir_all(home);
}

#[tokio::test]
async fn health_detailed_rejects_bad_bearer() {
    let home = temp_hermes_home("auth");
    let config = HermesSidecarConfig::for_binary(stub_bin(), home.clone()).expect("config");
    let handle = start_sidecar(&config).await.expect("start sidecar");

    let err = handle
        .client()
        .health_detailed_with_bearer("definitely-wrong-token")
        .await
        .expect_err("must 401");
    assert!(matches!(err, HermesClientError::Unauthorized));

    // 正确 token 仍可用
    let ok = handle
        .client()
        .health_detailed()
        .await
        .expect("detailed ok");
    assert!(ok.is_ready());

    handle.shutdown().expect("shutdown");
    let _ = fs::remove_dir_all(home);
}

#[tokio::test]
async fn wrong_sha256_refuses_spawn() {
    let home = temp_hermes_home("badhash");
    let mut config = HermesSidecarConfig::for_binary(stub_bin(), home.clone()).expect("config");
    config.expected_sha256 =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into();

    let err = match start_sidecar(&config).await {
        Ok(h) => {
            let _ = h.shutdown();
            panic!("must refuse wrong sha256");
        }
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("sha256") || msg.contains("配置"),
        "unexpected err: {msg}"
    );
    let _ = fs::remove_dir_all(home);
}

#[tokio::test]
async fn crash_does_not_touch_notes_markdown() {
    let root = temp_hermes_home("notes-iso");
    let notes = root.join("notes");
    fs::create_dir_all(&notes).expect("notes dir");
    let article = notes.join("fixture-article.md");
    let body = "# fixture\n\nbody must survive sidecar crash\n";
    fs::write(&article, body).expect("write note");
    let before_hash = file_sha256_hex(&article).expect("hash before");
    let before_meta = fs::metadata(&article).expect("meta before");
    let before_len = before_meta.len();

    let hermes_home = root.join("hermes-home");
    let config = HermesSidecarConfig::for_binary(stub_bin(), hermes_home).expect("config");
    let handle = start_sidecar(&config).await.expect("start");

    // cwd / hermes_home 不得等于 notes
    assert!(!handle.workdir().starts_with(&notes));
    assert!(!handle.hermes_home().starts_with(&notes));

    // 强制崩溃
    handle.shutdown().expect("kill");

    let after = fs::read_to_string(&article).expect("read after");
    assert_eq!(after, body);
    let after_hash = file_sha256_hex(&article).expect("hash after");
    assert_eq!(before_hash, after_hash);
    let after_meta = fs::metadata(&article).expect("meta after");
    assert_eq!(before_len, after_meta.len());

    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn client_can_talk_to_manual_base_url_shape() {
    // 轻量：仅验证 HealthStatus 反序列化契约（不启动进程）
    let json = r#"{"status":"ok"}"#;
    let parsed: sophonote_lib::agent::hermes::HealthStatus =
        serde_json::from_str(json).expect("parse");
    assert_eq!(parsed.status, "ok");

    let detailed = r#"{"status":"ok","readiness":{"checks":[]}}"#;
    let d: sophonote_lib::agent::hermes::DetailedHealth =
        serde_json::from_str(detailed).expect("parse detailed");
    assert!(d.is_ready());

    // 类型可构造（防回归）
    let _ = HermesHttpClient::new("http://127.0.0.1:9", "x");
}
