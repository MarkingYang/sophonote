//! P1C / NEXT-053 KB-0：版本证据契约与当前笔记基线预览。
//!
//! 本阶段只落 schema、feature flag 和只读预览。不写 Git、不入保存热路径、
//! 不把「已保存」伪装成「已建版」。UI 不得声称已支持版本溯源。

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::commands::ApiResponse;
use crate::db::get_db_path;
use crate::storage_layout::StorageLayout;

pub const VERSION_FLAG_KEY: &str = "knowledge.version.enabled";
pub const MANAGED_NOTES_REPO_ID: &str = "managed-notes";
const MAX_PREVIEW_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PREVIEW_FILES: usize = 500;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionStatus {
    pub enabled: bool,
    pub repository_id: Option<String>,
    pub authorization_state: Option<String>,
    pub document_version_count: i64,
    pub queued_job_count: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BaselineFile {
    pub path: String,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BaselinePreview {
    pub file_count: usize,
    pub total_bytes: u64,
    pub skipped: usize,
    pub files: Vec<BaselineFile>,
}

pub fn create_version_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS repositories (
            id TEXT PRIMARY KEY,
            mode TEXT NOT NULL,
            display_name TEXT NOT NULL,
            root_locator TEXT,
            default_ref TEXT,
            head_oid TEXT,
            include_globs_json TEXT,
            exclude_globs_json TEXT,
            authorization_state TEXT NOT NULL DEFAULT 'ready',
            index_state TEXT NOT NULL DEFAULT 'idle',
            last_indexed_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS repository_projects (
            repository_id TEXT NOT NULL,
            project_id TEXT NOT NULL,
            ref_name TEXT,
            path_prefix TEXT,
            PRIMARY KEY(repository_id, project_id)
        );

        CREATE TABLE IF NOT EXISTS document_versions (
            id TEXT PRIMARY KEY,
            repository_id TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            commit_oid TEXT NOT NULL,
            parent_oid TEXT,
            path TEXT NOT NULL,
            article_version INTEGER,
            content_hash TEXT NOT NULL,
            trigger TEXT NOT NULL,
            summary TEXT,
            actor_type TEXT NOT NULL,
            source_operation_id TEXT,
            source_run_id TEXT,
            manifest_json TEXT,
            created_at INTEGER NOT NULL,
            UNIQUE(repository_id, commit_oid, path)
        );

        CREATE TABLE IF NOT EXISTS version_jobs (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            entity_id TEXT,
            idempotency_key TEXT NOT NULL UNIQUE,
            payload_json TEXT,
            state TEXT NOT NULL DEFAULT 'queued',
            attempts INTEGER NOT NULL DEFAULT 0,
            available_at INTEGER NOT NULL,
            lease_until INTEGER,
            last_error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_document_versions_repo
            ON document_versions(repository_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_version_jobs_state
            ON version_jobs(state, available_at);
        "#,
    )
    .map_err(|e| e.to_string())
}

pub fn is_enabled(conn: &rusqlite::Connection) -> Result<bool, String> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            rusqlite::params![VERSION_FLAG_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(matches!(value.as_deref(), Some("1") | Some("true")))
}

pub fn set_enabled(conn: &rusqlite::Connection, enabled: bool) -> Result<VersionStatus, String> {
    let now = unix_ms();
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = datetime('now')",
        rusqlite::params![VERSION_FLAG_KEY, if enabled { "1" } else { "0" }],
    )
    .map_err(|e| e.to_string())?;

    if enabled {
        conn.execute(
            "INSERT INTO repositories (
                id, mode, display_name, authorization_state, index_state, created_at, updated_at
             ) VALUES (?1, 'managed_notes', 'SophoNote 笔记', 'ready', 'idle', ?2, ?2)
             ON CONFLICT(id) DO UPDATE SET
                authorization_state = 'ready',
                updated_at = excluded.updated_at",
            rusqlite::params![MANAGED_NOTES_REPO_ID, now],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "UPDATE repositories SET authorization_state = 'unlinked', updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![MANAGED_NOTES_REPO_ID, now],
        )
        .map_err(|e| e.to_string())?;
    }
    status(conn)
}

pub fn status(conn: &rusqlite::Connection) -> Result<VersionStatus, String> {
    let enabled = is_enabled(conn)?;
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT id, authorization_state FROM repositories WHERE id = ?1",
            rusqlite::params![MANAGED_NOTES_REPO_ID],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let document_version_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM document_versions WHERE repository_id = ?1",
            rusqlite::params![MANAGED_NOTES_REPO_ID],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let queued_job_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM version_jobs WHERE repository_id = ?1 AND state IN ('queued', 'running')",
            rusqlite::params![MANAGED_NOTES_REPO_ID],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(VersionStatus {
        enabled,
        repository_id: row.as_ref().map(|(id, _)| id.clone()),
        authorization_state: row.map(|(_, state)| state),
        document_version_count,
        queued_job_count,
    })
}

pub fn preview_notes_baseline(notes_dir: &Path) -> Result<BaselinePreview, String> {
    let mut files = Vec::new();
    let mut skipped = 0usize;
    let mut total_bytes = 0u64;
    collect_markdown(
        notes_dir,
        notes_dir,
        &mut files,
        &mut skipped,
        &mut total_bytes,
    )?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if files.len() > MAX_PREVIEW_FILES {
        skipped += files.len() - MAX_PREVIEW_FILES;
        files.truncate(MAX_PREVIEW_FILES);
    }
    Ok(BaselinePreview {
        file_count: files.len(),
        total_bytes,
        skipped,
        files,
    })
}

fn collect_markdown(
    root: &Path,
    dir: &Path,
    files: &mut Vec<BaselineFile>,
    skipped: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "assets" {
            continue;
        }
        if path.is_dir() {
            collect_markdown(root, &path, files, skipped, total_bytes)?;
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                *skipped += 1;
                continue;
            }
        };
        if meta.len() > MAX_PREVIEW_FILE_BYTES {
            *skipped += 1;
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                *skipped += 1;
                continue;
            }
        };
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        *total_bytes += bytes.len() as u64;
        files.push(BaselineFile {
            path: relative,
            content_hash: hex_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn open_db(app: &AppHandle) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn knowledge_version_status(app: AppHandle) -> ApiResponse<VersionStatus> {
    match open_db(&app).and_then(|conn| status(&conn)) {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub fn knowledge_version_set_enabled(app: AppHandle, enabled: bool) -> ApiResponse<VersionStatus> {
    match open_db(&app).and_then(|conn| set_enabled(&conn, enabled)) {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub fn knowledge_version_preview_baseline(app: AppHandle) -> ApiResponse<BaselinePreview> {
    let notes = match StorageLayout::resolve(&app) {
        Ok(layout) => layout.notes,
        Err(error) => return ApiResponse::err(error),
    };
    match preview_notes_baseline(&notes) {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::create_schema;

    fn memory() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("memory");
        create_schema(&conn).expect("schema");
        conn
    }

    #[test]
    fn default_flag_is_off_and_has_no_jobs() {
        let conn = memory();
        let current = status(&conn).expect("status");
        assert!(!current.enabled);
        assert_eq!(current.queued_job_count, 0);
        assert_eq!(current.document_version_count, 0);
        assert!(current.repository_id.is_none());
    }

    #[test]
    fn enable_creates_managed_repo_disable_unlinks_without_dropping_rows() {
        let conn = memory();
        let on = set_enabled(&conn, true).expect("enable");
        assert!(on.enabled);
        assert_eq!(on.repository_id.as_deref(), Some(MANAGED_NOTES_REPO_ID));
        assert_eq!(on.authorization_state.as_deref(), Some("ready"));

        conn.execute(
            "INSERT INTO document_versions (
                id, repository_id, entity_type, entity_id, commit_oid, path,
                content_hash, trigger, actor_type, created_at
             ) VALUES ('v1', ?1, 'article', 'a1', 'preview', 'a1.md', 'abc', 'explicit', 'host', 1)",
            rusqlite::params![MANAGED_NOTES_REPO_ID],
        )
        .expect("seed version");

        let off = set_enabled(&conn, false).expect("disable");
        assert!(!off.enabled);
        assert_eq!(off.authorization_state.as_deref(), Some("unlinked"));
        assert_eq!(off.document_version_count, 1);
    }

    #[test]
    fn baseline_preview_hashes_markdown_and_skips_assets() {
        let root = std::env::temp_dir().join(format!(
            "sophonote-kb0-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("assets")).unwrap();
        let body = "# 甲\n";
        fs::write(root.join("alpha.md"), body).unwrap();
        fs::write(root.join("assets/skip.png"), [0u8; 8]).unwrap();
        fs::write(root.join("notes.txt"), "ignore").unwrap();

        let preview = preview_notes_baseline(&root).expect("preview");
        fs::remove_dir_all(&root).ok();

        assert_eq!(preview.file_count, 1);
        assert_eq!(preview.files[0].path, "alpha.md");
        assert_eq!(preview.files[0].content_hash, hex_sha256(body.as_bytes()));
        assert_eq!(preview.skipped, 0);
    }
}
