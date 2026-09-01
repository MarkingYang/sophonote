use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::commands::ApiResponse;
use crate::local_workspace::canonical_root;

const TERMINAL_OUTPUT_EVENT: &str = "sophonote:terminal-output";
const TERMINAL_EXIT_EVENT: &str = "sophonote:terminal-exit";
const MAX_TERMINAL_INPUT_BYTES: usize = 1024 * 1024;

struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

pub struct TerminalManager {
    sessions: Mutex<HashMap<String, TerminalSession>>,
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutputPayload {
    session_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalExitPayload {
    session_id: String,
}

fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 500),
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[tauri::command]
pub fn local_terminal_create(
    app: AppHandle,
    manager: State<'_, TerminalManager>,
    root: String,
    cols: u16,
    rows: u16,
) -> ApiResponse<String> {
    let root = match canonical_root(&root) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(error),
    };
    let pair = match native_pty_system().openpty(terminal_size(cols, rows)) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("无法创建终端：{error}")),
    };

    let mut command = CommandBuilder::new("/bin/zsh");
    command.cwd(root);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let child = match pair.slave.spawn_command(command) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("无法启动 zsh：{error}")),
    };
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("无法读取终端：{error}")),
    };
    let writer = match pair.master.take_writer() {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("无法写入终端：{error}")),
    };
    let session_id = Uuid::new_v4().to_string();
    let session = TerminalSession {
        master: pair.master,
        writer,
        child,
    };
    match manager.sessions.lock() {
        Ok(mut sessions) => {
            sessions.insert(session_id.clone(), session);
        }
        Err(_) => return ApiResponse::err("终端会话状态不可用".to_string()),
    }

    let reader_session_id = session_id.clone();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => {
                    let payload = TerminalOutputPayload {
                        session_id: reader_session_id.clone(),
                        data: base64::engine::general_purpose::STANDARD.encode(&buffer[..length]),
                    };
                    if app.emit(TERMINAL_OUTPUT_EVENT, payload).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = app.emit(
            TERMINAL_EXIT_EVENT,
            TerminalExitPayload {
                session_id: reader_session_id.clone(),
            },
        );
        if let Ok(mut sessions) = app.state::<TerminalManager>().sessions.lock() {
            sessions.remove(&reader_session_id);
        }
    });

    ApiResponse::ok(session_id)
}

#[tauri::command]
pub fn local_terminal_write(
    manager: State<'_, TerminalManager>,
    session_id: String,
    data: String,
) -> ApiResponse<()> {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(data) {
        Ok(value) => value,
        Err(error) => return ApiResponse::err(format!("终端输入无效：{error}")),
    };
    if bytes.len() > MAX_TERMINAL_INPUT_BYTES {
        return ApiResponse::err("单次终端输入不能超过 1 MB".to_string());
    }
    let mut sessions = match manager.sessions.lock() {
        Ok(value) => value,
        Err(_) => return ApiResponse::err("终端会话状态不可用".to_string()),
    };
    let Some(session) = sessions.get_mut(&session_id) else {
        return ApiResponse::err("终端会话已结束".to_string());
    };
    if let Err(error) = session
        .writer
        .write_all(&bytes)
        .and_then(|_| session.writer.flush())
    {
        return ApiResponse::err(format!("无法写入终端：{error}"));
    }
    ApiResponse::ok(())
}

#[tauri::command]
pub fn local_terminal_resize(
    manager: State<'_, TerminalManager>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> ApiResponse<()> {
    let sessions = match manager.sessions.lock() {
        Ok(value) => value,
        Err(_) => return ApiResponse::err("终端会话状态不可用".to_string()),
    };
    let Some(session) = sessions.get(&session_id) else {
        return ApiResponse::err("终端会话已结束".to_string());
    };
    match session.master.resize(terminal_size(cols, rows)) {
        Ok(()) => ApiResponse::ok(()),
        Err(error) => ApiResponse::err(format!("无法调整终端大小：{error}")),
    }
}

#[tauri::command]
pub fn local_terminal_close(
    manager: State<'_, TerminalManager>,
    session_id: String,
) -> ApiResponse<()> {
    let mut session = match manager.sessions.lock() {
        Ok(mut sessions) => sessions.remove(&session_id),
        Err(_) => return ApiResponse::err("终端会话状态不可用".to_string()),
    };
    if let Some(active) = session.as_mut() {
        let _ = active.child.kill();
    }
    ApiResponse::ok(())
}

#[cfg(test)]
mod tests {
    use super::terminal_size;

    #[test]
    fn terminal_dimensions_are_clamped() {
        let smallest = terminal_size(0, 1);
        assert_eq!((smallest.cols, smallest.rows), (2, 2));
        let largest = terminal_size(u16::MAX, u16::MAX);
        assert_eq!((largest.cols, largest.rows), (500, 500));
    }
}
