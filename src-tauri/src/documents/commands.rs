//! AG-24 用户侧命令：dry-run 预览 / 批准应用 / 拒绝 / 撤销；AG-26 追加
//! 逐 hunk 部分批准（approved_hunks 子集）与项目 patch 列表（重启后重建审批卡）。
//!
//! 这是「预览后确认保存」的用户入口：模型侧只能产出 operation（proposed），
//! 落盘必须经这些命令显式触发。命令路径无项目闸门（用户对自己的数据无隔离需求；
//! 列表命令按项目过滤只是视图范围，不是安全边界）。

use rusqlite::Connection;
use tauri::AppHandle;

use crate::commands::ApiResponse;
use crate::documents::service::{self, ApplyResult, PatchPreview, ProjectPatchEntry};
use crate::notes;

/// dry-run 预览：生成 patch 提案与 diff，不写文件
#[tauri::command]
pub fn document_preview_patch(
    app: AppHandle,
    document_id: String,
    base_version: i64,
    expected_text: String,
    replacement_markdown: String,
    idempotency_key: Option<String>,
) -> ApiResponse<PatchPreview> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    match service::preview_patch(
        &conn,
        &notes::notes_dir(&app),
        &document_id,
        base_version,
        &expected_text,
        &replacement_markdown,
        idempotency_key.as_deref(),
        None,
        None,
    ) {
        Ok(preview) => ApiResponse::ok(preview),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 批准应用提案（锁内复检版本与锚点；幂等重入零写入）。
/// AG-26：approved_hunks = 批准的 hunk 下标子集（逐 hunk 部分批准）；
/// None 或覆盖全部 hunk = 整块批准（与 AG-24 行为完全一致）。
#[tauri::command]
pub fn document_apply_patch(
    app: AppHandle,
    operation_id: String,
    approved_hunks: Option<Vec<usize>>,
) -> ApiResponse<ApplyResult> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    let result = match approved_hunks {
        Some(hunks) => {
            service::apply_patch_partial(&conn, &notes::notes_dir(&app), &operation_id, &hunks)
        }
        None => service::apply_patch(&conn, &notes::notes_dir(&app), &operation_id),
    };
    match result {
        Ok(result) => ApiResponse::ok(result),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 拒绝提案（零文件写入）
#[tauri::command]
pub fn document_reject_patch(app: AppHandle, operation_id: String) -> ApiResponse<()> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    match service::reject_patch(&conn, &operation_id) {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 撤销最近一次修订（快照还原为新版本；可再撤销 = redo）
#[tauri::command]
pub fn document_undo(
    app: AppHandle,
    document_id: String,
    idempotency_key: Option<String>,
) -> ApiResponse<ApplyResult> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    match service::undo_last_change(
        &conn,
        &notes::notes_dir(&app),
        &document_id,
        None,
        idempotency_key.as_deref(),
    ) {
        Ok(result) => ApiResponse::ok(result),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 精确撤销指定 Agent patch 的 revision checkpoint；后续写入存在时拒绝覆盖。
#[tauri::command]
pub fn document_undo_patch(app: AppHandle, operation_id: String) -> ApiResponse<ApplyResult> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    match service::undo_patch(&conn, &notes::notes_dir(&app), &operation_id) {
        Ok(result) => ApiResponse::ok(result),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// AG-26：文档当前版本号（前端选区 chip 的 baseVersion 来源；轻量只读）
#[tauri::command]
pub fn document_current_version(app: AppHandle, document_id: String) -> ApiResponse<i64> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    match service::get_current_version(&conn, &document_id) {
        Ok(version) => ApiResponse::ok(version),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// AG-26：项目 patch 操作列表（重启后重建审批卡 + 审计轨迹）。
/// 返回提案全量 diff + 终局态（op_status），前端据此差异渲染：
/// proposed → 可交互审批卡；committed → 已应用 + undo；其余 → 状态展示。
#[tauri::command]
pub fn document_project_patches(
    app: AppHandle,
    project_id: String,
) -> ApiResponse<Vec<ProjectPatchEntry>> {
    let conn = match Connection::open(crate::db::get_db_path(&app)) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("db open failed: {e}")),
    };
    match service::list_project_patches(&conn, &project_id) {
        Ok(list) => ApiResponse::ok(list),
        Err(e) => ApiResponse::err(e.to_string()),
    }
}

/// 用户明确保存某条已完成回答；从持久化消息读取，保持全文，不重新调用模型。
#[tauri::command]
pub fn document_preview_chat_answer(
    app: AppHandle,
    document_id: String,
    base_version: i64,
    expected_markdown: String,
    run_id: String,
) -> ApiResponse<PatchPreview> {
    let result = (|| {
        let conn = Connection::open(crate::db::get_db_path(&app)).map_err(|e| e.to_string())?;
        preview_chat_answer(
            &conn,
            &notes::notes_dir(&app),
            &document_id,
            base_version,
            &expected_markdown,
            &run_id,
        )
    })();
    match result {
        Ok(preview) => ApiResponse::ok(preview),
        Err(error) => ApiResponse::err(error),
    }
}

fn preview_chat_answer(
    conn: &Connection,
    notes_dir: &std::path::Path,
    document_id: &str,
    base_version: i64,
    expected: &str,
    run_id: &str,
) -> Result<PatchPreview, String> {
    let answer: String = conn.query_row(
        "SELECT m.content FROM agent_messages m JOIN agent_runs r ON r.id=m.run_id WHERE m.run_id=?1 AND m.role='assistant' AND r.status IN ('completed', '\"completed\"') ORDER BY m.created_at DESC LIMIT 1",
        [run_id], |row| row.get(0),
    ).map_err(|_| "这条回答尚未完整保存，请等待完成或重新打开会话")?;
    if answer.trim().is_empty() || answer.len() > 1024 * 1024 || expected.len() > 1024 * 1024 {
        return Err("回答为空或内容超过 1 MiB，无法提交笔记审阅".into());
    }
    let body = if expected.is_empty() {
        answer
    } else {
        format!("{expected}\n\n{answer}")
    };
    service::preview_host_document_patch(
        conn,
        notes_dir,
        document_id,
        base_version,
        expected,
        &body,
        Some(&format!(
            "chat-answer:{run_id}:{document_id}:{base_version}"
        )),
        Some(run_id),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod chat_answer_tests {
    use super::*;
    #[test]
    fn saved_answer_becomes_reviewable_note_without_model_or_overwrite() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("chat-answer-{}", uuid::Uuid::new_v4()));
        let fixture = crate::documents::repository::tests::RepoFixture {
            db_path: dir.join("sophonote.db"),
            notes: dir.join("notes"),
            dir,
        };
        std::fs::create_dir_all(&fixture.notes).unwrap();
        let conn = fixture.conn();
        crate::db::create_schema(&conn).unwrap();
        fixture.seed_article("blank", "空白", "");
        fixture.seed_article("existing", "已有", "保留原文");
        conn.execute("INSERT INTO agent_threads(id,title,status,created_at,updated_at) VALUES ('t','t','completed',1,1)",[]).unwrap();
        conn.execute("INSERT INTO agent_runs(id,thread_id,status,created_at,updated_at) VALUES ('r','t','\"completed\"',1,1)",[]).unwrap();
        conn.execute("INSERT INTO agent_messages(id,thread_id,run_id,role,content,created_at) VALUES ('m','t','r','assistant','# 文章\n\n完整正文与引用 [1]',1)",[]).unwrap();
        let version = service::get_current_version(&conn, "blank").unwrap();
        let patch = preview_chat_answer(&conn, &fixture.notes, "blank", version, "", "r").unwrap();
        assert_eq!(patch.new_text, "# 文章\n\n完整正文与引用 [1]");
        assert_eq!(
            preview_chat_answer(&conn, &fixture.notes, "blank", version, "", "r")
                .unwrap()
                .operation_id,
            patch.operation_id
        );
        let original = std::fs::read_to_string(fixture.notes.join("blank.md")).unwrap();
        assert!(!original.contains("完整正文"));
        service::apply_patch(&conn, &fixture.notes, &patch.operation_id).unwrap();
        assert!(std::fs::read_to_string(fixture.notes.join("blank.md"))
            .unwrap()
            .contains("完整正文"));
        let version = service::get_current_version(&conn, "existing").unwrap();
        assert!(preview_chat_answer(
            &conn,
            &fixture.notes,
            "existing",
            version + 1,
            "保留原文",
            "r"
        )
        .is_err());
        let patch =
            preview_chat_answer(&conn, &fixture.notes, "existing", version, "保留原文", "r")
                .unwrap();
        assert_eq!(patch.new_text, "保留原文\n\n# 文章\n\n完整正文与引用 [1]");
        assert!(preview_chat_answer(
            &conn,
            &fixture.notes,
            "existing",
            version,
            "保留原文",
            "missing"
        )
        .is_err());
    }
}
