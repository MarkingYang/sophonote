// ============================================================
// Track B · 智能体演进（AG-27 · Phase 4 Skill 系统）
// 实施基线：docs/architecture.md（Skill 系统与权限交集）
//
// 边界（见 docs/architecture.md）：
// - Skill 第一阶段只负责：触发说明、领域指令、输入输出约定、所需工具、默认预算；
//   不当成脚本系统、权限系统或工作流引擎（禁令 6/7）。
// - 权限不写在 Skill 里直接生效：Skill 只能声明「我需要哪些工具」，
//   有效工具集 = 交集（§七）。本模块的 split_effective() 即该交集的计算点；
//   运行时过滤在 agent/commands.rs 构造 ToolRegistry 时完成。
// - 三个来源：bundled（内置只读，include_str 编译期内嵌）/ user（用户级目录）/
//   workspace（当前知识库专用目录）；优先级 workspace > user > bundled（§八）。
// - 清单为受限 YAML frontmatter 子集（key: 标量 / key: + "- item" 列表），
//   零新增依赖（不引入 serde_yaml）；未知字段直接拒绝（schema 校验）。
// - 加载安全检查（§八 加载检查清单）：文件大小上限、frontmatter schema、
//   名称/工具名白名单正则、不跟随符号链接（直接跳过）。
// ============================================================
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};

use crate::commands::ApiResponse;

pub mod hermes_export;

pub use hermes_export::{export_skills_readonly_cache, HermesSkillExportEntry};

/// 单个清单文件的大小上限（§八「文件大小」检查）
pub const SKILL_MAX_FILE_BYTES: u64 = 64 * 1024;

/// 内置工具名全集（权限交集的「当前可用工具」侧，Phase 4 口径）。
/// 必须与 agent/commands.rs::project_registry 注册的真实工具保持一致——
/// 漂移由本文件底部 builtin_tool_names_match_project_registry 测试守护。
/// AG-28：MCP 授权工具不在此常量内（随连接态变化），由调用方并入，见 builtin_available。
pub const BUILTIN_TOOL_NAMES: &[&str] = &[
    "list_project_documents",
    "read_document",
    "create_document",
    "propose_document_patch",
    "move_document",
];

/// Skill 来源（§八 三个来源；序列化为小写串，前端直接展示徽标文案）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Bundled,
    User,
    Workspace,
}

impl SkillSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }

    /// 解析优先级：workspace > user > bundled（§八）
    fn priority(self) -> u8 {
        match self {
            Self::Bundled => 0,
            Self::User => 1,
            Self::Workspace => 2,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "bundled" => Ok(Self::Bundled),
            "user" => Ok(Self::User),
            "workspace" => Ok(Self::Workspace),
            other => Err(format!("未知 Skill 来源: {}", other)),
        }
    }
}

/// 执行形态（§八「Pipeline 与 Agent Skill 区分」）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillExecution {
    /// 模型决定工具顺序
    Agent,
    /// 确定步骤由 Rust 编排（编排引擎为后续阶段；当前可激活为对话技能）
    Workflow,
}

impl SkillExecution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Workflow => "workflow",
        }
    }

    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "agent" => Ok(Self::Agent),
            "workflow" => Ok(Self::Workflow),
            other => Err(format!(
                "execution 必须是 agent 或 workflow，得到 '{}'",
                other
            )),
        }
    }
}

/// Skill 清单（frontmatter 解析 + 校验后的结构；body = 领域指令正文）
#[derive(Debug, Clone, PartialEq)]
pub struct SkillManifest {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub execution: SkillExecution,
    /// 声明「我需要」的工具（不是授权——生效必须过交集，§七）
    pub tools: Vec<String>,
    /// 默认预算（§八）：模型调用上限（夹紧进 Run 的 max_turns）
    pub max_model_calls: Option<u32>,
    /// 默认预算：工具调用上限（驱动层强制执行）
    pub max_tool_calls: Option<u32>,
    /// 正文（领域指令；激活时注入系统提示 <skill> 块）
    pub body: String,
}

/// 加载结果（manifest = None 表示文件级解析失败；仍进列表供管理 UI 展示原因）
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub manifest: Option<SkillManifest>,
    pub source: SkillSource,
    /// 出处描述：bundled 为 "bundled:<内嵌名>"；目录来源为文件名
    pub origin: String,
    pub problems: Vec<String>,
}

impl LoadedSkill {
    /// 展示名：有效清单用其 name；解析失败退化为文件名主干（供 UI 定位问题文件）
    pub fn display_name(&self) -> String {
        match &self.manifest {
            Some(m) => m.name.clone(),
            None => self.origin.trim_end_matches(".md").to_string(),
        }
    }

    /// 可激活 = 清单解析校验通过（启用态与工具交集在激活时另行检查）
    pub fn available(&self) -> bool {
        self.manifest.is_some()
    }
}

// ---------------- 受限 frontmatter 解析（零新增依赖） ----------------

fn name_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9-]{1,63}$").expect("static regex"))
}

fn tool_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9_.-]{0,63}$").expect("static regex"))
}

const KNOWN_KEYS: &[&str] = &[
    "name",
    "version",
    "description",
    "execution",
    "tools",
    "max_model_calls",
    "max_tool_calls",
];

/// 拆分 `---` frontmatter 与正文
fn split_frontmatter(raw: &str) -> Result<(&str, &str), String> {
    let text = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let (first, tail) = text
        .split_once('\n')
        .ok_or_else(|| "空文件：缺少 frontmatter".to_string())?;
    if first.trim_end() != "---" {
        return Err("清单必须以 --- frontmatter 开头".into());
    }
    let mut offset = 0usize;
    for line in tail.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Ok((&tail[..offset], &tail[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err("frontmatter 未闭合（缺少结束 --- 行）".into())
}

/// 去掉成对引号（单/双）
fn unquote(v: &str) -> &str {
    let t = v.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        return &t[1..t.len() - 1];
    }
    t
}

/// 解析受限 frontmatter：`key: 标量` 与 `key:` + `  - item` 列表；
/// 未知字段/重复字段/缩进异常直接报错（schema 校验，§八）。
fn parse_frontmatter(fm: &str) -> Result<HashMap<String, Vec<String>>, String> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut list_key: Option<String> = None;

    for (idx, raw_line) in fm.lines().enumerate() {
        let line_no = idx + 2; // frontmatter 从文件第 2 行开始
        let line = raw_line.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // 列表项（处于列表上下文中）
        if let Some(key) = &list_key {
            if let Some(item) = trimmed.strip_prefix("- ") {
                let item = unquote(item.trim());
                if item.is_empty() {
                    return Err(format!("第 {} 行：空列表项", line_no));
                }
                out.get_mut(key)
                    .expect("list key registered")
                    .push(item.to_string());
                continue;
            }
            if trimmed == "-" {
                return Err(format!("第 {} 行：空列表项", line_no));
            }
            // 非列表项 → 结束列表上下文，按顶层键继续
            list_key = None;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(format!("第 {} 行：意外的缩进内容", line_no));
        }

        let Some(colon) = trimmed.find(':') else {
            return Err(format!("第 {} 行：无法解析（缺少冒号）", line_no));
        };
        let key = trimmed[..colon].trim().to_string();
        let value = trimmed[colon + 1..].trim();

        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!("第 {} 行：未知字段 '{}'", line_no, key));
        }
        if out.contains_key(&key) {
            return Err(format!("第 {} 行：字段 '{}' 重复", line_no, key));
        }

        if value.is_empty() {
            if key != "tools" {
                return Err(format!("第 {} 行：字段 '{}' 缺少值", line_no, key));
            }
            out.insert(key.clone(), Vec::new());
            list_key = Some(key);
        } else {
            if value.starts_with('[') {
                return Err(format!(
                    "第 {} 行：不支持行内数组，请用换行 '- ' 列表语法",
                    line_no
                ));
            }
            out.insert(key, vec![unquote(value).to_string()]);
        }
    }
    Ok(out)
}

fn take_scalar(map: &HashMap<String, Vec<String>>, key: &str) -> Result<Option<String>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(v) => {
            if v.len() != 1 {
                Err(format!("字段 '{}' 应为标量", key))
            } else {
                Ok(Some(v[0].clone()))
            }
        }
    }
}

/// 解析完整清单（frontmatter + body）。失败返回首条致命错误文本。
pub fn parse_manifest(raw: &str) -> Result<SkillManifest, String> {
    let (fm, body) = split_frontmatter(raw)?;
    let map = parse_frontmatter(fm)?;

    let name = take_scalar(&map, "name")?.ok_or_else(|| "缺少必填字段 name".to_string())?;
    if !name_re().is_match(&name) {
        return Err(format!(
            "name '{}' 不合法：仅小写字母/数字/连字符，2-64 位，字母开头",
            name
        ));
    }

    let version_raw =
        take_scalar(&map, "version")?.ok_or_else(|| "缺少必填字段 version".to_string())?;
    let version: u32 = version_raw
        .parse()
        .map_err(|_| format!("version '{}' 不是非负整数", version_raw))?;
    if version < 1 {
        return Err("version 必须 ≥ 1".into());
    }

    let description =
        take_scalar(&map, "description")?.ok_or_else(|| "缺少必填字段 description".to_string())?;
    if description.trim().is_empty() {
        return Err("description 不能为空".into());
    }
    if description.chars().count() > 200 {
        return Err("description 过长（上限 200 字符）".into());
    }

    let execution_raw =
        take_scalar(&map, "execution")?.ok_or_else(|| "缺少必填字段 execution".to_string())?;
    let execution = SkillExecution::parse(&execution_raw)?;

    let tools = map.get("tools").cloned().unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let tools: Vec<String> = tools
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .collect();
    for t in &tools {
        if !tool_re().is_match(t) {
            return Err(format!(
                "工具名 '{}' 不合法（小写字母开头，限小写字母/数字/_/./-）",
                t
            ));
        }
    }

    let max_model_calls = match take_scalar(&map, "max_model_calls")? {
        None => None,
        Some(v) => {
            let n: u32 = v
                .parse()
                .map_err(|_| format!("max_model_calls '{}' 不是非负整数", v))?;
            if !(1..=20).contains(&n) {
                return Err("max_model_calls 必须在 1-20 之间".into());
            }
            Some(n)
        }
    };
    let max_tool_calls = match take_scalar(&map, "max_tool_calls")? {
        None => None,
        Some(v) => {
            let n: u32 = v
                .parse()
                .map_err(|_| format!("max_tool_calls '{}' 不是非负整数", v))?;
            if !(1..=100).contains(&n) {
                return Err("max_tool_calls 必须在 1-100 之间".into());
            }
            Some(n)
        }
    };

    let body = body.trim().to_string();
    if body.is_empty() {
        return Err("清单正文（领域指令）不能为空".into());
    }

    Ok(SkillManifest {
        name,
        version,
        description,
        execution,
        tools,
        max_model_calls,
        max_tool_calls,
        body,
    })
}

// ---------------- 三层加载与解析（§八） ----------------

/// 内嵌的 bundled 清单（内置只读来源；文件名仅用于出处展示）
const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "research-note.md",
        include_str!("../../skills/research-note.md"),
    ),
    (
        "daily-picks.md",
        include_str!("../../skills/daily-picks.md"),
    ),
];

fn load_bundled() -> Vec<LoadedSkill> {
    BUNDLED_SKILLS
        .iter()
        .map(|(file, raw)| match parse_manifest(raw) {
            Ok(m) => LoadedSkill {
                manifest: Some(m),
                source: SkillSource::Bundled,
                origin: format!("bundled:{}", file),
                problems: Vec::new(),
            },
            Err(e) => LoadedSkill {
                manifest: None,
                source: SkillSource::Bundled,
                origin: format!("bundled:{}", file),
                problems: vec![e],
            },
        })
        .collect()
}

/// 扫描目录内的 *.md 清单（非递归；符号链接不跟随，直接跳过——§八 安全检查）
fn load_dir(dir: &Path, source: SkillSource) -> Vec<LoadedSkill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new(); // 目录不存在 = 该层无 Skill
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    files.sort_by_key(|e| e.file_name());

    let mut out = Vec::new();
    for entry in files {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();

        // 不跟随符号链接（symlink_metadata 不穿透；非常规文件同样跳过）
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    eprintln!("[skills] 跳过符号链接清单: {}", path.display());
                    continue;
                }
                if !meta.is_file() {
                    continue;
                }
                if meta.len() > SKILL_MAX_FILE_BYTES {
                    out.push(LoadedSkill {
                        manifest: None,
                        source,
                        origin: file_name,
                        problems: vec![format!(
                            "文件超过大小上限（{} KiB）",
                            SKILL_MAX_FILE_BYTES / 1024
                        )],
                    });
                    continue;
                }
            }
            Err(e) => {
                out.push(LoadedSkill {
                    manifest: None,
                    source,
                    origin: file_name,
                    problems: vec![format!("读取文件信息失败: {}", e)],
                });
                continue;
            }
        }

        match std::fs::read_to_string(&path) {
            Ok(raw) => match parse_manifest(&raw) {
                Ok(m) => out.push(LoadedSkill {
                    manifest: Some(m),
                    source,
                    origin: file_name,
                    problems: Vec::new(),
                }),
                Err(e) => out.push(LoadedSkill {
                    manifest: None,
                    source,
                    origin: file_name,
                    problems: vec![e],
                }),
            },
            Err(e) => out.push(LoadedSkill {
                manifest: None,
                source,
                origin: file_name,
                problems: vec![format!("读取失败: {}", e)],
            }),
        }
    }
    out
}

/// 汇总三层来源（bundled 恒在；user/workspace 目录可选）
pub fn collect_skills(user_dir: Option<&Path>, workspace_dir: Option<&Path>) -> Vec<LoadedSkill> {
    let mut all = load_bundled();
    if let Some(dir) = user_dir {
        all.extend(load_dir(dir, SkillSource::User));
    }
    if let Some(dir) = workspace_dir {
        all.extend(load_dir(dir, SkillSource::Workspace));
    }
    all
}

/// 解析（Resolver）：同名有效清单按 workspace > user > bundled 取高优先级者；
/// 无效条目（解析失败）不参与遮蔽，原样保留供 UI 展示问题。
/// 结果按展示名排序，保证列表稳定。
pub fn resolve_skills(loaded: Vec<LoadedSkill>) -> Vec<LoadedSkill> {
    let mut by_name: HashMap<String, LoadedSkill> = HashMap::new();
    let mut invalid: Vec<LoadedSkill> = Vec::new();

    for skill in loaded {
        if skill.manifest.is_none() {
            invalid.push(skill);
            continue;
        }
        let name = skill.display_name();
        match by_name.get(&name) {
            Some(existing) => {
                let ep = existing.source.priority();
                let np = skill.source.priority();
                // 同优先级（同一层两个同名文件）保留先出现者（文件名字典序），
                // 后者被丢弃——加载层已按文件名排序，结果确定。
                if np > ep {
                    by_name.insert(name, skill);
                }
            }
            None => {
                by_name.insert(name, skill);
            }
        }
    }

    let mut result: Vec<LoadedSkill> = by_name.into_values().collect();
    result.extend(invalid);
    result.sort_by_key(|s| s.display_name());
    result
}

// ---------------- 权限交集（§七：Skill 声明 ∩ 当前可用工具） ----------------

/// 当前可用内置工具集合（Phase 4 口径）。AG-28：调用方（agent_run_start /
/// skill_list）在此基础上并入「已连接且已授权」的 MCP 命名空间工具
/// （mcp.<server>.<tool>）参与 §七 权限交集——本函数仍只返回内置口径，
/// MCP 面由调用方动态注入（MCP 可用性随连接态变化，不是常量）。
pub fn builtin_available() -> BTreeSet<String> {
    BUILTIN_TOOL_NAMES.iter().map(|s| s.to_string()).collect()
}

/// 交集计算：返回（有效工具 = 声明 ∩ 可用，缺失工具 = 声明 − 可用）
pub fn split_effective(
    declared: &[String],
    available: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let mut effective = Vec::new();
    let mut missing = Vec::new();
    for t in declared {
        if available.contains(t) {
            effective.push(t.clone());
        } else {
            missing.push(t.clone());
        }
    }
    (effective, missing)
}

// ---------------- 启用态持久化（skill_state 表） ----------------

/// AG-27：skill_state 建表（幂等）。由 db::create_schema 调用。
pub fn create_skill_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS skill_state (
            name TEXT NOT NULL,
            source TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (name, source)
        );
        "#,
    )
    .map_err(|e| e.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 启用态查询：无记录 = 默认启用（用户把文件放进目录即视为安装并信任）
pub fn skill_enabled(
    conn: &rusqlite::Connection,
    name: &str,
    source: SkillSource,
) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT enabled FROM skill_state WHERE name = ?1 AND source = ?2")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params![name, source.as_str()])
        .map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => row
            .get::<_, i64>(0)
            .map(|v| v != 0)
            .map_err(|e| e.to_string()),
        None => Ok(true),
    }
}

/// 启用/停用（upsert）
pub fn set_skill_enabled(
    conn: &rusqlite::Connection,
    name: &str,
    source: SkillSource,
    enabled: bool,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO skill_state (name, source, enabled, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name, source) DO UPDATE SET enabled = ?3, updated_at = ?4",
        rusqlite::params![name, source.as_str(), if enabled { 1 } else { 0 }, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------- 目录与命令层 ----------------

/// 用户级 Skill 目录：`<app_data_dir>/skills/*.md`
pub fn user_skills_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("Failed to get app data dir")
        .join("skills")
}

/// 工作区（当前知识库专用）Skill 目录：`<app_data_dir>/skills-workspaces/<project_id>/*.md`
pub fn workspace_skills_dir(app: &AppHandle, project_id: &str) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("Failed to get app data dir")
        .join("skills-workspaces")
        .join(project_id)
}

/// 管理面板条目（camelCase 与前端对齐；无效清单 version=0/execution='invalid'）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub execution: String,
    pub source: SkillSource,
    pub origin: String,
    pub enabled: bool,
    /// 清单解析校验通过（false = 仅展示问题，不可启用/激活）
    pub available: bool,
    /// 清单声明的工具（「我需要」，不是授权）
    pub tools: Vec<String>,
    /// 权限交集结果：声明 ∩ 当前可用（§七；AG-28 起含 MCP 授权工具）
    pub effective_tools: Vec<String>,
    /// 声明了但当前不存在的工具
    pub missing_tools: Vec<String>,
    pub problems: Vec<String>,
    pub max_model_calls: Option<u32>,
    pub max_tool_calls: Option<u32>,
}

/// skill_list 返回：清单 + 安装目录（「安装/启用 UI」的目录说明数据源）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListReport {
    pub skills: Vec<SkillInfo>,
    pub user_dir: String,
    pub workspace_dir: Option<String>,
}

/// AG-27：列出 Skill（bundled + user + 可选 workspace 层；含启用态与工具交集）。
/// 仅已安装启用且清单有效的 Skill 才能被 Chat 激活（agent_run_start 侧强校验）。
#[tauri::command]
pub async fn skill_list(
    app: AppHandle,
    project_id: Option<String>,
) -> ApiResponse<SkillListReport> {
    let user_dir = user_skills_dir(&app);
    let workspace_dir = project_id
        .as_deref()
        .map(|pid| workspace_skills_dir(&app, pid));

    let resolved = resolve_skills(collect_skills(
        Some(user_dir.as_path()),
        workspace_dir.as_deref(),
    ));

    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    let _ = create_skill_tables(&conn); // 幂等；兼容未重启升级的旧库

    // 产品 Agent 能力由 Hermes Runtime 统一提供；旧本地 Skill 清单仅计算内置工具交集。
    let available = builtin_available();
    let mut skills = Vec::new();
    for loaded in resolved {
        let name = loaded.display_name();
        let (enabled, version, description, execution, tools, max_model_calls, max_tool_calls) =
            match &loaded.manifest {
                Some(m) => {
                    let enabled = match skill_enabled(&conn, &m.name, loaded.source) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("[skills] 查询启用态失败（按启用处理）: {e}");
                            true
                        }
                    };
                    (
                        enabled,
                        m.version,
                        m.description.clone(),
                        m.execution.as_str().to_string(),
                        m.tools.clone(),
                        m.max_model_calls,
                        m.max_tool_calls,
                    )
                }
                None => (
                    false,
                    0,
                    String::new(),
                    "invalid".to_string(),
                    Vec::new(),
                    None,
                    None,
                ),
            };
        let (effective_tools, missing_tools) = split_effective(&tools, &available);
        // 先提取再消费字段（loaded.origin/problems 是 String/Vec，部分移动后不可再借用）
        let is_available = loaded.available();
        skills.push(SkillInfo {
            name,
            version,
            description,
            execution,
            source: loaded.source,
            origin: loaded.origin,
            enabled,
            available: is_available,
            tools,
            effective_tools,
            missing_tools,
            problems: loaded.problems,
            max_model_calls,
            max_tool_calls,
        });
    }

    ApiResponse::ok(SkillListReport {
        skills,
        user_dir: user_dir.to_string_lossy().to_string(),
        workspace_dir: workspace_dir.map(|p| p.to_string_lossy().to_string()),
    })
}

/// AG-27：启用/停用 Skill（安装 = 用户把清单放入 user/workspace 目录，本命令管开关）
#[tauri::command]
pub async fn skill_set_enabled(
    app: AppHandle,
    name: String,
    source: String,
    enabled: bool,
) -> ApiResponse<()> {
    let source = match SkillSource::parse(&source) {
        Ok(s) => s,
        Err(e) => return ApiResponse::err(e),
    };
    if name.trim().is_empty() {
        return ApiResponse::err("name 不能为空".into());
    }
    let db_path = crate::db::get_db_path(&app);
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return ApiResponse::err(format!("打开数据库失败: {}", e)),
    };
    if let Err(e) = create_skill_tables(&conn) {
        return ApiResponse::err(e);
    }
    match set_skill_enabled(&conn, &name, source, enabled) {
        Ok(()) => ApiResponse::ok(()),
        Err(e) => ApiResponse::err(e),
    }
}

// ---------------- Run 侧解析（agent_run_start 调用） ----------------

/// 激活校验通过的 Skill（Run 消费面）
pub struct ActiveSkill {
    pub manifest: SkillManifest,
    pub source: SkillSource,
}

/// 为一次 Run 解析要激活的 Skill：
/// 未安装 / 清单无效 / 已停用 均返回可直接展示的错误（验收：仅已安装启用可激活）。
pub fn find_active_skill(
    app: &AppHandle,
    name: &str,
    project_id: Option<&str>,
) -> Result<ActiveSkill, String> {
    let user_dir = user_skills_dir(app);
    let workspace_dir = project_id.map(|pid| workspace_skills_dir(app, pid));
    let resolved = resolve_skills(collect_skills(
        Some(user_dir.as_path()),
        workspace_dir.as_deref(),
    ));

    let mut found_invalid: Option<String> = None;
    for skill in &resolved {
        if skill.display_name() != name {
            continue;
        }
        match &skill.manifest {
            None => {
                found_invalid = Some(skill.problems.join("；"));
            }
            Some(manifest) => {
                let db_path = crate::db::get_db_path(app);
                let conn = rusqlite::Connection::open(&db_path)
                    .map_err(|e| format!("打开数据库失败: {}", e))?;
                let _ = create_skill_tables(&conn);
                let enabled = skill_enabled(&conn, &manifest.name, skill.source)
                    .map_err(|e| format!("查询启用态失败: {}", e))?;
                if !enabled {
                    return Err(format!("Skill {} 已停用，请先在技能管理中启用", name));
                }
                return Ok(ActiveSkill {
                    manifest: manifest.clone(),
                    source: skill.source,
                });
            }
        }
    }

    match found_invalid {
        Some(problems) => Err(format!("Skill {} 清单无效：{}", name, problems)),
        None => Err(format!("Skill {} 未安装", name)),
    }
}

// ============================================================
// 单测（验收偏好：单测优先、零真实模型调用）
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "---\nname: demo-skill\nversion: 2\ndescription: 演示技能\nexecution: agent\ntools:\n  - read_document\n  - create_document\nmax_model_calls: 4\nmax_tool_calls: 9\n---\n\n这里是领域指令正文。\n";

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sophonote-skills-test-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- frontmatter 解析 ----

    #[test]
    fn parse_valid_manifest_with_all_fields() {
        let m = parse_manifest(VALID).expect("valid manifest");
        assert_eq!(m.name, "demo-skill");
        assert_eq!(m.version, 2);
        assert_eq!(m.description, "演示技能");
        assert_eq!(m.execution, SkillExecution::Agent);
        assert_eq!(m.tools, vec!["read_document", "create_document"]);
        assert_eq!(m.max_model_calls, Some(4));
        assert_eq!(m.max_tool_calls, Some(9));
        assert!(m.body.contains("领域指令正文"));
    }

    #[test]
    fn parse_optional_budget_fields_default_none_and_empty_tools_allowed() {
        let raw =
            "---\nname: a-b\nversion: 1\ndescription: 最小\nexecution: workflow\n---\n正文。\n";
        let m = parse_manifest(raw).expect("minimal manifest");
        assert_eq!(m.execution, SkillExecution::Workflow);
        assert!(m.tools.is_empty());
        assert_eq!(m.max_model_calls, None);
        assert_eq!(m.max_tool_calls, None);
    }

    #[test]
    fn parse_rejects_schema_violations() {
        // 缺 frontmatter
        assert!(parse_manifest("没有清单头").is_err());
        // frontmatter 未闭合
        assert!(parse_manifest("---\nname: a-b\n").is_err());
        // 缺必填
        assert!(parse_manifest("---\nversion: 1\n---\n正文").is_err());
        // 非法 name（大写）
        let raw = VALID.replace("demo-skill", "Demo-Skill");
        assert!(parse_manifest(&raw).is_err());
        // version = 0
        let raw = VALID.replace("version: 2", "version: 0");
        assert!(parse_manifest(&raw).is_err());
        // version 非数字
        let raw = VALID.replace("version: 2", "version: x");
        assert!(parse_manifest(&raw).is_err());
        // 非法 execution
        let raw = VALID.replace("execution: agent", "execution: script");
        assert!(parse_manifest(&raw).is_err());
        // 空 description
        let raw = VALID.replace("description: 演示技能", "description: ' '");
        assert!(parse_manifest(&raw).is_err());
        // description 超长（201 个汉字）
        let long = "长".repeat(201);
        let raw = VALID.replace("description: 演示技能", &format!("description: {}", long));
        assert!(parse_manifest(&raw).is_err());
        // 空正文
        assert!(parse_manifest(
            "---\nname: a-b\nversion: 1\ndescription: x\nexecution: agent\n---\n   \n"
        )
        .is_err());
        // 未知字段
        let raw = "---\nname: a-b\nversion: 1\ndescription: x\nexecution: agent\nauthor: mallory\n---\n正文";
        assert!(parse_manifest(raw).is_err());
        // 重复字段
        let raw =
            "---\nname: a-b\nname: c-d\nversion: 1\ndescription: x\nexecution: agent\n---\n正文";
        assert!(parse_manifest(raw).is_err());
        // 预算越界
        let raw = VALID.replace("max_model_calls: 4", "max_model_calls: 99");
        assert!(parse_manifest(&raw).is_err());
        let raw = VALID.replace("max_tool_calls: 9", "max_tool_calls: 999");
        assert!(parse_manifest(&raw).is_err());
        // 非法工具名
        let raw = VALID.replace("  - read_document", "  - Evil.Tool");
        assert!(parse_manifest(&raw).is_err());
        // 行内数组不支持（明确报错而不是静默误解析）
        let raw = "---\nname: a-b\nversion: 1\ndescription: x\nexecution: agent\ntools: [read_document]\n---\n正文";
        assert!(parse_manifest(raw).is_err());
    }

    #[test]
    fn parse_supports_quoted_scalars_and_comment_lines() {
        let raw = "---\n# 注释行\nname: \"quoted-skill\"\nversion: 1\ndescription: '带引号'\nexecution: agent\n---\n正文";
        let m = parse_manifest(raw).expect("quoted values");
        assert_eq!(m.name, "quoted-skill");
        assert_eq!(m.description, "带引号");
    }

    // ---- 三层加载与 Resolver ----

    #[test]
    fn bundled_skills_are_valid_and_their_tools_all_exist() {
        // 回归守护：内置清单必须永远可激活——解析通过、工具全部在内置可用集内
        let bundled = load_bundled();
        assert_eq!(bundled.len(), 2);
        let available = builtin_available();
        let names: BTreeSet<String> = bundled
            .iter()
            .map(|s| {
                s.manifest
                    .as_ref()
                    .expect("bundled 必须解析通过")
                    .name
                    .clone()
            })
            .collect();
        assert!(names.contains("research-note"));
        assert!(names.contains("daily-picks"));
        for s in &bundled {
            let m = s.manifest.as_ref().unwrap();
            assert!(s.problems.is_empty());
            let (effective, missing) = split_effective(&m.tools, &available);
            assert!(
                missing.is_empty(),
                "内置 Skill {} 声明了不存在的工具: {:?}",
                m.name,
                missing
            );
            assert!(
                !effective.is_empty(),
                "内置 Skill {} 工具交集不得为空",
                m.name
            );
        }
        // execution 形态：research-note=agent / daily-picks=workflow（docs/architecture.md）
        let by_name = |n: &str| {
            bundled
                .iter()
                .find(|s| s.manifest.as_ref().unwrap().name == n)
                .unwrap()
                .manifest
                .as_ref()
                .unwrap()
                .execution
        };
        assert_eq!(by_name("research-note"), SkillExecution::Agent);
        assert_eq!(by_name("daily-picks"), SkillExecution::Workflow);
    }

    #[test]
    fn resolver_priority_workspace_over_user_over_bundled() {
        let raw = |name: &str, desc: &str| {
            format!(
                "---\nname: {}\nversion: 1\ndescription: {}\nexecution: agent\n---\n正文",
                name, desc
            )
        };
        let loaded = vec![
            LoadedSkill {
                manifest: parse_manifest(&raw("shared", "内置版")).ok(),
                source: SkillSource::Bundled,
                origin: "bundled:shared.md".into(),
                problems: vec![],
            },
            LoadedSkill {
                manifest: parse_manifest(&raw("shared", "用户版")).ok(),
                source: SkillSource::User,
                origin: "shared.md".into(),
                problems: vec![],
            },
            LoadedSkill {
                manifest: parse_manifest(&raw("shared", "工作区版")).ok(),
                source: SkillSource::Workspace,
                origin: "shared.md".into(),
                problems: vec![],
            },
        ];
        let resolved = resolve_skills(loaded);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].source, SkillSource::Workspace);
        assert_eq!(
            resolved[0].manifest.as_ref().unwrap().description,
            "工作区版"
        );
    }

    #[test]
    fn invalid_entries_do_not_shadow_valid_ones_but_stay_visible() {
        let valid = LoadedSkill {
            manifest: parse_manifest(VALID).ok(),
            source: SkillSource::Bundled,
            origin: "bundled:demo-skill.md".into(),
            problems: vec![],
        };
        let broken = LoadedSkill {
            manifest: None,
            source: SkillSource::Workspace,
            origin: "demo-skill.md".into(),
            problems: vec!["frontmatter 未闭合".into()],
        };
        let resolved = resolve_skills(vec![valid, broken]);
        // 有效 bundled 不被无效 workspace 文件遮蔽；无效条目仍可见（供 UI 展示问题）
        assert_eq!(resolved.len(), 2);
        assert!(resolved
            .iter()
            .any(|s| s.source == SkillSource::Bundled && s.available()));
        assert!(resolved.iter().any(|s| !s.available()));
    }

    #[test]
    fn load_dir_reads_md_files_skips_symlinks_and_flags_bad_files() {
        let dir = temp_dir("loaddir");
        std::fs::write(dir.join("good.md"), VALID).unwrap();
        std::fs::write(dir.join("broken.md"), "---\nname: broken-skill\n---\n正文").unwrap();
        std::fs::write(dir.join("not-markdown.txt"), "ignored").unwrap();
        std::fs::write(
            dir.join("huge.md"),
            "x".repeat(SKILL_MAX_FILE_BYTES as usize + 1),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let outside = temp_dir("outside");
            std::fs::write(outside.join("target.md"), VALID).unwrap();
            std::os::unix::fs::symlink(outside.join("target.md"), dir.join("link.md"))
                .expect("symlink");
            cleanup(&outside);
        }

        let loaded = load_dir(&dir, SkillSource::User);
        cleanup(&dir);

        let names: Vec<String> = loaded.iter().map(|s| s.display_name()).collect();
        // symlink 不跟随（不入列）；txt 忽略；good 有效；broken/huge 作为问题条目可见。
        // 无效清单按文件名主干定位（清单 name 仅有效清单可用——用户凭文件名找到并修复它）
        assert!(names.contains(&"demo-skill".to_string()));
        assert!(names.contains(&"broken".to_string()));
        assert!(names.contains(&"huge".to_string()));
        #[cfg(unix)]
        assert!(!names.contains(&"link".to_string()));

        let huge = loaded.iter().find(|s| s.display_name() == "huge").unwrap();
        assert!(!huge.available());
        assert!(huge.problems[0].contains("大小上限"));
        let broken = loaded
            .iter()
            .find(|s| s.display_name() == "broken")
            .unwrap();
        assert!(!broken.available());
    }

    // ---- 权限交集 ----

    #[test]
    fn split_effective_partitions_declared_into_intersection_and_missing() {
        let available = builtin_available();
        let declared: Vec<String> = vec![
            "read_document".into(),
            "document.delete_all".into(), // 不存在 → missing（Skill 不能自授权限）
            "create_document".into(),
        ];
        let (effective, missing) = split_effective(&declared, &available);
        assert_eq!(effective, vec!["read_document", "create_document"]);
        assert_eq!(missing, vec!["document.delete_all"]);
    }

    /// AG-29：Skill 交集的可用面 = 内置 ∪ 已连接且授权的 MCP 命名空间工具
    /// （agent_run_start 步骤 1.6 同款构造）——Skill 声明 mcp.<server>.<tool>
    /// 时按命名空间 ID 精确命中；未就绪的 MCP 工具落入 missing（只收窄不放大）。
    #[test]
    fn split_effective_includes_namespaced_mcp_tools_when_available() {
        let mut available = builtin_available();
        available.insert("mcp.pyfix.echo".into()); // 模拟 ready_tool_ids 并入
        let declared: Vec<String> = vec![
            "read_document".into(),
            "mcp.pyfix.echo".into(),
            "mcp.pyfix.big".into(), // 未授权/未连接 → missing
        ];
        let (effective, missing) = split_effective(&declared, &available);
        assert_eq!(effective, vec!["read_document", "mcp.pyfix.echo"]);
        assert_eq!(missing, vec!["mcp.pyfix.big"]);
    }

    /// 漂移守护：BUILTIN_TOOL_NAMES 必须 = project_registry 真实注册工具 − spike 假工具。
    /// 工具层增删工具时，本测试强制同步交集计算面（沙箱无编译器，宿主 cargo test 把关）。
    #[test]
    fn builtin_tool_names_match_project_registry() {
        let dir = temp_dir("registry");
        let reg = crate::agent::commands::project_registry(
            dir.join("db.sqlite"),
            dir.join("notes"),
            "p-test",
            "r-test",
        );
        cleanup(&dir);
        let names = reg.names();
        for t in BUILTIN_TOOL_NAMES {
            assert!(names.contains(*t), "project_registry 缺少工具 {}", t);
        }
        let extras: Vec<_> = names.difference(&builtin_available()).cloned().collect();
        // 差集只允许 spike 调试假工具（calculator/get_weather）
        for e in &extras {
            assert!(
                e == "calculator" || e == "get_weather",
                "project_registry 出现未登记工具 {}（请同步 BUILTIN_TOOL_NAMES）",
                e
            );
        }
    }

    // ---- 启用态持久化 ----

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        create_skill_tables(&conn).expect("create_skill_tables");
        conn
    }

    #[test]
    fn skill_enabled_defaults_true_and_persists_toggle() {
        let conn = mem_db();
        assert!(skill_enabled(&conn, "research-note", SkillSource::Bundled).unwrap());

        set_skill_enabled(&conn, "research-note", SkillSource::Bundled, false).unwrap();
        assert!(!skill_enabled(&conn, "research-note", SkillSource::Bundled).unwrap());

        // upsert 再启用
        set_skill_enabled(&conn, "research-note", SkillSource::Bundled, true).unwrap();
        assert!(skill_enabled(&conn, "research-note", SkillSource::Bundled).unwrap());

        // (name, source) 联合键：同名不同来源互不影响
        set_skill_enabled(&conn, "research-note", SkillSource::User, false).unwrap();
        assert!(skill_enabled(&conn, "research-note", SkillSource::Bundled).unwrap());
        assert!(!skill_enabled(&conn, "research-note", SkillSource::User).unwrap());
    }

    #[test]
    fn collect_skills_includes_bundled_even_when_dirs_missing() {
        let missing = PathBuf::from("/nonexistent/sophonote/skills");
        let loaded = collect_skills(Some(&missing), None);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().all(|s| s.source == SkillSource::Bundled));
    }
}
