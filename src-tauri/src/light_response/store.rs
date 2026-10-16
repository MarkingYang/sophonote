//! Independent text conversation persistence. No Agent Run, Session, or tool authority.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

pub(super) const MAX_TEXT: usize = 64 * 1024;
pub(super) const MAX_TURNS: usize = 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub id: String,
    pub conversation_id: String,
    pub message: String,
    pub answer: Option<String>,
    pub state: String,
    pub provider: String,
    pub model: String,
    pub error: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub created_at: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub project_id: Option<String>,
    pub conversation_kind: &'static str,
    pub status: String,
    pub latest_run_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub closed_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub pinned_at: Option<i64>,
    pub collection_id: Option<String>,
}

pub(crate) fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS response_conversations(
        id TEXT PRIMARY KEY, project_id TEXT, title TEXT NOT NULL,
        created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        closed_at INTEGER, archived_at INTEGER, pinned_at INTEGER, collection_id TEXT);
        CREATE TABLE IF NOT EXISTS response_turns(
        id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES response_conversations(id),
        intent TEXT NOT NULL, message TEXT NOT NULL, answer TEXT, state TEXT NOT NULL
        CHECK(state IN ('preparing','running','completed','failed','cancelled','interrupted')),
        provider TEXT NOT NULL, model TEXT NOT NULL, error TEXT, usage_json TEXT,
        created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS response_turn_conversation ON response_turns(conversation_id,created_at);
        CREATE UNIQUE INDEX IF NOT EXISTS response_one_active ON response_turns(conversation_id)
        WHERE state IN ('preparing','running');")
}
fn db_error(_: rusqlite::Error) -> String {
    "无法读取或保存响应记录".into()
}
fn row_turn(r: &rusqlite::Row<'_>) -> rusqlite::Result<Turn> {
    let raw: Option<String> = r.get(8)?;
    Ok(Turn {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        message: r.get(2)?,
        answer: r.get(3)?,
        state: r.get(4)?,
        provider: r.get(5)?,
        model: r.get(6)?,
        error: r.get(7)?,
        usage: raw.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(9)?,
    })
}
const TURN_COLUMNS: &str =
    "id,conversation_id,message,answer,state,provider,model,error,usage_json,created_at";
pub(super) fn get(conn: &Connection, id: &str) -> Result<Option<Turn>, String> {
    conn.query_row(
        &format!("SELECT {TURN_COLUMNS} FROM response_turns WHERE id=?1"),
        [id],
        row_turn,
    )
    .optional()
    .map_err(db_error)
}
pub(super) fn check_project(
    conn: &Connection,
    id: &str,
    project: Option<&str>,
) -> Result<(), String> {
    let actual: Option<Option<String>> = conn
        .query_row(
            "SELECT project_id FROM response_conversations WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if actual.as_ref().map(|v| v.as_deref()) != Some(project) {
        return Err("响应会话不属于当前项目".into());
    }
    Ok(())
}
pub(super) fn turns(
    conn: &Connection,
    id: &str,
    project: Option<&str>,
) -> Result<Vec<Turn>, String> {
    check_project(conn, id, project)?;
    let mut stmt=conn.prepare(&format!("SELECT {TURN_COLUMNS} FROM response_turns WHERE conversation_id=?1 ORDER BY created_at,rowid")).map_err(db_error)?;
    let rows = stmt.query_map([id], row_turn).map_err(db_error)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}
pub(super) fn duplicate(
    conn: &Connection,
    id: &str,
    intent: &str,
    project: Option<&str>,
) -> Result<Option<Turn>, String> {
    let existing: Option<String> = conn
        .query_row("SELECT intent FROM response_turns WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db_error)?;
    match existing {
        Some(saved) if saved != intent => Err("请求标识已绑定另一条消息".into()),
        Some(_) => {
            let turn = get(conn, id)?.ok_or("响应记录不存在")?;
            // A known request ID is not authority after a conversation is moved.
            check_project(conn, &turn.conversation_id, project)?;
            Ok(Some(turn))
        }
        None => Ok(None),
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn admit(
    conn: &mut Connection,
    id: &str,
    intent: &str,
    conversation: Option<&str>,
    project: Option<&str>,
    message: &str,
    provider: &str,
    model: &str,
    now: i64,
) -> Result<(Turn, bool), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if let Some(turn) = duplicate(&tx, id, intent, project)? {
        return Ok((turn, false));
    }
    if message.trim().is_empty() || message.len() > MAX_TEXT {
        return Err("消息为空或超过 64 KiB，请缩短后发送".into());
    }
    // UUID v7 makes abandoned IDs expire instead of unexpectedly replaying after cleanup.
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| "请求标识无效")?;
    let mut timestamp = [0u8; 8];
    timestamp[2..].copy_from_slice(&uuid.as_bytes()[..6]);
    let timestamp = i64::from_be_bytes(timestamp);
    if uuid.get_version_num() != 7
        || timestamp > now + 60_000
        || timestamp < now - 24 * 60 * 60 * 1000
    {
        return Err("请求标识已过期，请重新发送".into());
    }
    if let Some(project) = project {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                [project],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        if !exists {
            return Err("项目已不存在".into());
        }
    }
    let count: i64 = tx
        .query_row(
            "SELECT count(*) FROM response_turns WHERE state IN ('preparing','running')",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if count >= 2 {
        return Err("已有两个响应正在执行，请等待或停止后再试".into());
    }
    let conversation = if let Some(conversation) = conversation {
        let history = turns(&tx, conversation, project)?;
        let closed:bool=tx.query_row("SELECT closed_at IS NOT NULL OR archived_at IS NOT NULL FROM response_conversations WHERE id=?1",[conversation],|r|r.get(0)).map_err(db_error)?;
        if closed {
            return Err("请先恢复此会话".into());
        }
        if history
            .iter()
            .any(|t| matches!(t.state.as_str(), "preparing" | "running"))
        {
            return Err("当前响应尚未结束".into());
        }
        let context: usize = history
            .iter()
            .filter(|t| t.state == "completed")
            .map(|t| t.message.len() + t.answer.as_deref().unwrap_or("").len())
            .sum();
        if history.len() >= MAX_TURNS || context + message.len() > MAX_TEXT {
            return Err("对话已达到上下文上限，请新建会话".into());
        }
        conversation.to_owned()
    } else {
        let conversation = format!("response-{}", uuid::Uuid::new_v4());
        let title: String = message.chars().take(40).collect();
        tx.execute("INSERT INTO response_conversations(id,project_id,title,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",params![conversation,project,title,now]).map_err(db_error)?;
        conversation
    };
    tx.execute("INSERT INTO response_turns(id,conversation_id,intent,message,state,provider,model,created_at,updated_at) VALUES(?1,?2,?3,?4,'preparing',?5,?6,?7,?7)",params![id,conversation,intent,message,provider,model,now]).map_err(db_error)?;
    tx.execute(
        "UPDATE response_conversations SET updated_at=?2 WHERE id=?1",
        params![conversation, now],
    )
    .map_err(db_error)?;
    let turn = get(&tx, id)?.ok_or("响应记录丢失")?;
    tx.commit().map_err(db_error)?;
    Ok((turn, true))
}
pub(super) fn running(conn: &Connection, id: &str, now: i64) -> Result<bool, String> {
    conn.execute(
        "UPDATE response_turns SET state='running',updated_at=?2 WHERE id=?1 AND state='preparing'",
        params![id, now],
    )
    .map(|v| v == 1)
    .map_err(db_error)
}
pub(super) fn finish(
    conn: &Connection,
    id: &str,
    state: &str,
    answer: Option<&str>,
    error: Option<&str>,
    usage: Option<&serde_json::Value>,
    now: i64,
) -> Result<bool, String> {
    if !matches!(state, "completed" | "failed" | "cancelled")
        || answer.is_some_and(|a| a.len() > MAX_TEXT)
    {
        return Err("响应终态无效".into());
    }
    conn.execute("UPDATE response_turns SET state=?2,answer=?3,error=?4,usage_json=?5,updated_at=?6 WHERE id=?1 AND state IN ('preparing','running')",params![id,state,answer,error,usage.map(ToString::to_string),now]).map(|v|v==1).map_err(db_error)
}
pub(crate) fn recover(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute("UPDATE response_turns SET state='interrupted',error='应用已重启，响应已中断；原消息已保留，未自动重发',updated_at=?1 WHERE state IN ('preparing','running')",[now])
}
pub(super) fn list(
    conn: &Connection,
    project: Option<&str>,
    scope: &str,
) -> Result<Vec<Conversation>, String> {
    if !matches!(scope, "active" | "history") {
        return Err("会话范围无效".into());
    }
    let mut stmt=conn.prepare("SELECT c.id,c.title,c.project_id,c.created_at,c.updated_at,c.closed_at,c.archived_at,c.pinned_at,c.collection_id,
        (SELECT state FROM response_turns t WHERE t.conversation_id=c.id ORDER BY created_at DESC,rowid DESC LIMIT 1)
        FROM response_conversations c WHERE project_id IS ?1 AND archived_at IS NULL AND ((?2='active' AND closed_at IS NULL) OR (?2='history' AND closed_at IS NOT NULL)) ORDER BY updated_at DESC").map_err(db_error)?;
    let rows = stmt
        .query_map(params![project, scope], |r| {
            let state: String = r.get(9)?;
            Ok(Conversation {
                id: r.get(0)?,
                title: r.get(1)?,
                project_id: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
                closed_at: r.get(5)?,
                archived_at: r.get(6)?,
                pinned_at: r.get(7)?,
                collection_id: r.get(8)?,
                conversation_kind: "response",
                latest_run_id: None,
                status: match state.as_str() {
                    "preparing" | "running" => "running",
                    "cancelled" => "cancelled",
                    "completed" => "completed",
                    _ => "failed",
                }
                .into(),
            })
        })
        .map_err(db_error)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_error)
}
pub(super) fn action(
    conn: &mut Connection,
    id: &str,
    project: Option<&str>,
    action: &str,
    value: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let history = turns(&tx, id, project)?;
    if history
        .iter()
        .any(|t| matches!(t.state.as_str(), "preparing" | "running"))
    {
        return Err("请先停止当前响应".into());
    }
    match action {
        "close" => tx.execute(
            "UPDATE response_conversations SET closed_at=?2 WHERE id=?1",
            params![id, now],
        ),
        "reopen" => tx.execute(
            "UPDATE response_conversations SET closed_at=NULL,archived_at=NULL WHERE id=?1",
            [id],
        ),
        "archive" => tx.execute(
            "UPDATE response_conversations SET archived_at=?2 WHERE id=?1",
            params![id, now],
        ),
        "pin" => tx.execute(
            "UPDATE response_conversations SET pinned_at=?2 WHERE id=?1",
            params![
                id,
                if value == Some("true") {
                    Some(now)
                } else {
                    None
                }
            ],
        ),
        "collection" => {
            if let Some(collection) = value {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM thread_collections WHERE id=?1)",
                        [collection],
                        |r| r.get(0),
                    )
                    .map_err(db_error)?;
                if !exists {
                    return Err("分类不存在".into());
                }
            }
            tx.execute(
                "UPDATE response_conversations SET collection_id=?2 WHERE id=?1",
                params![id, value],
            )
        }
        _ => return Err("未知会话操作".into()),
    }
    .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok(true)
}
