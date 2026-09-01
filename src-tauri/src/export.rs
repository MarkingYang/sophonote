//! N7 笔记本批量导出（Track A · NB-02）：整本导出为 .md 文件夹，Obsidian 可直接打开且 [[双链]] 可用。
//!
//! 定位（docs/architecture.md 轨道 A 所有权）：新增只读扩展文件——
//! DB 与 notes/ 目录只读，仅向用户指定的导出目录写入；不碰 notes.rs 写路径、无 schema 变更、无模型调用。
//!
//! Obsidian 兼容策略：
//! - 文件名 = 笔记标题（文件系统非法字符替换为 `-`），vault 内 `[[标题]]` 按文件名直接解析
//! - 保留 frontmatter（id/title/type/item_id/created/updated，合法 YAML = Obsidian properties）
//! - 标题因清洗改名时同步改写正文 `[[旧标题]]`；同名冲突加数字后缀（与 Obsidian 新建行为一致）
//! - N1 沉淀的 `sophonote:item/<id>` 内部反链改写为原条目外链（携带原 url），Obsidian 不留死链
//! - 正文引用的图片从 assets/ 拷贝随迁，`assets/<name>` 相对引用保持有效

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tauri::AppHandle;

use regex::Regex;
use serde::Serialize;

use crate::commands::ApiResponse;
use crate::db::get_db_path;

#[derive(Serialize)]
pub struct ExportReport {
    /// 实际导出目录（前端用它唤起 Finder）
    pub dir: String,
    /// 导出笔记篇数（manual + journal）
    pub notes: usize,
    /// 随迁资产（图片）数
    pub assets: usize,
}

struct NoteRow {
    id: String,
    item_id: Option<String>,
    title: String,
    article_type: String,
    created_at: String,
    updated_at: Option<String>,
}

/// 文件系统非法字符替换（覆盖 macOS 与 Obsidian 的禁用集）；结果全空兜底「未命名」
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            other => other,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim().to_string();
    if trimmed.is_empty() {
        "未命名".to_string()
    } else {
        trimmed
    }
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 导出用 frontmatter（格式与 notes.rs 一致；去掉内部字段 prompt_version）
fn frontmatter(n: &NoteRow) -> String {
    let mut fm = String::from("---\n");
    fm.push_str(&format!("id: {}\n", n.id));
    fm.push_str(&format!("title: {}\n", yaml_quote(&n.title)));
    fm.push_str(&format!("type: {}\n", n.article_type));
    if let Some(item_id) = &n.item_id {
        fm.push_str(&format!("item_id: {}\n", item_id));
    }
    fm.push_str(&format!("created: {}\n", yaml_quote(&n.created_at)));
    if let Some(u) = &n.updated_at {
        fm.push_str(&format!("updated: {}\n", yaml_quote(u)));
    }
    fm.push_str("---\n\n");
    fm
}

/// 条目反链表（sophonote:item 反链改写用），整本/单篇导出共用（NB-13 抽取）
fn load_items_map(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, (String, Option<String>)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, title, url FROM items")
        .map_err(|e| e.to_string())?;
    // E0597 防御（CLAUDE.md 同款）：尾表达式临时值析构晚于 stmt，
    // 统一「先 let 再返回」两段式
    let map: HashMap<String, (String, Option<String>)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?),
            ))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(map)
}

/// 正文导出改写：sophonote:item 内部反链 → 原条目外链（条目已删保留原文不丢信息），顺带收集 assets/<name> 引用。整本/单篇共用（NB-13 抽取）
fn rewrite_body_for_export(
    body: &str,
    items: &HashMap<String, (String, Option<String>)>,
    asset_refs: &mut HashSet<String>,
) -> String {
    let backlink_re = Regex::new(r"\[[^\]]*\]\(sophonote:item/([^)]+)\)").expect("regex");
    let asset_re = Regex::new(r"assets/[A-Za-z0-9._-]+").expect("regex");
    let body = backlink_re
        .replace_all(body, |caps: &regex::Captures| {
            let id = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            match items.get(id) {
                Some((title, Some(url))) => {
                    let text: String = title.chars().filter(|c| *c != '[' && *c != ']').collect();
                    format!("[↩ 原条目：{}]({})", text, url)
                }
                Some((title, None)) => {
                    let text: String = title.chars().filter(|c| *c != '[' && *c != ']').collect();
                    format!("↩ 原条目：{}", text)
                }
                // 条目已删：保留原文，不丢信息
                None => caps
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            }
        })
        .to_string();
    for m in asset_re.find_iter(&body) {
        asset_refs.insert(m.as_str().to_string());
    }
    body
}

/// 标题因清洗/去重改名时改写自身正文的 [[旧]] / [[旧|别名]] / [[旧#标题]] 三形态（NB-10 语义，NB-13 抽取）
fn rewrite_self_links(body: &mut String, from: &str, to: &str) {
    *body = body.replace(&format!("[[{}]]", from), &format!("[[{}]]", to));
    *body = body.replace(&format!("[[{}|", from), &format!("[[{}|", to));
    *body = body.replace(&format!("[[{}#", from), &format!("[[{}#", to));
}

/// 笔记本整体导出。target_dir 为空时默认导出到 `~/Desktop/SophoNote-Notebook-<时间戳>/`
#[tauri::command]
pub fn export_notebook(app: AppHandle, target_dir: Option<String>) -> ApiResponse<ExportReport> {
    match export_notebook_inner(&app, target_dir) {
        Ok(report) => ApiResponse::ok(report),
        Err(e) => ApiResponse::err(e),
    }
}

fn export_notebook_inner(
    app: &AppHandle,
    target_dir: Option<String>,
) -> Result<ExportReport, String> {
    // —— 1. 导出目录：显式传入优先，否则桌面新建时间戳文件夹 ——
    let target = match target_dir
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        Some(p) => p,
        None => {
            let home = std::env::var("HOME").map_err(|_| "无法读取 HOME 环境变量".to_string())?;
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            PathBuf::from(home)
                .join("Desktop")
                .join(format!("SophoNote-Notebook-{}", ts))
        }
    };
    std::fs::create_dir_all(&target).map_err(|e| format!("无法创建导出目录：{}", e))?;

    // 安全护栏：导出目标不得落在笔记存储目录内（避免污染在用存储）
    let notes_root = crate::notes::notes_dir(app);
    if let (Ok(t), Ok(n)) = (target.canonicalize(), notes_root.canonicalize()) {
        if t == n || t.starts_with(&n) {
            return Err("导出目标不能是笔记存储目录或其子目录".into());
        }
    }

    // —— 2. 只读取数：笔记本元数据（manual + journal）+ 条目反链用的 title/url ——
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, item_id, title, article_type, created_at, updated_at \
             FROM articles WHERE article_type IN ('manual','journal') ORDER BY created_at",
        )
        .map_err(|e| e.to_string())?;
    let notes: Vec<NoteRow> = stmt
        .query_map([], |r| {
            Ok(NoteRow {
                id: r.get(0)?,
                item_id: r.get(1)?,
                title: r.get(2)?,
                article_type: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    if notes.is_empty() {
        return Err("笔记本为空，没有可导出的笔记".into());
    }

    let items = load_items_map(&conn)?;

    // —— 3. 文件名 = 标题：清洗 + 去重（大小写不敏感，冲突加数字后缀）——
    let mut used: HashSet<String> = HashSet::new();
    let mut plan: Vec<(&NoteRow, String)> = Vec::with_capacity(notes.len());
    // 改名对照（旧标题 → 新文件名）：正文 [[旧标题]] 需要同步改写时才有条目
    let mut rewrite_pairs: Vec<(String, String)> = Vec::new();
    for n in &notes {
        let base = sanitize_filename(&n.title);
        let mut name = base.clone();
        let mut k = 2usize;
        while used.contains(&name.to_lowercase()) {
            name = format!("{} {}", base, k);
            k += 1;
        }
        used.insert(name.to_lowercase());
        if n.title != name {
            rewrite_pairs.push((n.title.clone(), name.clone()));
        }
        plan.push((n, name));
    }
    // 长标题先替换，避免短标题前缀遮蔽（如「AI」先于「AI Agent」误匹配——字面 [[..]] 虽已精确，仍防御）
    rewrite_pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));

    // —— 4. 逐篇写 .md：改写正文（条目反链 → 外链；改名标题的 [[双链]]）+ 收集资产引用 ——
    let mut asset_refs: HashSet<String> = HashSet::new();
    let mut written = 0usize;

    for (n, name) in &plan {
        // 正文真相源 = .md 文件（剥 frontmatter）；文件缺失兜底空正文（元数据仍导出）
        let raw = crate::notes::read_article_body(app, &n.id).unwrap_or_default();
        // 4a. sophonote:item 反链 → 原条目外链 + 收集资产引用（NB-13 抽取共享）
        let mut body = rewrite_body_for_export(&raw, &items, &mut asset_refs);
        // 4b. 改名标题的 [[双链]] 同步改写（NB-10 三形态，Obsidian 导出后仍可解析）
        for (from, to) in &rewrite_pairs {
            rewrite_self_links(&mut body, from, to);
        }

        let mut out = frontmatter(n);
        out.push_str(body.trim_end());
        out.push('\n');
        let path = target.join(format!("{}.md", name));
        std::fs::write(&path, out).map_err(|e| format!("写入 {} 失败：{}", path.display(), e))?;
        written += 1;
    }

    // —— 5. 资产随迁：仅拷贝被引用文件，assets/<name> 相对路径保持有效 ——
    let assets_src = notes_root.join("assets");
    let assets_dst = target.join("assets");
    let mut copied = 0usize;
    if !asset_refs.is_empty() {
        std::fs::create_dir_all(&assets_dst).map_err(|e| e.to_string())?;
        for rel in &asset_refs {
            let name = rel.trim_start_matches("assets/");
            let src = assets_src.join(name);
            if src.is_file() && std::fs::copy(&src, assets_dst.join(name)).is_ok() {
                copied += 1;
            }
        }
    }

    println!(
        "[export] notebook exported: notes={}, assets={}, dir={}",
        written,
        copied,
        target.display()
    );
    Ok(ExportReport {
        dir: target.display().to_string(),
        notes: written,
        assets: copied,
    })
}

/// NB-13 单篇导出报告
#[derive(Serialize)]
pub struct SingleExportReport {
    /// 导出的 .md 完整路径（前端用于展示/在访达中显示）
    pub path: String,
    /// 随迁资产数
    pub assets: usize,
}

/// NB-13：单篇导出（三空间右键菜单入口）。
///
/// 正文真相源：笔记（manual/journal）= .md 文件；其它类型（deep-dive 等）= DB content 列。
/// - 无资产引用：`~/Desktop/<标题>.md`
/// - 有资产引用：`~/Desktop/<标题>/<标题>.md` + `assets/` 随迁（自包含文件夹，相对路径有效）
/// - 同名冲突加数字后缀，不覆盖用户已有文件
/// - 指向其它笔记的 [[双链]] 保持原样（单篇不带出其它笔记，Obsidian 显未解析链接）
#[tauri::command]
pub fn export_article(app: AppHandle, article_id: String) -> ApiResponse<SingleExportReport> {
    match export_article_inner(&app, &article_id) {
        Ok(report) => ApiResponse::ok(report),
        Err(e) => ApiResponse::err(e),
    }
}

fn export_article_inner(app: &AppHandle, article_id: &str) -> Result<SingleExportReport, String> {
    // —— 1. 元数据 + DB 正文（正文优先 .md 文件，DB 列作 deep-dive 等无文件类型的真相源）——
    let conn = rusqlite::Connection::open(get_db_path(app)).map_err(|e| e.to_string())?;
    let (row, db_content) = conn
        .query_row(
            "SELECT id, item_id, title, article_type, created_at, updated_at, content \
             FROM articles WHERE id = ?1",
            [article_id],
            |r| {
                Ok((
                    NoteRow {
                        id: r.get(0)?,
                        item_id: r.get(1)?,
                        title: r.get(2)?,
                        article_type: r.get(3)?,
                        created_at: r.get(4)?,
                        updated_at: r.get(5)?,
                    },
                    r.get::<_, String>(6)?,
                ))
            },
        )
        .map_err(|_| "文章不存在（可能已删除）".to_string())?;

    // —— 2. 正文改写：条目反链 → 外链 + 收集资产引用 ——
    let raw = crate::notes::read_article_body(app, &row.id).unwrap_or(db_content);
    let items = load_items_map(&conn)?;
    let mut asset_refs: HashSet<String> = HashSet::new();
    let mut body = rewrite_body_for_export(&raw, &items, &mut asset_refs);

    // —— 3. 导出落点：桌面；文件名 = 标题清洗，同名加数字后缀不覆盖 ——
    let home = std::env::var("HOME").map_err(|_| "无法读取 HOME 环境变量".to_string())?;
    let desktop = PathBuf::from(home).join("Desktop");
    std::fs::create_dir_all(&desktop).map_err(|e| format!("无法访问桌面目录：{}", e))?;
    let base = sanitize_filename(&row.title);
    let mut k = 2usize;

    let (md_path, assets_dst) = if asset_refs.is_empty() {
        let mut name = base.clone();
        let mut p = desktop.join(format!("{}.md", name));
        while p.exists() {
            name = format!("{} {}", base, k);
            k += 1;
            p = desktop.join(format!("{}.md", name));
        }
        if row.title != name {
            rewrite_self_links(&mut body, &row.title, &name);
        }
        (p, None)
    } else {
        let mut name = base.clone();
        let mut d = desktop.join(&name);
        while d.exists() {
            name = format!("{} {}", base, k);
            k += 1;
            d = desktop.join(&name);
        }
        if row.title != name {
            rewrite_self_links(&mut body, &row.title, &name);
        }
        (d.join(format!("{}.md", name)), Some(d.join("assets")))
    };

    // —— 4. 写 .md + 资产随迁 ——
    if let Some(dst) = &assets_dst {
        std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    }
    let mut out = frontmatter(&row);
    out.push_str(body.trim_end());
    out.push('\n');
    std::fs::write(&md_path, out).map_err(|e| format!("写入 {} 失败：{}", md_path.display(), e))?;

    let mut copied = 0usize;
    if let Some(dst) = assets_dst {
        let assets_src = crate::notes::notes_dir(app).join("assets");
        for rel in &asset_refs {
            let n = rel.trim_start_matches("assets/");
            let src = assets_src.join(n);
            if src.is_file() && std::fs::copy(&src, dst.join(n)).is_ok() {
                copied += 1;
            }
        }
    }

    println!(
        "[export] article exported: path={}, assets={}",
        md_path.display(),
        copied
    );
    Ok(SingleExportReport {
        path: md_path.display().to_string(),
        assets: copied,
    })
}
