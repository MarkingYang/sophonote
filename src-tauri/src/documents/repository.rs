//! DocumentRepository（AG-24，审计批次 5 第 1 步）：唯一底层文档读写层。
//!
//! 从 notes/commands 的基座抽出——既有 Tauri 命令（编辑器直写路径）与
//! DocumentService（Agent 写入路径）都经过本层，保证「全应用单一文档写入路径」
//! （docs/architecture.md「文档底层仓储」行）。
//!
//! 边界：本层只管「读写事实」（读文档 / 写正文 / 建文档 / 版本号）；
//! 冲突策略、审批、dry-run、undo 全部在 service.rs。命令签名零变化。
//!
//! 版本号语义：articles.version 仅在「正文写入」时递增（编辑器心跳与 Agent
//! 写入同源递增），改名不递增——它是正文并发冲突检测（CAS）的真相源。

use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use rusqlite::OptionalExtension;

use crate::db::Article;

/// 文档域统一错误语义（repository/service/commands/tools 共用）。
/// Display 文案直接进用户可见错误与模型回填文本，措辞需稳定。
#[derive(Debug)]
pub enum DocError {
    /// 文档不存在（工具侧对「不属于当前项目」与「不存在」统一用此口径，防成员信息泄漏）
    NotFound(String),
    /// base_version 与当前版本不一致（并发冲突；禁止静默覆盖，docs/architecture.md）
    VersionConflict {
        expected: i64,
        actual: i64,
    },
    Db(String),
    Io(String),
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "文档不存在: {id}"),
            Self::VersionConflict { expected, actual } => write!(
                f,
                "版本冲突：base_version {expected} 已过期，当前版本 {actual}（可能有其它编辑并发修改，请重新读取后再试）"
            ),
            Self::Db(e) => write!(f, "数据库错误: {e}"),
            Self::Io(e) => write!(f, "文件写入失败: {e}"),
        }
    }
}

impl std::error::Error for DocError {}

/// 文档记录 = DB 索引行 + .md 正文 + 版本号
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    pub id: String,
    pub item_id: Option<String>,
    pub title: String,
    pub article_type: String,
    pub edited: bool,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub prompt_version: Option<String>,
    /// 剥 frontmatter 后的正文（.md 为唯一真相源；文件缺失时回退 DB 残留正文，迁移期兜底）
    pub body: String,
    /// .md 文件是否真实存在（patch 要求文件在盘；缺失按 NotFound 处理）
    pub file_exists: bool,
    /// 正文写入版本号（编辑器与 Agent 写入同源递增；CAS 真相源）
    pub version: i64,
}

// ---- 单文档写锁 ----
// 所有正文写入路径（编辑器心跳 db_update_article / Agent apply / undo / create）
// 写入前先取本锁，把「版本检查 → tmp 写入 → rename → 版本递增」串成单一临界区。
// 单进程桌面应用 Mutex 足够；不嵌套持有两把文档锁，无死锁。
//
// 值存 `&'static Mutex<()>`：每个 document_id 首次加锁时 Box::leak 一次（有界——
// 上限=本进程编辑过的文档数，单 Mutex 仅数十字节），换来无 unsafe 的 'static guard；
// 旧实现返回 `Arc` clone 的 guard 会借用局部量，编译不过（E0515）。
static DOCUMENT_LOCKS: OnceLock<Mutex<std::collections::HashMap<String, &'static Mutex<()>>>> =
    OnceLock::new();

/// 取单文档写锁（guard 存活期间持有）。宿主旧 rustc 友好：仅 get_or_init。
pub fn document_lock(id: &str) -> MutexGuard<'static, ()> {
    let map = DOCUMENT_LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    // or_insert_with 返回 &mut &'static Mutex<()>；后续 .lock() 的方法 receiver
    // 探测经一步 auto-deref 直接命中 &'static Mutex<()>（匹配 &self），
    // guard 生命周期不依赖外层 map 锁——不加显式 *（clippy explicit_auto_deref），
    // 也不 let 绑定中转（deref coercion 会把生命周期绑回外层 guard，E0515）
    guard
        .entry(id.to_string())
        .or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 读文档（DB 行 + 文件正文）。文档行不存在 → Ok(None)。
pub fn get_document(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    id: &str,
) -> Result<Option<DocumentRecord>, DocError> {
    // 元组别名化（Clippy type_complexity 口径，同 AG-23 commands.rs）
    type ArticleRow = (
        Option<String>,
        String,
        String,
        i32,
        String,
        Option<String>,
        Option<String>,
        i64,
        String,
    );
    let row: Option<ArticleRow> = conn
        .query_row(
            "SELECT item_id, title, article_type, edited, created_at, updated_at,
                    prompt_version, version, content
             FROM articles WHERE id = ?1",
            rusqlite::params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(|e| DocError::Db(e.to_string()))?;
    let Some((
        item_id,
        title,
        article_type,
        edited,
        created_at,
        updated_at,
        prompt_version,
        version,
        db_content,
    )) = row
    else {
        return Ok(None);
    };
    // 正文：文件为真相；文件缺失回退 DB content（迁移期兜底，同 db_get_articles 口径）
    let (body, file_exists) = match crate::notes::read_article_body_in(notes_dir, id) {
        Some(b) => (b, true),
        None => (db_content, false),
    };
    Ok(Some(DocumentRecord {
        id: id.to_string(),
        item_id,
        title,
        article_type,
        edited: edited != 0,
        created_at,
        updated_at,
        prompt_version,
        body,
        file_exists,
        version,
    }))
}

/// 当前版本号；文档不存在 → Ok(None)
pub fn current_version(conn: &rusqlite::Connection, id: &str) -> Result<Option<i64>, DocError> {
    conn.query_row(
        "SELECT version FROM articles WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(|e| DocError::Db(e.to_string()))
}

/// 写正文单一原语：文件落盘成功 → 版本号递增 + DB 索引更新，返回新版本号。
///
/// - expected_version = Some(base)：CAS 校验，不匹配 → VersionConflict（Agent 路径）；
/// - expected_version = None：无条件递增（编辑器直写路径，保持低延迟体验）。
///
/// 顺序保留 NB-31 契约：先写文件（真相源），成功后才动 DB——写失败不推进版本。
pub fn write_body(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    id: &str,
    new_body: &str,
    expected_version: Option<i64>,
) -> Result<i64, DocError> {
    let _guard = document_lock(id);
    type MetaRow = (Option<String>, String, String, String, Option<String>, i64);
    let meta: Option<MetaRow> = conn
        .query_row(
            "SELECT item_id, title, article_type, created_at, prompt_version, version
             FROM articles WHERE id = ?1",
            rusqlite::params![id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| DocError::Db(e.to_string()))?;
    let Some((item_id, title, article_type, created_at, prompt_version, version)) = meta else {
        return Err(DocError::NotFound(id.to_string()));
    };
    if let Some(expected) = expected_version {
        if version != expected {
            return Err(DocError::VersionConflict {
                expected,
                actual: version,
            });
        }
    }
    let article = Article {
        id: id.to_string(),
        item_id,
        title,
        content: new_body.to_string(),
        article_type,
        edited: true,
        created_at,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        prompt_version,
        blocks_json: None,
    };
    crate::notes::write_article_file_in(notes_dir, &article)
        .map_err(|e| DocError::Io(e.to_string()))?;
    conn.execute(
        "UPDATE articles SET content = '', edited = 1, updated_at = CURRENT_TIMESTAMP,
                            version = version + 1 WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| DocError::Db(e.to_string()))?;
    current_version(conn, id)?.ok_or_else(|| DocError::Db(format!("version 回读失败: {id}")))
}

/// 建文档（锁内查重 → 先文件后 DB，同 notes::insert_article 落盘顺序），version 起步 = 1。
/// 重复 id → Db 错误：持有单文档锁内先查行再写文件——INSERT 无 OR REPLACE，
/// 且绝不能先落盘后才发现重复（那会用新正文静默覆盖既有 .md）。
pub fn create_document(
    conn: &rusqlite::Connection,
    notes_dir: &Path,
    id: &str,
    title: &str,
    article_type: &str,
    body: &str,
) -> Result<i64, DocError> {
    let _guard = document_lock(id);
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM articles WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .map_err(|e| DocError::Db(e.to_string()))?;
    if exists > 0 {
        return Err(DocError::Db(format!("文档已存在，禁止覆盖: {id}")));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let article = Article {
        id: id.to_string(),
        item_id: None,
        title: title.to_string(),
        content: body.to_string(),
        article_type: article_type.to_string(),
        edited: false,
        created_at: now.clone(),
        updated_at: None,
        prompt_version: None,
        blocks_json: None,
    };
    crate::notes::write_article_file_in(notes_dir, &article)
        .map_err(|e| DocError::Io(e.to_string()))?;
    conn.execute(
        "INSERT INTO articles (id, item_id, title, content, article_type, edited, created_at, version)
         VALUES (?1, NULL, ?2, '', ?3, 0, ?4, 1)",
        rusqlite::params![id, title, article_type, now],
    )
    .map_err(|e| DocError::Db(e.to_string()))?;
    Ok(1)
}

/// FNV-1a 64（与 §3.3 item_contents.content_hash 同算法口径）
pub fn content_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

#[cfg(test)]
pub mod tests {
    //! 零模型夹具：临时目录 + create_schema 建库（含 AG-24 文档域表）。
    //! pub：service.rs 与 tools/documents.rs 的测试模块复用同一夹具（单一口径）。
    use super::*;

    pub(crate) struct RepoFixture {
        pub dir: std::path::PathBuf,
        pub db_path: std::path::PathBuf,
        pub notes: std::path::PathBuf,
    }

    impl RepoFixture {
        pub(crate) fn setup(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sophonote-ag24-repo-{}-{}-{}",
                tag,
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let notes = dir.join("notes");
            std::fs::create_dir_all(&notes).unwrap();
            let db_path = dir.join("sophonote.db");
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            crate::db::create_schema(&conn).unwrap();
            Self {
                dir,
                db_path,
                notes,
            }
        }

        pub(crate) fn conn(&self) -> rusqlite::Connection {
            rusqlite::Connection::open(&self.db_path).unwrap()
        }

        /// 种子一篇「文件在盘」的文档（version 默认 1）
        pub(crate) fn seed_article(&self, id: &str, title: &str, body: &str) {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO articles (id, title, content, article_type, version)
                 VALUES (?1, ?2, '', 'manual', 1)",
                rusqlite::params![id, title],
            )
            .unwrap();
            std::fs::write(
                self.notes.join(format!("{id}.md")),
                format!("---\nid: {id}\ntitle: \"{title}\"\ntype: manual\n---\n\n{body}"),
            )
            .unwrap();
        }
    }

    impl Drop for RepoFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn get_document_returns_body_and_version() {
        let fx = RepoFixture::setup("get");
        fx.seed_article("a1", "测试", "第一行\n第二行");
        let conn = fx.conn();
        let rec = get_document(&conn, &fx.notes, "a1").unwrap().expect("存在");
        assert_eq!(rec.title, "测试");
        assert_eq!(rec.body, "第一行\n第二行");
        assert_eq!(rec.version, 1);
        assert!(rec.file_exists);
        assert!(get_document(&conn, &fx.notes, "ghost").unwrap().is_none());
    }

    #[test]
    fn write_body_bumps_version_and_persists_file() {
        // 编辑器直写路径（expected=None）：文件落盘 + 版本 1→2 + 不丢字
        let fx = RepoFixture::setup("write");
        fx.seed_article("a1", "测试", "旧正文");
        let conn = fx.conn();
        let v = write_body(&conn, &fx.notes, "a1", "新正文", None).unwrap();
        assert_eq!(v, 2);
        let rec = get_document(&conn, &fx.notes, "a1").unwrap().unwrap();
        assert_eq!(rec.body, "新正文");
        assert_eq!(rec.version, 2);
        assert!(rec.edited);
    }

    #[test]
    fn write_body_cas_rejects_stale_version_and_keeps_content() {
        // CAS 防御：base 过期 → 冲突错误，文件与版本都不变（禁止静默覆盖）
        let fx = RepoFixture::setup("cas");
        fx.seed_article("a1", "测试", "原内容");
        let conn = fx.conn();
        let err = write_body(&conn, &fx.notes, "a1", "覆盖内容", Some(99)).unwrap_err();
        assert!(matches!(
            err,
            DocError::VersionConflict {
                expected: 99,
                actual: 1
            }
        ));
        assert!(err.to_string().contains("版本冲突"));
        let rec = get_document(&conn, &fx.notes, "a1").unwrap().unwrap();
        assert_eq!(rec.body, "原内容");
        assert_eq!(rec.version, 1);
        // base 匹配则成功
        let v = write_body(&conn, &fx.notes, "a1", "覆盖内容", Some(1)).unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn create_document_writes_file_row_version_one_and_rejects_duplicate() {
        let fx = RepoFixture::setup("create");
        let conn = fx.conn();
        let v = create_document(&conn, &fx.notes, "new-1", "新笔记", "manual", "hello").unwrap();
        assert_eq!(v, 1);
        let rec = get_document(&conn, &fx.notes, "new-1").unwrap().unwrap();
        assert_eq!(rec.body, "hello");
        assert!(rec.file_exists);
        // 重复 id 必须报错，不得静默覆盖
        assert!(create_document(&conn, &fx.notes, "new-1", "另一个", "manual", "x").is_err());
        let rec = get_document(&conn, &fx.notes, "new-1").unwrap().unwrap();
        assert_eq!(rec.body, "hello");
    }

    #[test]
    fn content_hash_is_stable_and_order_sensitive() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
        assert_eq!(content_hash("").len(), 16);
    }
}
