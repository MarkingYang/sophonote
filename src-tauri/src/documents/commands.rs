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
