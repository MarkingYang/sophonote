//! SophoNote 本地数据根的唯一解析入口。
//!
//! 这里刻意区分两类工作区：
//! - `notes/` 是产品 Markdown 真相源，只允许 DocumentService 写入；
//! - `workspace/` 是用户与 Hermes 可直接操作的普通文件区；
//! - `version/` 预留给 managed notes bare repo，当前只建空目录，不写入 Git。

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageLayout {
    pub root: PathBuf,
    pub database: PathBuf,
    pub notes: PathBuf,
    pub workspace: PathBuf,
    pub hermes: PathBuf,
    pub runtime: PathBuf,
    pub logs: PathBuf,
    pub version: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageLayoutInfo {
    pub root: String,
    pub database: String,
    pub notes: String,
    pub workspace: String,
    pub hermes: String,
    pub runtime: String,
    pub logs: String,
    pub version: String,
    pub migration_required: bool,
}

impl StorageLayout {
    pub fn from_root(root: PathBuf) -> Self {
        Self {
            database: root.join("sophonote.db"),
            notes: root.join("notes"),
            workspace: root.join("workspace"),
            hermes: root.join("hermes"),
            runtime: root.join("runtime"),
            logs: root.join("logs"),
            version: root.join("version"),
            root,
        }
    }

    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("解析 SophoNote 数据根失败: {error}"))?;
        Ok(Self::from_root(root))
    }

    /// 品牌迁移（MindBox → SophoNote）：Bundle ID 由 `com.fei.mindbox` 改为
    /// `com.fei.sophonote` 后，Tauri 的 `app_data_dir` 随之切换到新根。
    ///
    /// 首次以新 ID 启动且旧根存在时，整体**复制**旧根（DB、notes、hermes、
    /// workspace 等）到新根，并把 `mindbox.db` 改名为 `sophonote.db`。
    /// 只复制不删除：旧目录原样保留作为回滚备份。新根已存在 `sophonote.db`
    /// 或旧根不存在时直接跳过（幂等）。
    pub fn migrate_legacy_root(app: &AppHandle) -> Result<bool, String> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("解析 SophoNote 数据根失败: {error}"))?;
        let legacy = root
            .parent()
            .map(|parent| parent.join("com.fei.mindbox"))
            .filter(|path| path.is_dir());
        let Some(legacy) = legacy else {
            return Ok(false);
        };
        Self::migrate_legacy_root_at(&legacy, &root)
    }

    /// 可测试版本：显式传入旧根与新根，规则与 [`StorageLayout::migrate_legacy_root`] 一致。
    pub fn migrate_legacy_root_at(legacy: &Path, root: &Path) -> Result<bool, String> {
        if root.join("sophonote.db").exists() {
            return Ok(false);
        }
        copy_dir_recursive(legacy, root).map_err(|error| {
            format!(
                "迁移 MindBox 旧数据根 {} → {} 失败: {error}",
                legacy.display(),
                root.display()
            )
        })?;
        let legacy_db = root.join("mindbox.db");
        if legacy_db.is_file() {
            std::fs::rename(&legacy_db, root.join("sophonote.db")).map_err(|error| {
                format!(
                    "重命名旧数据库 {} → sophonote.db 失败: {error}",
                    legacy_db.display()
                )
            })?;
        }
        Ok(true)
    }

    /// 修补品牌迁移中的 Hermes Cron 局部缺口。
    ///
    /// 整根迁移只在新数据库尚未建立时运行；如果用户曾先启动过新 Bundle ID，
    /// 新根可能已经有数据库和空 Hermes Home，却仍缺少旧计划任务。这里以
    /// `cron/jobs.json` 为幂等门闩：新任务已存在时绝不覆盖；仅在新任务文件
    /// 缺失时，从相同 Runtime 版本的旧私有 Home 补拷任务、可安全替换的空
    /// 执行库及缺失的本地输出。
    pub fn migrate_legacy_hermes_cron(app: &AppHandle, version: &str) -> Result<bool, String> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("解析 SophoNote 数据根失败: {error}"))?;
        let Some(legacy) = root
            .parent()
            .map(|parent| parent.join("com.fei.mindbox"))
            .filter(|path| path.is_dir())
        else {
            return Ok(false);
        };
        Self::migrate_legacy_hermes_cron_at(&legacy, &root, version)
    }

    pub fn migrate_legacy_hermes_cron_at(
        legacy: &Path,
        root: &Path,
        version: &str,
    ) -> Result<bool, String> {
        if version.is_empty()
            || Path::new(version).components().count() != 1
            || version == "."
            || version == ".."
        {
            return Err("Hermes 版本目录无效".into());
        }

        let source = legacy.join("hermes").join(version).join("cron");
        let source_jobs = source.join("jobs.json");
        let target = root.join("hermes").join(version).join("cron");
        let target_jobs = target.join("jobs.json");
        if !source_jobs.is_file() || target_jobs.exists() {
            return Ok(false);
        }

        std::fs::create_dir_all(&target)
            .map_err(|error| format!("创建 Hermes Cron 迁移目录失败: {error}"))?;
        std::fs::copy(&source_jobs, &target_jobs)
            .map_err(|error| format!("迁移 Hermes 计划任务失败: {error}"))?;

        for name in ["notepad.db", "usage_audit.jsonl"] {
            copy_file_if_missing(&source.join(name), &target.join(name))
                .map_err(|error| format!("迁移 Hermes Cron 附属数据失败: {error}"))?;
        }

        let source_executions = source.join("executions.db");
        let target_executions = target.join("executions.db");
        let target_has_sidecar =
            target.join("executions.db-wal").exists() || target.join("executions.db-shm").exists();
        let target_has_runs = if target_executions.is_file() {
            execution_db_has_rows(&target_executions).unwrap_or(true)
        } else {
            false
        };
        if source_executions.is_file() && !target_has_sidecar && !target_has_runs {
            std::fs::copy(&source_executions, &target_executions)
                .map_err(|error| format!("迁移 Hermes 执行历史失败: {error}"))?;
        }

        let source_output = source.join("output");
        if source_output.is_dir() {
            copy_dir_recursive_missing(&source_output, &target.join("output"))
                .map_err(|error| format!("迁移 Hermes 本地任务输出失败: {error}"))?;
        }
        Ok(true)
    }

    pub fn ensure(&self) -> Result<(), String> {
        for directory in [
            &self.root,
            &self.notes,
            &self.workspace,
            &self.hermes,
            &self.runtime,
            &self.logs,
            &self.version,
        ] {
            std::fs::create_dir_all(directory).map_err(|error| {
                format!(
                    "创建 SophoNote 数据目录 {} 失败: {error}",
                    directory.display()
                )
            })?;
        }
        Ok(())
    }

    pub fn hermes_home(&self, version: &str) -> PathBuf {
        self.hermes.join(version)
    }

    pub fn info(&self) -> StorageLayoutInfo {
        StorageLayoutInfo {
            root: display(&self.root),
            database: display(&self.database),
            notes: display(&self.notes),
            workspace: display(&self.workspace),
            hermes: display(&self.hermes),
            runtime: display(&self.runtime),
            logs: display(&self.logs),
            version: display(&self.version),
            // 旧版 DB 与 notes 已经位于 root；新增分区只需幂等 mkdir。
            migration_required: false,
        }
    }
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 递归复制目录（保留文件/子目录结构，不删除目标中已有内容）。
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_symlink() {
            // 符号链接按其指向复制实体，避免目录循环
            let metadata = std::fs::metadata(entry.path())?;
            if metadata.is_dir() {
                copy_dir_recursive(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), &target)?;
            }
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn copy_file_if_missing(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_file() && !dst.exists() {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

fn copy_dir_recursive_missing(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive_missing(&entry.path(), &target)?;
        } else if !target.exists() {
            if file_type.is_symlink() {
                let metadata = std::fs::metadata(entry.path())?;
                if metadata.is_dir() {
                    copy_dir_recursive_missing(&entry.path(), &target)?;
                } else {
                    std::fs::copy(entry.path(), &target)?;
                }
            } else {
                std::fs::copy(entry.path(), &target)?;
            }
        }
    }
    Ok(())
}

fn execution_db_has_rows(path: &Path) -> Result<bool, rusqlite::Error> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let count: i64 =
        connection.query_row("SELECT COUNT(*) FROM executions", [], |row| row.get(0))?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_controlled_notes_separate_from_agent_workspace() {
        let layout = StorageLayout::from_root(PathBuf::from("/tmp/sophonote-layout"));
        assert_eq!(
            layout.database,
            PathBuf::from("/tmp/sophonote-layout/sophonote.db")
        );
        assert_eq!(layout.notes, PathBuf::from("/tmp/sophonote-layout/notes"));
        assert_eq!(layout.version, PathBuf::from("/tmp/sophonote-layout/version"));
        assert_eq!(
            layout.workspace,
            PathBuf::from("/tmp/sophonote-layout/workspace")
        );
        assert!(!layout.workspace.starts_with(&layout.notes));
        assert!(!layout.notes.starts_with(&layout.workspace));
        assert_eq!(
            layout.hermes_home("0.20.0"),
            PathBuf::from("/tmp/sophonote-layout/hermes/0.20.0")
        );
    }

    #[test]
    fn current_layout_needs_no_data_migration() {
        let info = StorageLayout::from_root(PathBuf::from("/tmp/sophonote-layout")).info();
        assert!(!info.migration_required);
        assert!(info.workspace.ends_with("/workspace"));
    }

    #[test]
    fn migrates_legacy_mindbox_root_and_renames_database() {
        let base = std::env::temp_dir().join(format!(
            "sophonote-migrate-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = base.join("com.fei.mindbox");
        let root = base.join("com.fei.sophonote");
        let notes = legacy.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(legacy.join("mindbox.db"), b"db-bytes").unwrap();
        std::fs::write(notes.join("hello.md"), b"# hello").unwrap();

        assert!(StorageLayout::migrate_legacy_root_at(&legacy, &root).unwrap());
        assert!(root.join("sophonote.db").is_file());
        assert!(!root.join("mindbox.db").exists());
        assert_eq!(
            std::fs::read(root.join("notes/hello.md")).unwrap(),
            b"# hello"
        );
        // 只复制不删除：旧根保留作为回滚备份
        assert!(legacy.join("mindbox.db").is_file());

        // 幂等：新根已有 sophonote.db 时不再迁移
        assert!(!StorageLayout::migrate_legacy_root_at(&legacy, &root).unwrap());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn repairs_missing_legacy_cron_without_overwriting_new_jobs() {
        let base = std::env::temp_dir().join(format!(
            "sophonote-cron-migrate-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = base.join("com.fei.mindbox");
        let root = base.join("com.fei.sophonote");
        let legacy_cron = legacy.join("hermes/0.20.0/cron");
        let current_cron = root.join("hermes/0.20.0/cron");
        std::fs::create_dir_all(legacy_cron.join("output/job-1")).unwrap();
        std::fs::create_dir_all(&current_cron).unwrap();
        std::fs::write(legacy_cron.join("jobs.json"), br#"[{"id":"legacy"}]"#).unwrap();
        std::fs::write(legacy_cron.join("output/job-1/run.md"), b"legacy output").unwrap();

        let legacy_db = rusqlite::Connection::open(legacy_cron.join("executions.db")).unwrap();
        legacy_db
            .execute("CREATE TABLE executions (id TEXT PRIMARY KEY)", [])
            .unwrap();
        legacy_db
            .execute("INSERT INTO executions (id) VALUES ('legacy-run')", [])
            .unwrap();
        drop(legacy_db);
        let current_db = rusqlite::Connection::open(current_cron.join("executions.db")).unwrap();
        current_db
            .execute("CREATE TABLE executions (id TEXT PRIMARY KEY)", [])
            .unwrap();
        drop(current_db);

        assert!(StorageLayout::migrate_legacy_hermes_cron_at(&legacy, &root, "0.20.0").unwrap());
        assert_eq!(
            std::fs::read(current_cron.join("jobs.json")).unwrap(),
            br#"[{"id":"legacy"}]"#
        );
        assert!(execution_db_has_rows(&current_cron.join("executions.db")).unwrap());
        assert_eq!(
            std::fs::read(current_cron.join("output/job-1/run.md")).unwrap(),
            b"legacy output"
        );

        std::fs::write(current_cron.join("jobs.json"), br#"[{"id":"new"}]"#).unwrap();
        assert!(!StorageLayout::migrate_legacy_hermes_cron_at(&legacy, &root, "0.20.0").unwrap());
        assert_eq!(
            std::fs::read(current_cron.join("jobs.json")).unwrap(),
            br#"[{"id":"new"}]"#
        );
        assert!(legacy_cron.join("jobs.json").is_file());
        std::fs::remove_dir_all(&base).ok();
    }
}
