use super::*;
use rusqlite::params;
fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY); INSERT INTO projects VALUES('p1'),('p2'); CREATE TABLE thread_collections(id TEXT PRIMARY KEY);").unwrap();
    store::create_schema(&conn).unwrap();
    conn
}
fn id(now: i64) -> String {
    let mut bytes = *uuid::Uuid::new_v4().as_bytes();
    bytes[..6].copy_from_slice(&now.to_be_bytes()[2..]);
    bytes[6] = (bytes[6] & 15) | 0x70;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn admit(
    conn: &mut Connection,
    conversation: Option<&str>,
    project: Option<&str>,
    message: &str,
) -> Turn {
    store::admit(
        conn,
        &id(now_ms()),
        message,
        conversation,
        project,
        message,
        "test",
        "exact-model",
        now_ms(),
    )
    .unwrap()
    .0
}
#[test]
fn duplicate_is_exact_and_never_creates_agent_tables() {
    let mut conn = database();
    let turn = admit(&mut conn, None, Some("p1"), "翻译：hello");
    let duplicate = store::admit(
        &mut conn,
        &turn.id,
        &turn.message,
        None,
        Some("p1"),
        &turn.message,
        "test",
        "exact-model",
        now_ms(),
    )
    .unwrap();
    assert!(!duplicate.1);
    assert_eq!(duplicate.0.conversation_id, turn.conversation_id);
    assert!(store::duplicate(&conn, &turn.id, "different message", Some("p1")).is_err());
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('agent_runs','agent_threads')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
#[test]
fn project_fence_and_atomic_single_active_turn() {
    let mut conn = database();
    let first = admit(&mut conn, None, Some("p1"), "first");
    assert!(store::turns(&conn, &first.conversation_id, Some("p2")).is_err());
    assert!(store::admit(
        &mut conn,
        &id(now_ms()),
        "next",
        Some(&first.conversation_id),
        Some("p1"),
        "next",
        "test",
        "model",
        now_ms()
    )
    .is_err());
    assert!(store::action(
        &mut conn,
        &first.conversation_id,
        Some("p1"),
        "close",
        None,
        now_ms()
    )
    .is_err());
    admit(&mut conn, None, Some("p2"), "second");
    assert!(store::admit(
        &mut conn,
        &id(now_ms()),
        "third",
        None,
        None,
        "third",
        "test",
        "model",
        now_ms()
    )
    .is_err());
    assert_eq!(store::list(&conn, Some("p1"), "active").unwrap().len(), 1);
    assert!(store::list(&conn, None, "active").unwrap().is_empty());
}
#[test]
fn cancel_before_dispatch_and_late_answer_are_fenced() {
    let mut conn = database();
    let turn = admit(&mut conn, None, None, "first");
    assert!(store::finish(&conn, &turn.id, "cancelled", None, None, None, now_ms()).unwrap());
    assert!(!store::running(&conn, &turn.id, now_ms()).unwrap());
    assert!(!store::finish(
        &conn,
        &turn.id,
        "completed",
        Some("late answer"),
        None,
        None,
        now_ms()
    )
    .unwrap());
    let saved = store::get(&conn, &turn.id).unwrap().unwrap();
    assert_eq!(saved.state, "cancelled");
    assert!(saved.answer.is_none());
}
#[test]
fn recovery_never_replays_and_duplicate_returns_interruption() {
    let mut conn = database();
    let turn = admit(&mut conn, None, None, "original");
    assert!(store::running(&conn, &turn.id, now_ms()).unwrap());
    assert_eq!(store::recover(&conn, now_ms()).unwrap(), 1);
    assert_eq!(store::recover(&conn, now_ms()).unwrap(), 0);
    let duplicate = store::duplicate(&conn, &turn.id, "original", None)
        .unwrap()
        .unwrap();
    assert_eq!(duplicate.state, "interrupted");
    assert!(duplicate.answer.is_none());
    assert!(!store::running(&conn, &turn.id, now_ms()).unwrap());
}

#[test]
fn replay_rechecks_current_project_after_move() {
    let mut conn = database();
    let turn = admit(&mut conn, None, Some("p1"), "private input");
    store::finish(
        &conn,
        &turn.id,
        "completed",
        Some("answer"),
        None,
        None,
        now_ms(),
    )
    .unwrap();
    conn.execute(
        "UPDATE response_conversations SET project_id='p2' WHERE id=?1",
        [&turn.conversation_id],
    )
    .unwrap();
    assert!(store::duplicate(&conn, &turn.id, "private input", Some("p1")).is_err());
    assert!(store::admit(
        &mut conn,
        &turn.id,
        "private input",
        None,
        Some("p1"),
        "private input",
        "test",
        "exact-model",
        now_ms()
    )
    .is_err());
    assert_eq!(
        store::turns(&conn, &turn.conversation_id, Some("p2"))
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn completed_context_and_bounds_do_not_silently_truncate() {
    let mut conn = database();
    let turn = admit(&mut conn, None, None, "first");
    store::finish(
        &conn,
        &turn.id,
        "completed",
        Some(&"x".repeat(store::MAX_TEXT - 5)),
        None,
        None,
        now_ms(),
    )
    .unwrap();
    assert!(store::admit(
        &mut conn,
        &id(now_ms()),
        "next",
        Some(&turn.conversation_id),
        None,
        "next",
        "test",
        "model",
        now_ms()
    )
    .is_err());
    assert!(store::finish(
        &conn,
        &turn.id,
        "completed",
        Some(&"x".repeat(store::MAX_TEXT + 1)),
        None,
        None,
        now_ms()
    )
    .is_err());
}
#[test]
fn history_actions_preserve_identity_and_unknown_usage() {
    let mut conn = database();
    let turn = admit(&mut conn, None, Some("p1"), "first");
    store::finish(
        &conn,
        &turn.id,
        "completed",
        Some("answer"),
        None,
        None,
        now_ms(),
    )
    .unwrap();
    assert!(store::get(&conn, &turn.id)
        .unwrap()
        .unwrap()
        .usage
        .is_none());
    store::action(
        &mut conn,
        &turn.conversation_id,
        Some("p1"),
        "close",
        None,
        now_ms(),
    )
    .unwrap();
    assert!(store::list(&conn, Some("p1"), "active").unwrap().is_empty());
    assert_eq!(store::list(&conn, Some("p1"), "history").unwrap().len(), 1);
    store::action(
        &mut conn,
        &turn.conversation_id,
        Some("p1"),
        "reopen",
        None,
        now_ms(),
    )
    .unwrap();
    assert_eq!(
        store::list(&conn, Some("p1"), "active").unwrap()[0].id,
        turn.conversation_id
    );
    assert!(store::action(
        &mut conn,
        &turn.conversation_id,
        Some("p2"),
        "pin",
        Some("true"),
        now_ms()
    )
    .is_err());
}
#[test]
fn text_routing_rejects_all_tool_context_and_never_rebinds_agent() {
    let base = serde_json::json!({"message":"翻译成中文：hello"});
    assert!(eligible(&serde_json::from_value(base.clone()).unwrap()));
    for (key, value) in [
        ("threadId", serde_json::json!("thread-existing")),
        ("skill", serde_json::json!("writing")),
        ("includeProjectContext", serde_json::json!(true)),
        ("engine", serde_json::json!("pi")),
        ("hermesCommand", serde_json::json!("/help")),
    ] {
        let mut request = base.clone();
        request[key] = value;
        assert!(
            !eligible(&serde_json::from_value(request).unwrap()),
            "{key}"
        );
    }
    assert!(!eligible(
        &serde_json::from_value(serde_json::json!({"message":"搜索最新消息"})).unwrap()
    ));
    assert!(eligible(
        &serde_json::from_value(
            serde_json::json!({"message":"翻译：hello", "workspaceRoot":"/fixture"})
        )
        .unwrap()
    ));
    assert!(!eligible(
        &serde_json::from_value(
            serde_json::json!({"message":"总结 README", "workspaceRoot":"/fixture"})
        )
        .unwrap()
    ));
    assert!(!eligible(&serde_json::from_value(serde_json::json!({"message":"修复代码并测试：然后总结结果", "workspaceRoot":"/fixture"})).unwrap()));
}
#[test]
fn expired_unknown_request_is_rejected_but_known_id_is_recoverable() {
    let mut conn = database();
    let now = now_ms();
    let old = id(now - 48 * 60 * 60 * 1000);
    assert!(store::admit(&mut conn, &old, "old", None, None, "old", "test", "model", now).is_err());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM response_conversations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let turn = admit(&mut conn, None, None, "first");
    conn.execute(
        "UPDATE response_turns SET id=?1 WHERE id=?2",
        params![old, turn.id],
    )
    .unwrap();
    assert!(
        store::admit(&mut conn, &old, "first", None, None, "first", "test", "model", now).is_ok()
    );
}
