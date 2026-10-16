//! Durable manual compatibility and automatic product submission APIs.
use super::{live, Error, State, Submission};
use crate::{
    agent::{commands::AgentRunStartArgs, events::AgentEvent},
    commands::ApiResponse,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    pub client_request_id: String,
    pub request: AgentRunStartArgs,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub submission_id: String,
    pub client_request_id: String,
    pub thread_id: Option<String>,
    pub run_id: Option<String>,
    pub requested_engine: String,
    pub selected_engine: Option<String>,
    pub state: State,
    pub generation: i64,
    pub cancel_requested: bool,
    pub message: Option<String>,
}

fn snapshot(conn: &mut Connection, id: &str) -> Result<Snapshot, String> {
    // Read state and cancellation in one transaction, never mix generations.
    let tx = conn.transaction().map_err(|_| "无法读取提交状态")?;
    let state = super::read(&tx, id).map_err(|e| e.to_string())?;
    if state.gone {
        return Err(Error::RequestGone.to_string());
    }
    let (client_request_id, requested_engine, cancel_requested): (String,String,bool) = tx.query_row(
        "SELECT client_request_id,requested_engine,cancel_requested FROM agent_submissions WHERE id=?1", [id],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_| "无法读取提交状态")?;
    let message = match state.state {
        State::Failed => tx
            .query_row(
                "SELECT failure_reason FROM agent_submissions WHERE id=?1",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap_or(None)
            .or_else(|| Some("提交失败，请查看运行详情或检查模型与运行时设置".into())),
        State::Interrupted => Some("准备过程已中断，保留输入后可重新发送".into()),
        State::Recovering => Some("运行状态尚未核实，请查看已有结果，暂勿重复发送".into()),
        _ => None,
    };
    let result = Snapshot {
        submission_id: state.id,
        client_request_id,
        thread_id: state.thread_id,
        run_id: state.run_id,
        requested_engine,
        selected_engine: tx.query_row("SELECT COALESCE(selected_engine,CASE WHEN requested_engine != 'auto' THEN requested_engine END) FROM agent_submissions WHERE id=?1", [id], |r|r.get(0)).map_err(|_| "无法读取路由结果")?,
        state: state.state,
        generation: state.generation,
        cancel_requested,
        message,
    };
    tx.commit().map_err(|_| "无法读取提交状态")?;
    Ok(result)
}

fn connect(db: &Path) -> Result<Connection, String> {
    let db = db.canonicalize().map_err(|_| "提交数据库不可访问")?;
    live::ensure_owner(&db)?;
    Connection::open(db).map_err(|_| "无法打开提交数据库".into())
}

fn validate(request: &AgentRunStartArgs) -> Result<&str, String> {
    let engine = request
        .engine
        .as_deref()
        .ok_or("手动提交需要明确选择引擎")?;
    super::super::registry::EngineId::parse(engine)?;
    if request.message.len() > 1024 * 1024 {
        return Err("消息超过 1 MiB".into());
    }
    if request
        .focus_document
        .as_ref()
        .is_some_and(|d| d.article_id.trim().is_empty() || d.markdown.is_none())
    {
        return Err("当前笔记上下文不完整，请重新打开笔记后发送".into());
    }
    Ok(engine)
}

#[tauri::command]
pub async fn agent_submission_start(
    app: tauri::AppHandle,
    request: StartRequest,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> ApiResponse<Snapshot> {
    let engine = match validate(&request.request) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(e),
    };
    let db = crate::db::get_db_path(&app);
    let (admitted, lease) = match live::LiveSubmission::admit(
        &db,
        &request.request,
        engine,
        &request.client_request_id,
    ) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(e),
    };
    let initial = match connect(&db).and_then(|mut conn| snapshot(&mut conn, &admitted.id)) {
        Ok(v) => v,
        Err(e) => {
            if let Some(lease) = lease {
                lease.failed();
            }
            return ApiResponse::err(e);
        }
    };
    if let Some(lease) = lease {
        // Only the winner of durable admission gets a lease and may dispatch.
        tauri::async_runtime::spawn(async move {
            let route = match super::super::routing::resolve_request(&app, &request.request) {
                Ok(v) => v,
                Err(error) => {
                    lease.failed_reason(&error);
                    return;
                }
            };
            // Drop invalidates unclaimed work; claim also checks wall-clock deadline
            // after any uninterruptible credential lookup returns.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                super::super::registry::dispatch(app, request.request, on_event, route, lease),
            )
            .await;
        });
    }
    ApiResponse::ok(initial)
}

#[tauri::command]
pub async fn agent_submission_auto_start(
    app: tauri::AppHandle,
    mut request: StartRequest,
    on_event: tauri::ipc::Channel<AgentEvent>,
) -> ApiResponse<Snapshot> {
    if request.request.engine.is_some() {
        return ApiResponse::err("自动任务不接受手动引擎选择".into());
    }
    if request
        .request
        .focus_document
        .as_ref()
        .is_some_and(|d| d.article_id.trim().is_empty() || d.markdown.is_none())
    {
        return ApiResponse::err("当前笔记上下文不完整，请重新打开笔记后发送".into());
    }
    if request.request.message.len() > 1024 * 1024 {
        return ApiResponse::err("消息超过 1 MiB".into());
    }
    let db = crate::db::get_db_path(&app);
    let (admitted, lease) = match live::LiveSubmission::admit(
        &db,
        &request.request,
        "auto",
        &request.client_request_id,
    ) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(e),
    };
    let initial = match connect(&db).and_then(|mut c| snapshot(&mut c, &admitted.id)) {
        Ok(v) => v,
        Err(e) => return ApiResponse::err(e),
    };
    if let Some(mut lease) = lease {
        tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(120), async move {
                let route = match super::super::automatic::resolve(&app, &mut request.request).await
                {
                    Ok(v) => v,
                    Err(error) => {
                        lease.failed_reason(&error);
                        return;
                    }
                };
                if lease.select_engine(route.engine()).is_err() {
                    return;
                }
                super::super::registry::dispatch(app, request.request, on_event, route, lease)
                    .await;
            })
            .await;
        });
    }
    ApiResponse::ok(initial)
}

/// Resolve a lost start response by client identity without resubmitting input.
#[tauri::command]
pub async fn agent_submission_find(
    app: tauri::AppHandle,
    client_request_id: String,
) -> ApiResponse<Snapshot> {
    let result = (|| {
        let mut conn = connect(&crate::db::get_db_path(&app))?;
        let id: String = conn
            .query_row(
                "SELECT id FROM agent_submissions WHERE client_request_id=?1",
                [client_request_id],
                |r| r.get(0),
            )
            .map_err(|_| "提交尚未确认，请保留输入并稍后查询")?;
        snapshot(&mut conn, &id)
    })();
    match result {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

#[tauri::command]
pub async fn agent_submission_get(
    app: tauri::AppHandle,
    submission_id: String,
) -> ApiResponse<Snapshot> {
    match connect(&crate::db::get_db_path(&app))
        .and_then(|mut conn| snapshot(&mut conn, &submission_id))
    {
        Ok(value) => ApiResponse::ok(value),
        Err(e) => ApiResponse::err(e),
    }
}

/// Content is never included in ordinary status/audit snapshots.
fn recover_draft(
    conn: &Connection,
    request_id: &str,
    discard: bool,
) -> Result<Option<AgentRunStartArgs>, String> {
    let request_id = uuid::Uuid::parse_str(request_id)
        .map_err(|_| "提交标识无效")?
        .to_string();
    let raw: Option<String> = conn
        .query_row(
            "SELECT recovery_json FROM agent_submissions WHERE client_request_id=?1 AND gone=0
         AND run_id IS NULL AND state IN ('failed','cancelled','interrupted')",
            [&request_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| "无法读取草稿")?
        .flatten();
    if discard {
        conn.execute(
            "UPDATE agent_submissions SET recovery_json=NULL WHERE client_request_id=?1 AND gone=0
            AND run_id IS NULL AND state IN ('failed','cancelled','interrupted')",
            [&request_id],
        )
        .map_err(|_| "无法丢弃草稿")?;
        return Ok(None);
    }
    raw.map(|value| serde_json::from_str(&value).map_err(|_| "草稿格式无法读取".to_string()))
        .transpose()
}

#[tauri::command]
pub async fn agent_submission_draft(
    app: tauri::AppHandle,
    client_request_id: String,
    discard: bool,
) -> ApiResponse<Option<AgentRunStartArgs>> {
    match connect(&crate::db::get_db_path(&app))
        .and_then(|conn| recover_draft(&conn, &client_request_id, discard))
    {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

/// Persist before signalling: callers may race with claim and token registration.
fn request_cancel(conn: &mut Connection, id: &str) -> Result<Submission, Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = super::read(&tx, id)?;
    if current.gone {
        return Err(Error::RequestGone);
    }
    if current.state.terminal() {
        return Ok(current);
    }
    tx.execute("UPDATE agent_submissions SET cancel_requested=1,generation=generation+1,
        updated_at=max(updated_at,?2),state=CASE WHEN state IN ('accepted','preparing','waiting_credentials','prepared')
        THEN 'cancelled' ELSE state END WHERE id=?1 AND cancel_requested=0",params![id,live::now_ms()])?;
    let result = super::read(&tx, id)?;
    tx.commit()?;
    Ok(result)
}

#[tauri::command]
pub async fn agent_submission_cancel(
    app: tauri::AppHandle,
    submission_id: String,
) -> ApiResponse<Snapshot> {
    let db = crate::db::get_db_path(&app);
    let result = (|| {
        let mut conn = connect(&db)?;
        let state = request_cancel(&mut conn, &submission_id).map_err(|e| e.to_string())?;
        if !state.state.terminal() {
            if let Some(run_id) = state.run_id {
                let runs = super::super::orchestration::supervisor::cancel_for_run(
                    &mut conn,
                    &run_id,
                    live::now_ms(),
                )?;
                super::super::orchestration::supervisor::signal(runs);
            }
        }
        snapshot(&mut conn, &submission_id)
    })();
    match result {
        Ok(v) => ApiResponse::ok(v),
        Err(e) => ApiResponse::err(e),
    }
}

/// Must run after registering the native token and before any model/tool side effect.
pub(crate) fn apply_cancel_intent(
    db: &Path,
    run_id: &str,
    token: &tokio_util::sync::CancellationToken,
) {
    let result = Connection::open(db).and_then(|conn| {
        if super::super::orchestration::supervisor::cancellation_requested(
            &conn,
            run_id,
            live::now_ms(),
        )? {
            return Ok(true);
        }
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_submissions WHERE run_id=?1 AND cancel_requested=1)",
            [run_id],
            |r| r.get::<_, bool>(0),
        )
    });
    if !matches!(result, Ok(false)) {
        token.cancel();
    }
}

#[cfg(test)]
mod tests;
