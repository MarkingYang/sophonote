//! NB-12 存储治理：笔记本容量统计 + 孤儿资产 GC。
//!
//! Track A §3.9 **备案例外**（登记于 docs/PRD.md Track A 区块与 docs/architecture.md）：
//! 此为 A 轨首个存储写行为，但独立新文件实现——不碰 notes.rs 任何函数、不碰 SQLite
//! 写路径，仅「目录扫描 + 孤儿文件删除」；孤儿判定 = 不被任何笔记正文引用。
//!
//! 孤儿来源：粘贴图片后删除笔记，assets/<uuid>.<ext> 不随删（delete_article_file 只删 .md）。

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use tauri::AppHandle;

use crate::commands::ApiResponse;
use crate::notes::notes_dir;

#[derive(Serialize, Clone)]
pub struct StorageStats {
    pub note_count: usize,
    pub notes_bytes: u64,
    pub asset_count: usize,
    pub assets_bytes: u64,
    pub orphan_count: usize,
    pub orphan_bytes: u64,
}

#[derive(Serialize, Clone)]
pub struct GcReport {
    pub deleted_count: usize,
    pub freed_bytes: u64,
    pub after: StorageStats,
}

fn assets_dir(app: &AppHandle) -> PathBuf {
    notes_dir(app).join("assets")
}

/// 扫全部笔记 .md，提取 `assets/<name>` 引用（文件名为 uuid+扩展名纯 ASCII，字节扫描安全）
fn referenced_assets(app: &AppHandle) -> HashSet<String> {
    let mut refs = HashSet::new();
    let entries = match fs::read_dir(notes_dir(app)) {
        Ok(e) => e,
        Err(_) => return refs,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let bytes = text.as_bytes();
        let mut i = 0;
        while i + 7 <= bytes.len() {
            if &bytes[i..i + 7] == b"assets/" {
                let start = i + 7;
                let mut end = start;
                while end < bytes.len() {
                    let c = bytes[end];
                    if c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_' {
                        end += 1;
                    } else {
                        break;
                    }
                }
                if end > start {
                    refs.insert(String::from_utf8_lossy(&bytes[start..end]).to_string());
                }
                i = end.max(i + 1);
            } else {
                i += 1;
            }
        }
    }
    refs
}

/// assets 目录文件清单（name, 大小）；读失败视为空目录
fn list_assets(app: &AppHandle) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(assets_dir(app)) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let name = match path.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };
        out.push((name, len));
    }
    out
}

fn compute_stats(app: &AppHandle) -> StorageStats {
    let mut note_count = 0usize;
    let mut notes_bytes = 0u64;
    if let Ok(entries) = fs::read_dir(notes_dir(app)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("md") {
                note_count += 1;
                notes_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    let refs = referenced_assets(app);
    let mut asset_count = 0usize;
    let mut assets_bytes = 0u64;
    let mut orphan_count = 0usize;
    let mut orphan_bytes = 0u64;
    for (name, len) in list_assets(app) {
        asset_count += 1;
        assets_bytes += len;
        if !refs.contains(&name) {
            orphan_count += 1;
            orphan_bytes += len;
        }
    }
    StorageStats {
        note_count,
        notes_bytes,
        asset_count,
        assets_bytes,
        orphan_count,
        orphan_bytes,
    }
}

/// 只读容量统计：笔记正文合计 / 资产合计 / 孤儿合计（笔记本页头部展示）
#[tauri::command]
pub fn notebook_storage_stats(app: AppHandle) -> ApiResponse<StorageStats> {
    ApiResponse::ok(compute_stats(&app))
}

/// 孤儿资产清理：删除时重算防陈旧快照，逐个删、失败跳过不中断；返回清理后统计
#[tauri::command]
pub fn gc_orphan_assets(app: AppHandle) -> ApiResponse<GcReport> {
    let refs = referenced_assets(&app);
    let dir = assets_dir(&app);
    let mut deleted_count = 0usize;
    let mut freed_bytes = 0u64;
    for (name, len) in list_assets(&app) {
        if refs.contains(&name) {
            continue;
        }
        match fs::remove_file(dir.join(&name)) {
            Ok(()) => {
                deleted_count += 1;
                freed_bytes += len;
            }
            Err(e) => eprintln!("[storage_gc] remove orphan asset failed: {} ({})", name, e),
        }
    }
    ApiResponse::ok(GcReport {
        deleted_count,
        freed_bytes,
        after: compute_stats(&app),
    })
}
