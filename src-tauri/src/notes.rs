//! 笔记文件存储层（N0）：.md 文件为唯一真相源，SQLite 降为索引。
//!
//! 目录布局：`<app_data_dir>/notes/<id>.md` + `<app_data_dir>/notes/assets/<uuid>.<ext>`
//!
//! 设计要点（2026-08-07 决策备案，见 PRD §五）：
//! - 文件 = frontmatter（元数据，v1 只写不解析）+ 正文；元数据读取仍走 DB 索引
//! - 增/存/改名/删同步落文件；`db_get_articles` 读文件回填 content（文件缺失时回退 DB，迁移期兜底）
//! - 图片一律 `assets/` 相对路径落盘，不再以 data URL 内联进正文（解决已知债 #5）
//! - 手动新建 / 夜间解读统一走 `insert_article` 单一入口
//! - 原子写：tmp + rename，避免半截文件

use std::path::{Path, PathBuf};
use tauri::AppHandle;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use regex::Regex;
use rusqlite::OptionalExtension;

use crate::commands::ApiResponse;
use crate::db::Article;

/// 笔记根目录（与 sophonote.db 同级）
pub fn notes_dir(app: &AppHandle) -> PathBuf {
    crate::storage_layout::StorageLayout::resolve(app)
        .expect("Failed to resolve SophoNote storage layout")
        .notes
}

fn assets_dir(app: &AppHandle) -> PathBuf {
    notes_dir(app).join("assets")
}

/// id 来自 uuid / crypto.randomUUID；仍防御性过滤路径敏感字符。
/// AG-24：升级为 pub(crate)——DocumentService 的唯一临时文件命名复用同一清洗口径
pub(crate) fn safe_article_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn article_path(app: &AppHandle, id: &str) -> PathBuf {
    notes_dir(app).join(format!("{}.md", safe_article_id(id)))
}

fn ensure_dirs(app: &AppHandle) -> std::io::Result<()> {
    std::fs::create_dir_all(notes_dir(app))?;
    std::fs::create_dir_all(assets_dir(app))
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn frontmatter(a: &Article) -> String {
    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {}\n", a.id));
    fm.push_str(&format!("title: {}\n", yaml_quote(&a.title)));
    fm.push_str(&format!("type: {}\n", a.article_type));
    if let Some(item_id) = &a.item_id {
        fm.push_str(&format!("item_id: {}\n", item_id));
    }
    fm.push_str(&format!("created: {}\n", yaml_quote(&a.created_at)));
    if let Some(u) = &a.updated_at {
        fm.push_str(&format!("updated: {}\n", yaml_quote(u)));
    }
    if let Some(pv) = &a.prompt_version {
        fm.push_str(&format!("prompt_version: {}\n", yaml_quote(pv)));
    }
    fm.push_str("---\n\n");
    fm
}

/// 剥掉文件头 frontmatter（容忍无 frontmatter 的文件，原样返回）
fn strip_frontmatter(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let body = &rest[end + 5..];
            // 与写端 frontmatter() 的 "---\n\n" 分隔对齐：吃掉一个前导空行，
            // 保证写盘→读回逐字节等价（AG-23 往返测试首次暴露；此前读回恒多
            // 一个前导换行，渲染不可见但属保真缺陷）
            return body.strip_prefix('\n').unwrap_or(body);
        }
    }
    text
}

fn atomic_write(path: &PathBuf, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// 写笔记文件（frontmatter + 正文）。Article.content 即正文
///
/// AG-24 重构为三层（Phase 3 DocumentRepository 复用，命令签名与落盘口径零变化）：
/// - write_article_to：向任意路径写「frontmatter+正文」（DocumentService 的唯一临时
///   文件写入用——临时文件名带 operation ID，不用固定 .md.tmp，docs/architecture.md）；
/// - write_article_file_in：目录参数版（单测可直接驱动，同 read_article_body_in 模式）；
/// - write_article_file：AppHandle 包装（既有调用方不动）。
pub(crate) fn write_article_to(path: &Path, article: &Article) -> std::io::Result<()> {
    let body = strip_frontmatter(&article.content);
    atomic_write(&path.to_path_buf(), &(frontmatter(article) + body))
}

/// 写笔记文件的目录参数版（AG-24，§3.9 规则 1：notes.rs 写路径归轨道 B 演进）
pub fn write_article_file_in(dir: &Path, article: &Article) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_article_to(
        &dir.join(format!("{}.md", safe_article_id(&article.id))),
        article,
    )
}

pub fn write_article_file(app: &AppHandle, article: &Article) -> std::io::Result<()> {
    ensure_dirs(app)?;
    write_article_file_in(&notes_dir(app), article)
}

/// 读正文（剥 frontmatter）；文件不存在返回 None
pub fn read_article_body(app: &AppHandle, id: &str) -> Option<String> {
    read_article_body_in(&notes_dir(app), id)
}

/// 读正文（剥 frontmatter）的目录参数版（AG-19：供 Track B 项目只读工具复用，
/// §3.9 规则 8 跨轨道最小改动）。与 article_path 共用 safe_article_id 清洗口径，
/// 保证文件定位单一标准；无 AppHandle 依赖，可在单测中直接驱动。
pub fn read_article_body_in(dir: &Path, id: &str) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(format!("{}.md", safe_article_id(id)))).ok()?;
    Some(strip_frontmatter(&text).to_string())
}

/// 按 itemId 精确读取最新一篇指定类型文章，并从 Markdown 真相源回填正文。
/// 用于发现详情等按需读取场景，不能受 `db_get_articles(limit)` 的列表缓存上限影响。
pub fn load_latest_article_for_item(
    conn: &rusqlite::Connection,
    notes_root: &Path,
    item_id: &str,
    article_type: &str,
) -> Result<Option<Article>, String> {
    let mut article = conn
        .query_row(
            "SELECT id, item_id, title, content, article_type, edited, created_at, updated_at, prompt_version, blocks_json
             FROM articles
             WHERE item_id = ?1 AND article_type = ?2
             ORDER BY COALESCE(updated_at, created_at) DESC, created_at DESC
             LIMIT 1",
            rusqlite::params![item_id, article_type],
            |row| {
                Ok(Article {
                    id: row.get(0)?,
                    item_id: row.get(1)?,
                    title: row.get(2)?,
                    content: row.get(3)?,
                    article_type: row.get(4)?,
                    edited: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    prompt_version: row.get(8)?,
                    blocks_json: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some(article) = article.as_mut() {
        if let Some(body) = read_article_body_in(notes_root, &article.id) {
            article.content = body;
        }
    }
    Ok(article)
}

pub fn delete_article_file(app: &AppHandle, id: &str) {
    let _ = std::fs::remove_file(article_path(app, id));
}

/// 改名时同步文件 frontmatter 标题行（尽力而为：失败仅影响文件自描述，不影响应用读写）
pub fn rename_article_file(app: &AppHandle, id: &str, new_title: &str) {
    let path = article_path(app, id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    if !text.starts_with("---\n") {
        return;
    }
    let ends_nl = text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let end = lines.iter().position(|l| l == "---").filter(|&i| i > 0);
    let Some(end) = end else { return };
    for l in lines.iter_mut().take(end).skip(1) {
        if l.starts_with("title: ") {
            *l = format!("title: {}", yaml_quote(new_title));
            break;
        }
    }
    let mut out = lines.join("\n");
    if ends_nl {
        out.push('\n');
    }
    let _ = atomic_write(&path, &out);
}

/// 统一新建入口（手动 / 夜间解读）：先文件后 DB 索引，DB 不再存正文
pub fn insert_article(app: &AppHandle, article: &Article) -> Result<(), String> {
    write_article_file(app, article).map_err(|e| e.to_string())?;
    let conn =
        rusqlite::Connection::open(crate::db::get_db_path(app)).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO articles (id, item_id, title, content, article_type, edited, created_at, prompt_version, blocks_json)
         VALUES (?1, ?2, ?3, '', ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            article.id,
            article.item_id,
            article.title,
            article.article_type,
            article.edited as i32,
            article.created_at,
            article.prompt_version,
            article.blocks_json,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn ext_from_mime(sub: &str) -> Option<&'static str> {
    match sub.to_lowercase().as_str() {
        "png" => Some("png"),
        "jpeg" | "jpg" => Some("jpg"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "svg+xml" => Some("svg"),
        _ => None,
    }
}

/// data URL 解码落盘 assets/，返回 Markdown 相对路径 `assets/<name>`；不支持的格式返回 None
fn store_data_url(app: &AppHandle, data_url: &str) -> Option<String> {
    // AG-23（审计 P1-3 性能项）：保存路径按图片调用，Regex 静态化避免重复编译。
    // 注意只用 get_or_init（宿主 rustc 旧，禁 get_or_try_init）
    static DATA_URL_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = DATA_URL_RE.get_or_init(|| {
        Regex::new(r"^data:image/(?P<sub>[A-Za-z0-9.+-]+);base64,(?P<b64>[A-Za-z0-9+/=\s]+)$")
            .expect("static regex")
    });
    let caps = re.captures(data_url.trim())?;
    let ext = ext_from_mime(&caps["sub"])?;
    let cleaned: String = caps["b64"].chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD.decode(cleaned).ok()?;
    let name = format!("{}.{}", uuid::Uuid::new_v4(), ext);
    std::fs::write(assets_dir(app).join(&name), &bytes).ok()?;
    Some(format!("assets/{}", name))
}

/// 启动迁移（N0）：DB content → .md 文件；data URL 图片抽出的落盘；DB 正文字段清空。
/// 幂等：文件已存在且 DB 已清空则跳过；文件与 DB 并存时文件为真相、只清 DB。
pub fn migrate_articles_to_files(app: &AppHandle) {
    if let Err(e) = ensure_dirs(app) {
        eprintln!("[notes] migration aborted: cannot create notes dir: {}", e);
        return;
    }
    let Ok(conn) = rusqlite::Connection::open(crate::db::get_db_path(app)) else {
        return;
    };
    // 注意：不能用 prepare().and_then(|stmt| stmt.query_map(...))——MappedRows 借用 stmt，
    // 无法从闭包返回（E0515）；match 绑定 stmt 后在同一表达式内 collect
    let articles: Vec<Article> = match conn.prepare(
        "SELECT id, item_id, title, content, article_type, edited, created_at, updated_at, prompt_version, blocks_json FROM articles",
    ) {
        Ok(mut stmt) => stmt
            .query_map([], |r| {
                Ok(Article {
                    id: r.get(0)?,
                    item_id: r.get(1)?,
                    title: r.get(2)?,
                    content: r.get(3)?,
                    article_type: r.get(4)?,
                    edited: r.get::<_, i32>(5)? != 0,
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                    prompt_version: r.get(8)?,
                    blocks_json: r.get(9)?,
                })
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    // 存量 data URL 内联图片（新内容走 save_note_asset，不会再产生此形态）
    let inline_img = Regex::new(
        r"!\[(?P<alt>[^\]]*)\]\((?P<url>data:image/[A-Za-z0-9.+-]+;base64,[A-Za-z0-9+/=\s]+)\)",
    )
    .expect("regex");

    let (mut files_written, mut images_extracted, mut db_cleared) = (0usize, 0usize, 0usize);
    for a in &articles {
        let path = article_path(app, &a.id);
        if path.exists() {
            // 文件已是真相：仅清 DB 残留正文
            if !a.content.trim().is_empty() {
                let _ = conn.execute(
                    "UPDATE articles SET content='' WHERE id=?1",
                    rusqlite::params![a.id],
                );
                db_cleared += 1;
            }
            continue;
        }
        // 文件不存在：从 DB 正文写文件（空正文也落文件，保证「新旧笔记均走文件」）
        // data URL 图片抽出的落盘
        let mut body = String::new();
        let mut last = 0usize;
        for caps in inline_img.captures_iter(&a.content) {
            let m = caps.name("url").unwrap();
            body.push_str(&a.content[last..m.start()]);
            match store_data_url(app, m.as_str()) {
                Some(rel) => {
                    body.push_str(&format!("![{}]({})", &caps["alt"], rel));
                    images_extracted += 1;
                }
                None => body.push_str(m.as_str()), // 解码失败保留原样，不丢数据
            }
            last = m.end();
        }
        body.push_str(&a.content[last..]);

        let mut file_article = a.clone();
        file_article.content = body;
        match write_article_file(app, &file_article) {
            Ok(()) => {
                files_written += 1;
                let _ = conn.execute(
                    "UPDATE articles SET content='' WHERE id=?1",
                    rusqlite::params![a.id],
                );
                db_cleared += 1;
            }
            Err(e) => eprintln!("[notes] migration: write failed for {}: {}", a.id, e),
        }
    }
    println!(
        "[notes] migration: articles={}, files_written={}, images_extracted={}, db_cleared={}",
        articles.len(),
        files_written,
        images_extracted,
        db_cleared
    );
}

/// 粘贴/拖拽图片落盘（前端传 data URL），返回 Markdown 使用的相对路径 `assets/<name>`
#[tauri::command]
pub fn save_note_asset(app: AppHandle, data_url: String) -> ApiResponse<String> {
    match store_data_url(&app, &data_url) {
        Some(rel) => ApiResponse::ok(rel),
        None => ApiResponse::err(
            "无法识别的图片编码（支持 png/jpeg/gif/webp/svg 的 base64 data URL）".into(),
        ),
    }
}

/// 读取资产为 data URL（预览/编辑渲染用）。路径白名单：仅 `assets/` 前缀，拒绝 `..` 穿越
#[tauri::command]
pub fn read_note_asset(app: AppHandle, rel_path: String) -> ApiResponse<Option<String>> {
    let rel = rel_path.trim_start_matches("./");
    if !rel.starts_with("assets/") || rel.contains("..") {
        return ApiResponse::err("非法资产路径".into());
    }
    let path = notes_dir(&app).join(rel);
    let Ok(bytes) = std::fs::read(&path) else {
        return ApiResponse::ok(None);
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    ApiResponse::ok(Some(format!(
        "data:{};base64,{}",
        mime,
        STANDARD.encode(&bytes)
    )))
}

#[cfg(test)]
mod tests {
    //! AG-23：笔记落盘失败链路自动化（取代人工 chmod 走查）。
    //! 背景：写入走 atomic_write（临时文件 + rename），rename 只依赖「目录」写权限，
    //! 锁单个文件权限无法制造失败——真实故障注入点是目录只读。
    //! 链路其余环节已有测试：fs Err → ApiResponse::err（db_update_article 直映射），
    //! 前端收 error → 基线不推进、内容留在编辑器（noteSave.nb31 / articleWrites.nb31）。

    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sophonote-notes-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(content: &str) -> crate::db::Article {
        crate::db::Article {
            id: "note-1".into(),
            item_id: None,
            title: "标题".into(),
            content: content.into(),
            article_type: "note".into(),
            edited: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: None,
            prompt_version: None,
            blocks_json: None,
        }
    }

    #[test]
    fn atomic_write_success_leaves_no_tmp_residue() {
        let dir = tmp_dir();
        let path = dir.join("a.md");
        atomic_write(&path, "v1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
        // 目录里只有目标文件：临时文件已被 rename 消费
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_failure_keeps_content_and_retry_recovers() {
        // NB-31「失败重试不丢字」的 Rust 侧真相：写入失败时旧内容完整保留，
        // 故障解除后重试成功落盘——等价于人工走查 chmod 场景且可回归
        let dir = tmp_dir();
        let path = dir.join("a.md");
        atomic_write(&path, "v1-旧内容").unwrap();

        // 目录只读 → 临时文件无法创建 → 写入必须失败（模拟磁盘/权限故障）
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let failed = atomic_write(&path, "v2-新内容");
        // 先恢复权限再断言：即使断言失败也不影响清理
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(failed.is_err(), "只读目录下写入必须失败");

        // 不丢字：磁盘仍是旧内容，且没有留下半成品 tmp
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1-旧内容");
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);

        // 故障解除后重试成功
        atomic_write(&path, "v2-新内容").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2-新内容");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frontmatter_round_trip_preserves_body() {
        let body = "# 标题\n\n中文正文不丢字，含特殊字符 \"引号\" 与 ---";
        let file_text = frontmatter(&fixture(body)) + strip_frontmatter(body);
        // 写盘格式读回后正文逐字节还原
        assert_eq!(strip_frontmatter(&file_text), body);
        // 无 frontmatter 的文件原样容忍
        assert_eq!(strip_frontmatter("plain text"), "plain text");
    }

    #[test]
    fn safe_article_id_strips_path_traversal() {
        let cleaned = safe_article_id("../../etc/passwd");
        assert!(!cleaned.contains('/'));
        assert!(!cleaned.contains('.'));
        assert_eq!(cleaned, "etcpasswd");
    }

    #[test]
    fn latest_deep_dive_is_loaded_exactly_by_item_id_from_markdown() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::create_schema(&conn).unwrap();
        let dir = tmp_dir();
        let old = Article {
            id: "deep-old".into(),
            item_id: Some("historical-item".into()),
            title: "旧解读".into(),
            content: "旧正文".into(),
            article_type: "deep-dive".into(),
            edited: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: None,
            prompt_version: Some("radar@v1".into()),
            blocks_json: None,
        };
        let mut latest = old.clone();
        latest.id = "deep-latest".into();
        latest.title = "最新解读".into();
        latest.content = "# 来自 Markdown 的完整深度正文".into();
        latest.created_at = "2026-08-18T00:00:00Z".into();

        conn.execute(
            "INSERT INTO sources (id, name, source_type) VALUES ('aihot', 'AIHOT', 'rss')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO items (id, source_id, item_type, title) VALUES ('historical-item', 'aihot', 'article', '历史条目')",
            [],
        )
        .unwrap();

        for article in [&old, &latest] {
            write_article_file_in(&dir, article).unwrap();
            conn.execute(
                "INSERT INTO articles (id, item_id, title, content, article_type, edited, created_at, prompt_version)
                 VALUES (?1, ?2, ?3, '', ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    article.id,
                    article.item_id,
                    article.title,
                    article.article_type,
                    article.edited as i32,
                    article.created_at,
                    article.prompt_version,
                ],
            )
            .unwrap();
        }

        let loaded = load_latest_article_for_item(&conn, &dir, "historical-item", "deep-dive")
            .unwrap()
            .expect("exact deep dive must exist");
        assert_eq!(loaded.id, "deep-latest");
        assert_eq!(loaded.content, latest.content);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
