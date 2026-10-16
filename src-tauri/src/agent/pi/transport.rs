use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::{tools, Prepared};
use crate::agent::engine::HermesSessionBinding;
use crate::agent::events::{AgentEventPayload, EventEmitter};
use crate::agent::hermes::gateway_client::{register_run_control, GatewayControl};

const FRAME_LIMIT: usize = 8 * 1024 * 1024;

struct ReaderTask(tokio::task::JoinHandle<()>);
impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// LF-only framing, including U+2028/U+2029 inside valid JSON strings, with bounded allocation.
pub async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf().await.map_err(|_| "读取 Pi 事件失败")?;
        if buffer.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err("Pi 返回了不完整事件".into())
            };
        }
        let end = buffer.iter().position(|b| *b == b'\n');
        let count = end.map_or(buffer.len(), |n| n + 1);
        if bytes.len() + count > FRAME_LIMIT {
            return Err("Pi 事件超过大小限制".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if end.is_some() {
            break;
        }
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Pi 返回了无效 JSONL 事件".into())
}

async fn send(stdin: &mut tokio::process::ChildStdin, frame: Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&frame).map_err(|_| "无法编码 Pi 请求")?;
    if bytes.len() > 40 * 1024 * 1024 {
        return Err("Pi 输入超过 40 MiB，请减少附件".into());
    }
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|_| "Pi 输入管道已关闭".to_string())
}

fn text_content(message: &Value) -> String {
    message["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part["type"] == "text")
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn emit(events: &EventEmitter, payload: AgentEventPayload) -> Result<(), String> {
    events
        .emit(payload)
        .map(|_| ())
        .map_err(|e| format!("Pi 事件持久化失败：{e}"))
}

pub fn map_event(frame: &Value) -> Option<AgentEventPayload> {
    let string = |key: &str| frame[key].as_str().unwrap_or_default().to_string();
    match frame["type"].as_str()? {
        "message_update" => {
            let event = &frame["assistantMessageEvent"];
            let text = event["delta"].as_str()?.to_string();
            match event["type"].as_str()? {
                "text_delta" => Some(AgentEventPayload::MessageDelta { text, index: None }),
                "thinking_delta" => Some(AgentEventPayload::ReasoningDelta { text }),
                _ => None,
            }
        }
        "message_end"
            if frame["message"]["role"] == "assistant"
                && frame["message"]["stopReason"] == "toolUse" =>
        {
            let text = text_content(&frame["message"]);
            if text.is_empty() {
                None
            } else {
                Some(AgentEventPayload::MessageInterim {
                    text,
                    already_streamed: true,
                })
            }
        }
        "tool_execution_start" => Some(AgentEventPayload::ToolStarted {
            call_id: string("toolCallId"),
            name: string("toolName"),
            arguments_json: frame["args"].to_string(),
        }),
        "tool_execution_end" => Some(AgentEventPayload::ToolCompleted {
            call_id: string("toolCallId"),
            name: string("toolName"),
            ok: frame["isError"] != true,
            error: if frame["isError"] == true {
                Some(text_content(&frame["result"]))
            } else {
                None
            },
            preresolved: false,
            structured: frame["result"].clone(),
            ui_artifact: None,
            truncated: false,
            provenance: vec![],
        }),
        _ => None,
    }
}

pub async fn run(
    prepared: &Prepared,
    binding: &HermesSessionBinding,
    events: &Arc<EventEmitter>,
    cancel: &CancellationToken,
) -> Result<(String, usize), String> {
    let super::ExecutionRuntime::Pi(runtime) = &prepared.runtime else {
        return Err("Pi 运行时类型不匹配".into());
    };
    let collaboration = crate::agent::orchestration::native_child::parent_context(binding)?;
    if collaboration.is_some() && prepared.scope.mode != "plan" {
        return Err("当前协作执行器只开放计划模式，不能混入写入授权".into());
    }
    let deadline = crate::agent::orchestration::native_child::execution_deadline(binding)?;
    let expires = tokio::time::Instant::now()
        + Duration::from_millis(deadline.map_or(600_000, |value| {
            value.saturating_sub(super::now_ms() as i64).max(0) as u64
        }));
    let context = match &collaboration {
        Some(guidance) => format!("{}\n\n{guidance}", prepared.context),
        None => prepared.context.clone(),
    };
    let mut command = tokio::process::Command::new(&runtime.executable);
    command.args(["--mode","rpc","--offline","--no-builtin-tools","--no-extensions","--no-skills","--no-prompt-templates","--no-themes","--no-context-files","--no-approve"])
        .arg("--extension").arg(&runtime.extension)
        .args(["--provider","sophonote","--model"]).arg(&prepared.model)
        .arg("--session").arg(&prepared.session)
        .current_dir(&prepared.scope.workspace).env_clear()
        .env("HOME", &prepared.home).env("PI_CODING_AGENT_DIR", &prepared.home)
        .env("PI_OFFLINE","1").env("LANG","en_US.UTF-8")
        .env("PATH",std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()))
        .env("SOPHONOTE_PI_PARENT",std::process::id().to_string())
        .env("SOPHONOTE_PI_MAX_REQUESTS",prepared.max_turns.to_string())
        .env("SOPHONOTE_PI_DEADLINE_MS",deadline.unwrap_or(0).to_string())
        .env("SOPHONOTE_PI_MODEL_KEY", if prepared.key.is_empty() { "local-no-key" } else { &prepared.key })
        .env("SOPHONOTE_PI_MODEL",json!({"baseUrl":prepared.provider.base_url,"id":prepared.model,
            "api":if prepared.provider.protocol == "anthropic" { "anthropic-messages" } else { "openai-completions" }}).to_string())
        .env("SOPHONOTE_PI_CONTEXT",context)
        .env("SOPHONOTE_PI_COLLABORATION",if collaboration.is_some(){"1"}else{"0"})
        .env("SOPHONOTE_PI_READONLY",if prepared.scope.read_lease.is_some(){"1"}else{"0"})
        .env("SOPHONOTE_WEB_GUIDANCE",crate::agent::web::guidance())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    // Proxy values only; provider/other agent secrets are never inherited by the sidecar.
    for key in [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
        "no_proxy",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
    ] {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = tools::ManagedChild::new(
        command
            .spawn()
            .map_err(|_| "Pi 进程启动失败，请重新构建或安装运行时")?,
    );
    let mut stdin = child.0.stdin.take().ok_or("Pi 缺少输入管道")?;
    let mut stdout = BufReader::new(child.0.stdout.take().ok_or("Pi 缺少事件管道")?);
    // A dedicated bounded reader preserves partial frames while control/cancel wins select!.
    let (tx, mut frames) = tokio::sync::mpsc::channel(16);
    let _reader = ReaderTask(tokio::spawn(async move {
        loop {
            match read_frame(&mut stdout).await {
                Ok(Some(frame)) => {
                    if tx.send(Ok(frame)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    break;
                }
            }
        }
    }));
    let mut controls = register_run_control(&binding.run_id);
    let mut pending: Option<(String, tools::Request)> = None;
    let mut started = false;
    let mut turns = 0;
    let result = async {
    send(&mut stdin, json!({"id":"handshake","type":"get_commands"})).await?;
    loop {
        let frame = tokio::select! {
            _ = tokio::time::sleep_until(expires), if deadline.is_some() => {
                let mut conn = rusqlite::Connection::open(&binding.db_path).map_err(|_|"任务预算到期，停止状态待核实")?;
                let runs = crate::agent::orchestration::supervisor::cancel_for_run(&mut conn,&binding.run_id,super::now_ms() as i64)?;
                crate::agent::orchestration::supervisor::signal(runs);
                break Err("协作任务已达到时限".into());
            }
            _ = cancel.cancelled() => {
                let _ = send(&mut stdin,json!({"id":"abort","type":"abort"})).await;
                // Let Pi record its aborted assistant turn before forceful process cleanup.
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    while let Some(Ok(frame)) = frames.recv().await {
                        if frame["type"] == "response" && frame["command"] == "abort" { break; }
                    }
                }).await;
                break Err("Pi 已取消".into());
            }
            control = controls.recv(), if pending.is_some() => {
                if let Some(GatewayControl::Approval { approval_id, choice, all: false }) = control {
                    if !matches!(choice.as_str(),"once"|"deny") || pending.as_ref().is_none_or(|(id, _)| id != &approval_id) { continue; }
                    if let Some((id,request)) = pending.take() {
                        let result = if choice == "once" { tools::execute(&prepared.scope,&request,cancel).await }
                            else { Err("用户拒绝了此操作".into()) };
                        send_tool_result(&mut stdin,&id,result).await?;
                    }
                }
                continue;
            }
            frame = tokio::time::timeout(Duration::from_secs(if started { 600 } else { 30 }),frames.recv()) => {
                match frame {
                    Ok(Some(Ok(frame))) => frame,
                    Ok(None) => break Err("Pi 进程在完成回复前退出".into()),
                    Ok(Some(Err(error))) => break Err(error),
                    Err(_) => break Err("Pi 响应超时，可重试或取消本轮".into()),
                }
            }
        };
        if frame["type"] == "response" {
            if frame["success"] == false {
                break Err(format!(
                    "Pi 请求失败：{}",
                    frame["error"].as_str().unwrap_or("运行时拒绝请求")
                ));
            }
            if frame["id"] == "handshake" {
                let ready = frame["data"]["commands"]
                    .as_array()
                    .is_some_and(|commands| {
                        commands.iter().any(|c| c["name"] == "sophonote_host_ready")
                    });
                if !ready {
                    break Err("Pi 宿主权限扩展未加载，已停止执行".into());
                }
                if (collaboration.is_some() || prepared.scope.read_lease.is_some())
                    && !frame["data"]["commands"]
                        .as_array()
                        .is_some_and(|commands| {
                            commands
                                .iter()
                                .any(|c| c["name"] == "sophonote_readonly_collaboration_v1")
                        })
                {
                    break Err("Pi 尚未加载受控协作与请求预算协议".into());
                }
                send(&mut stdin,json!({"id":"prompt","type":"prompt","message":prepared.prompt,"images":prepared.images})).await?;
                started = true;
            }
        }
        if frame["type"] == "turn_start" {
            turns += 1;
            if turns == 1 && prepared.scope.read_lease.is_some() {
                crate::agent::orchestration::native_child::acknowledge_start(binding)?;
            }
            if turns > prepared.max_turns {
                break Err("Pi 已达到本轮模型调用上限".into());
            }
            emit(events, AgentEventPayload::ModelStarted { turn: turns })?;
        }
        if frame["type"] == "extension_ui_request" {
            let id = frame["id"]
                .as_str()
                .ok_or("Pi 工具请求缺少关联 ID")?
                .to_string();
            if frame["method"] != "input" || frame["title"] != "sophonote.tool" {
                send(
                    &mut stdin,
                    json!({"type":"extension_ui_response","id":id,"cancelled":true}),
                )
                .await?;
                continue;
            }
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct ToolRequest {
                name: String,
                args: Value,
                #[serde(default)]
                call_id: Option<String>,
            }
            let envelope: ToolRequest =
                serde_json::from_str(frame["placeholder"].as_str().ok_or("Pi 工具参数缺失")?)
                    .map_err(|_| "Pi 工具参数无效")?;
            let request = tools::Request {
                name: envelope.name,
                args: envelope.args,
            };
            if matches!(
                request.name.as_str(),
                "delegate_task" | "await_task" | "cancel_task"
            ) {
                let result = match envelope
                    .call_id
                    .as_deref()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                {
                    Some(call) if collaboration.is_some() => {
                        crate::agent::orchestration::native_child::handle_tool(
                            prepared,
                            binding,
                            &request.name,
                            &request.args,
                            call,
                            cancel,
                        )
                        .await
                    }
                    _ => Err("此运行没有受控委托权限或原生调用身份".into()),
                };
                send_tool_result(&mut stdin, &id, result).await?;
                continue;
            }
            match prepared.scope.validate(&request) {
                Ok(true) => {
                    if pending.is_some() {
                        break Err("Pi 重复请求未完成的审批".into());
                    }
                    emit(
                        events,
                        AgentEventPayload::ApprovalRequired {
                            approval_id: id.clone(),
                            tool_name: request.name.clone(),
                            arguments_json: request.args.to_string(),
                            choices: vec!["once".into(), "deny".into()],
                        },
                    )?;
                    pending = Some((id, request));
                }
                Ok(false) => {
                    send_tool_result(
                        &mut stdin,
                        &id,
                        tools::execute(&prepared.scope, &request, cancel).await,
                    )
                    .await?;
                }
                Err(error) => {
                    send_tool_result(&mut stdin, &id, Err(error)).await?;
                }
            }
        }
        if let Some(payload) = map_event(&frame) {
            emit(events, payload)?;
        }
        if frame["type"] == "agent_end" {
            let last = frame["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "assistant"));
            match last {
                Some(message)
                    if message["stopReason"] == "error" || message["stopReason"] == "aborted" =>
                {
                    break Err(format!(
                        "Pi 模型调用失败：{}",
                        message["errorMessage"].as_str().unwrap_or("模型未完成回复")
                    ));
                }
                Some(message) => break Ok((text_content(message), turns)),
                None => break Err("Pi 未返回助手回复".into()),
            }
        }
    }
    }.await;
    // EOF triggers Pi shutdown. Drop also kills the process group if native shutdown hangs.
    drop(stdin);
    if !matches!(
        tokio::time::timeout(Duration::from_secs(2), child.0.wait()).await,
        Ok(Ok(_))
    ) {
        child.0.start_kill().map_err(|_| "Pi 进程停止状态待核实")?;
        tokio::time::timeout(Duration::from_secs(3), child.0.wait())
            .await
            .map_err(|_| "Pi 进程停止状态待核实")?
            .map_err(|_| "Pi 进程停止状态待核实")?;
    }
    result
}

async fn send_tool_result(
    stdin: &mut tokio::process::ChildStdin,
    id: &str,
    result: Result<String, String>,
) -> Result<(), String> {
    let value = match result {
        Ok(text) => json!({"ok":true,"text":text}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    send(
        stdin,
        json!({"type":"extension_ui_response","id":id,"value":value.to_string()}),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn framing_preserves_unicode_line_separators_and_rejects_partial_frames() {
        let mut input = BufReader::new("{\"text\":\"甲\u{2028}乙\u{2029}丙\"}\r\n".as_bytes());
        assert_eq!(
            read_frame(&mut input).await.unwrap().unwrap()["text"],
            "甲\u{2028}乙\u{2029}丙"
        );
        assert!(read_frame(&mut input).await.unwrap().is_none());
        assert!(read_frame(&mut BufReader::new("{\"text\":1}".as_bytes()))
            .await
            .is_err());
    }
    #[test]
    fn native_events_keep_reasoning_and_answer_separate() {
        assert!(matches!(
            map_event(
                &json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"思考"}})
            ),
            Some(AgentEventPayload::ReasoningDelta { .. })
        ));
        assert!(matches!(
            map_event(
                &json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"回答"}})
            ),
            Some(AgentEventPayload::MessageDelta { .. })
        ));
        let failure = map_event(
            &json!({"type":"tool_execution_end","toolCallId":"c","toolName":"write","isError":true,"result":{"content":[{"type":"text","text":"用户拒绝"}]}}),
        );
        assert!(matches!(
            failure,
            Some(AgentEventPayload::ToolCompleted { ok: false, .. })
        ));
    }
}
