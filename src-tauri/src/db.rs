use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub source_id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub title: String,
    pub url: String,
    pub description: String,
    pub author: Option<String>,
    pub language: Option<String>,
    pub stars: Option<i32>,
    pub forks: Option<i32>,
    pub topics: Option<String>,
    pub published_at: String,
    pub fetched_at: String,
    pub status: String,
    pub ai_summary: Option<String>,
    pub ai_tags: Option<String>,
    // 列表轻量标识用（P0-5）：item_contents 联表带出，不承载正文
    #[serde(default)]
    pub content_status: Option<String>,
    #[serde(default)]
    pub quality_level: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Article {
    pub id: String,
    pub item_id: Option<String>,
    pub title: String,
    pub content: String,
    pub article_type: String,
    pub edited: bool,
    pub created_at: String,
    pub updated_at: Option<String>,
    // 生成该解读所用提示词版本（回归对比用，如 nightly@v1）
    #[serde(default)]
    pub prompt_version: Option<String>,
    // BlockSuite 文档快照（DocSnapshot JSON）；与 content（Markdown）双格式并存，默认 BlockSuite
    #[serde(default)]
    pub blocks_json: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub source_type: String,
    pub enabled: bool,
    pub config: Option<String>,
    pub fetch_interval_minutes: i32,
    pub last_fetched_at: Option<String>,
    // 信源分层（借鉴 ai-news-radar source_tier）：core | standard | experimental
    #[serde(default = "default_tier")]
    pub tier: String,
    // 准入状态：active | probation（试用观察期：参与抓取但不进默认视图）| skipped（高风险跳过）
    #[serde(default = "default_admission")]
    pub admission: String,
    // 源健康三件套（借鉴 ai-news-radar source-status）：成功率 / 最后成功时间 / 产量
    #[serde(default)]
    pub last_success_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub fetch_success_count: i64,
    #[serde(default)]
    pub fetch_fail_count: i64,
}

fn default_tier() -> String {
    "core".to_string()
}

fn default_admission() -> String {
    "active".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyLog {
    pub id: String,
    pub date: String,
    #[serde(rename = "type")]
    pub log_type: String,
    pub content: String,
    pub sources: Option<String>,
    pub generated_by: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: i32,
    pub due_date: Option<String>,
    pub recurring: Option<String>,
    pub tags: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

/// 番茄钟专注会话（DEC-034）：task_id 可空表示未关联任务的专注记录。
/// completed = 1 仅当自然走完计划时长；中途放弃记 0 但保留起止时间。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PomodoroSession {
    pub id: String,
    pub task_id: Option<String>,
    pub planned_minutes: i32,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub completed: bool,
}

/// 插入或覆盖一条番茄钟会话记录（幂等）。
pub fn insert_pomodoro_session(
    conn: &rusqlite::Connection,
    session: &PomodoroSession,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR REPLACE INTO pomodoro_sessions (id, task_id, planned_minutes, started_at, ended_at, completed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            session.id,
            session.task_id,
            session.planned_minutes,
            session.started_at,
            session.ended_at,
            session.completed
        ],
    )?;
    Ok(())
}

/// 读取 `since`（含）之后的番茄钟会话；`since` 为 None 时读全量，按开始时间倒序。
pub fn list_pomodoro_sessions(
    conn: &rusqlite::Connection,
    since: Option<&str>,
) -> Result<Vec<PomodoroSession>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, task_id, planned_minutes, started_at, ended_at, completed
         FROM pomodoro_sessions WHERE started_at >= ?1 ORDER BY started_at DESC",
    )?;
    let since_value = since.unwrap_or("");
    let rows = stmt
        .query_map(rusqlite::params![since_value], map_pomodoro_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn map_pomodoro_row(row: &rusqlite::Row) -> Result<PomodoroSession, rusqlite::Error> {
    Ok(PomodoroSession {
        id: row.get(0)?,
        task_id: row.get(1)?,
        planned_minutes: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        completed: row.get(5)?,
    })
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ItemContent {
    pub item_id: String,
    pub status: String, // pending | fetching | ready | partial | failed | unsupported
    pub content_text: Option<String>,
    pub excerpt: Option<String>,
    pub evidence_json: Option<String>,
    pub content_type: Option<String>, // readme | article | abstract | model_card | discussion
    pub quality_level: i32,           // 0 标题 / 1 简介 / 2 摘要或正文 / 3 正文+多源证据
    pub content_hash: Option<String>,
    pub fetched_at: Option<String>,
    pub error_message: Option<String>,
}

pub fn get_db_path(app: &AppHandle) -> PathBuf {
    crate::storage_layout::StorageLayout::resolve(app)
        .expect("Failed to resolve SophoNote storage layout")
        .database
}

pub fn init_db(app: &AppHandle) -> Result<(), String> {
    let db_path = get_db_path(app);

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;

    create_schema(&conn)?;
    seed_default_sources(&conn)?;
    Ok(())
}

/// 全量建表（幂等）：全部 CREATE IF NOT EXISTS + ensure_columns 补列 + Track B 项目表。
/// 自 init_db 抽出，供单测内存库原样复用（不含默认数据播种）。
pub fn create_schema(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sources (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            source_type TEXT NOT NULL,
            enabled INTEGER DEFAULT 1,
            config TEXT,
            fetch_interval_minutes INTEGER DEFAULT 60,
            last_fetched_at TIMESTAMP,
            tier TEXT NOT NULL DEFAULT 'core',
            admission TEXT NOT NULL DEFAULT 'active',
            last_success_at TIMESTAMP,
            last_error TEXT,
            fetch_success_count INTEGER DEFAULT 0,
            fetch_fail_count INTEGER DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS items (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            item_type TEXT NOT NULL,
            title TEXT NOT NULL,
            url TEXT,
            description TEXT,
            author TEXT,
            language TEXT,
            stars INTEGER,
            forks INTEGER,
            topics TEXT,
            published_at TIMESTAMP,
            fetched_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            -- DEC-028：收件箱严格保留 168 小时。首次拉取与过期时刻不可续期；
            -- last_seen_at 只记录数据源最近一次再次观测到该条目。
            first_fetched_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            expires_at TIMESTAMP NOT NULL DEFAULT (datetime('now', '+7 days')),
            status TEXT DEFAULT 'unread',
            user_notes TEXT,
            ai_summary TEXT,
            ai_tags TEXT,
            ai_prompt_version TEXT,
            ai_enrich_json TEXT,
            -- NEXT-048 发现五断面数据面：Skill（sophonote-ai-radar）打分趟经 Bridge
            -- save_discovery_scores 写入；Rust 只读不解释。精选=aspect∧近7天∧≥8.5，全部=≥7
            ai_score REAL,
            ai_scored_at TIMESTAMP,
            aspect TEXT,
            ai_topics TEXT,
            ai_reason TEXT,
            embedding BLOB,
            FOREIGN KEY (source_id) REFERENCES sources(id)
        );

        -- DEC-028：极小型去重/TTL 账本。条目正文过期删除后仍保留稳定 id 的首次拉取边界，
        -- 防止同一来源重复返回旧条目时被当成新数据重新续命。
        CREATE TABLE IF NOT EXISTS inbox_item_ttl (
            item_id TEXT PRIMARY KEY,
            first_fetched_at TIMESTAMP NOT NULL,
            last_seen_at TIMESTAMP NOT NULL,
            expires_at TIMESTAMP NOT NULL
        );

        CREATE TABLE IF NOT EXISTS collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            icon TEXT,
            color TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS collection_items (
            collection_id TEXT NOT NULL,
            item_id TEXT NOT NULL,
            added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (collection_id, item_id)
        );

        CREATE TABLE IF NOT EXISTS daily_logs (
            id TEXT PRIMARY KEY,
            date TEXT NOT NULL UNIQUE,
            log_type TEXT NOT NULL,
            content TEXT NOT NULL,
            sources TEXT,
            generated_by TEXT DEFAULT 'ai',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT,
            status TEXT DEFAULT 'todo',
            priority INTEGER DEFAULT 2,
            due_date TEXT,
            recurring TEXT,
            tags TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            completed_at TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS pomodoro_sessions (
            id TEXT PRIMARY KEY,
            task_id TEXT,
            planned_minutes INTEGER NOT NULL DEFAULT 25,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            completed INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS articles (
            id TEXT PRIMARY KEY,
            item_id TEXT,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            article_type TEXT DEFAULT 'deep-dive',
            edited INTEGER DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP,
            prompt_version TEXT,
            blocks_json TEXT,
            FOREIGN KEY (item_id) REFERENCES items(id)
        );

        -- 故事级合并（借鉴 ai-news-radar stories-merged）：同一事件的多源报道聚合成一个 story，
        -- source_count >= 2 时 signal_level = 'multi'（多源验证）
        CREATE TABLE IF NOT EXISTS stories (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            item_ids TEXT NOT NULL,
            source_ids TEXT NOT NULL,
            source_count INTEGER NOT NULL DEFAULT 1,
            signal_level TEXT NOT NULL DEFAULT 'single',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        -- chunk 级语义索引的文本侧（借鉴 khoj chunk+embedding 管线）：
        -- 正文/证据分片存储，向量在 vec_chunks 虚拟表，语义搜索可命中证据片段并溯源到条目
        CREATE TABLE IF NOT EXISTS item_chunks (
            chunk_id TEXT PRIMARY KEY,
            item_id TEXT NOT NULL,
            chunk_idx INTEGER NOT NULL,
            text TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        -- N3：笔记/文档 chunk 文本侧（与 item_chunks 平行，note_id = articles.id）。
        -- 向量在 vec_note_chunks 虚拟表；检索命中回表 articles 取标题与类型
        CREATE TABLE IF NOT EXISTS note_chunks (
            chunk_id TEXT PRIMARY KEY,
            note_id TEXT NOT NULL,
            chunk_idx INTEGER NOT NULL,
            text TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        -- 每日 Top5 推荐（发现页数据层）：LLM 打分筛选出的每类别当日最热条目。
        -- 跨天去重由应用层基于 (category, item_id) 历史判断；热度增长超阈值的「迭代」允许再入选
        CREATE TABLE IF NOT EXISTS daily_picks (
            id TEXT PRIMARY KEY,
            date TEXT NOT NULL,
            category TEXT NOT NULL,
            item_id TEXT NOT NULL,
            rank INTEGER NOT NULL,
            heat_score INTEGER,
            ai_score REAL,
            reason TEXT,
            selection_lane TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(date, category, item_id),
            FOREIGN KEY (item_id) REFERENCES items(id)
        );

        -- 条目正文内容（与轻量元数据 items 分离，列表查询不受长文本拖累）
        -- status: pending | fetching | ready | partial | failed | unsupported
        -- quality_level: 0 只有标题 / 1 只有简介 / 2 有摘要或有效正文 / 3 正文+多源证据
        CREATE TABLE IF NOT EXISTS item_contents (
            item_id         TEXT PRIMARY KEY,
            status          TEXT NOT NULL DEFAULT 'pending',
            content_text    TEXT,
            excerpt         TEXT,
            evidence_json   TEXT,
            content_type    TEXT,
            quality_level   INTEGER DEFAULT 0,
            content_hash    TEXT,
            fetched_at      TEXT,
            error_message   TEXT,
            FOREIGN KEY (item_id) REFERENCES items(id)
        );

        CREATE INDEX IF NOT EXISTS idx_items_source ON items(source_id);
        CREATE INDEX IF NOT EXISTS idx_items_status ON items(status);
        CREATE INDEX IF NOT EXISTS idx_items_type ON items(item_type);
        CREATE INDEX IF NOT EXISTS idx_items_fetched ON items(fetched_at);
        CREATE INDEX IF NOT EXISTS idx_inbox_ttl_expires ON inbox_item_ttl(expires_at);
        CREATE INDEX IF NOT EXISTS idx_articles_item ON articles(item_id);
        CREATE INDEX IF NOT EXISTS idx_articles_created ON articles(created_at);
        CREATE INDEX IF NOT EXISTS idx_chunks_item ON item_chunks(item_id);
        CREATE INDEX IF NOT EXISTS idx_note_chunks_note ON note_chunks(note_id);
        CREATE INDEX IF NOT EXISTS idx_picks_date_cat ON daily_picks(date, category);
        CREATE INDEX IF NOT EXISTS idx_picks_cat_item ON daily_picks(category, item_id);

        -- NEXT-048：模型榜周快照（共识分）。Skill action=model-board 周更写入，
        -- 唯一 (date, model_key) 保证幂等重跑。v1 = SophoNote 共识分；v2 聚合外部公开榜单
        CREATE TABLE IF NOT EXISTS model_leaderboard_snapshots (
            id TEXT PRIMARY KEY,
            date TEXT NOT NULL,
            model_key TEXT NOT NULL,
            name TEXT NOT NULL,
            vendor TEXT,
            rank INTEGER,
            consensus REAL NOT NULL,
            meta_json TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(date, model_key)
        );
        CREATE INDEX IF NOT EXISTS idx_leaderboard_date ON model_leaderboard_snapshots(date);

        -- DEC-023 / NEXT-052：OpenRouter 官方模型榜完整快照。只保留最近一次成功
        -- 的原子快照；payload 包含 models/usage/tasks/session-cost/benchmarks，
        -- API Key 永不进入本表。
        CREATE TABLE IF NOT EXISTS openrouter_ranking_snapshots (
            id TEXT PRIMARY KEY,
            as_of TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            citation TEXT NOT NULL,
            payload_json TEXT NOT NULL
        );
        "#,
    )
    .map_err(|e| e.to_string())?;

    // 轻量迁移：历史库无迁移机制，逐列补齐新增字段（幂等）
    ensure_columns(
        conn,
        "sources",
        &[
            ("tier", "tier TEXT NOT NULL DEFAULT 'core'"),
            ("admission", "admission TEXT NOT NULL DEFAULT 'active'"),
            ("last_success_at", "last_success_at TIMESTAMP"),
            ("last_error", "last_error TEXT"),
            (
                "fetch_success_count",
                "fetch_success_count INTEGER DEFAULT 0",
            ),
            ("fetch_fail_count", "fetch_fail_count INTEGER DEFAULT 0"),
        ],
    )?;
    ensure_columns(
        conn,
        "items",
        &[
            ("ai_prompt_version", "ai_prompt_version TEXT"),
            ("ai_enrich_json", "ai_enrich_json TEXT"),
            // NEXT-048 五断面数据面补列（旧库）
            ("ai_score", "ai_score REAL"),
            ("ai_scored_at", "ai_scored_at TIMESTAMP"),
            ("aspect", "aspect TEXT"),
            ("ai_topics", "ai_topics TEXT"),
            ("ai_reason", "ai_reason TEXT"),
            // DEC-028：ALTER TABLE 不能安全添加动态默认值，先补可空列，再统一回填。
            ("first_fetched_at", "first_fetched_at TIMESTAMP"),
            ("last_seen_at", "last_seen_at TIMESTAMP"),
            ("expires_at", "expires_at TIMESTAMP"),
        ],
    )?;
    // 历史库没有真实 first_fetched_at，只能以旧 fetched_at 作迁移基线；此后该值不可变。
    // datetime() 统一归一化为 UTC SQLite 时间，保证精确 168 小时而非自然日边界。
    conn.execute_batch(
        "UPDATE items
         SET first_fetched_at = COALESCE(datetime(first_fetched_at), datetime(fetched_at), datetime('now'))
         WHERE first_fetched_at IS NULL;
         UPDATE items
         SET last_seen_at = COALESCE(datetime(last_seen_at), datetime(fetched_at), first_fetched_at)
         WHERE last_seen_at IS NULL;
         UPDATE items
         SET expires_at = datetime(first_fetched_at, '+168 hours')
         WHERE expires_at IS NULL;
         INSERT OR IGNORE INTO inbox_item_ttl (item_id, first_fetched_at, last_seen_at, expires_at)
         SELECT id, first_fetched_at, last_seen_at, expires_at FROM items;",
    )
    .map_err(|e| e.to_string())?;
    // 历史库必须先经 ensure_columns 补齐字段，才能创建索引。
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_items_feed ON items(ai_scored_at, ai_score);
         CREATE INDEX IF NOT EXISTS idx_items_expires ON items(expires_at);",
    )
    .map_err(|e| e.to_string())?;
    ensure_columns(
        conn,
        "daily_picks",
        &[("selection_lane", "selection_lane TEXT")],
    )?;
    ensure_columns(
        conn,
        "articles",
        &[
            ("prompt_version", "prompt_version TEXT"),
            ("blocks_json", "blocks_json TEXT"),
            // AG-24（Phase 3 DocumentService）：正文写入版本号——CAS 并发冲突检测的真相源。
            // 仅正文写入递增（编辑器心跳与 Agent 写路径同源递增），改名不递增；存量行 → 1
            ("version", "version INTEGER NOT NULL DEFAULT 1"),
        ],
    )?;

    create_project_tables(conn)?;
    // Track B · Agent Runtime（AG-13 追加）
    create_agent_tables(conn)?;
    // Track B · AG-24（Phase 3 DocumentService）：文档域版本/修订/操作日志
    create_document_tables(conn)?;
    // Track B · AG-27（Phase 4 Skill 系统）：Skill 启用态（skill_state）
    crate::skills::create_skill_tables(conn)?;
    // Track B · AG-28（Phase 5 MCP 管理）：服务器配置与工具授权（mcp_servers/mcp_tool_auth）
    crate::tools::mcp::create_mcp_tables(conn)?;
    crate::knowledge::create_version_tables(conn)?;
    Ok(())
}

/// 默认数据播种（数据源 + 收藏夹，均 INSERT OR IGNORE 幂等）。
/// 与建表分离：单测只建 schema 不播种，避免默认数据干扰断言。
fn seed_default_sources(conn: &rusqlite::Connection) -> Result<(), String> {
    // 初始化默认数据源
    let default_sources = [
        ("github-trending", "GitHub Trending", "github", true, 360),
        ("arxiv-ai", "arXiv AI Papers", "arxiv", true, 360),
        ("hackernews", "HackerNews", "hackernews", true, 60),
        (
            "huggingface-models",
            "HuggingFace 模型榜",
            "huggingface",
            true,
            360,
        ),
        (
            "huggingface-papers",
            "HuggingFace 每日论文",
            "huggingface_papers",
            true,
            360,
        ),
        ("producthunt", "ProductHunt", "producthunt", false, 360),
        // AIHOT：官方匿名只读 v1 API，无需 Key；个人非商业用途内免费（见 Skill references/aihot-source.md）
        ("aihot", "AIHOT 精选", "aihot", true, 60),
    ];

    for (id, name, source_type, enabled, interval) in &default_sources {
        conn.execute(
            "INSERT OR IGNORE INTO sources (id, name, source_type, enabled, fetch_interval_minutes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            rusqlite::params![id, name, source_type, if *enabled { 1 } else { 0 }, interval],
        ).map_err(|e| e.to_string())?;
    }

    // 初始化默认收藏夹
    let default_collections = [
        ("favorites", "收藏夹", "⭐", "#fbbf24"),
        ("ai-models", "AI模型", "🤖", "#8b5cf6"),
        ("architecture", "架构设计", "🏗️", "#06b6d4"),
        ("products", "产品分析", "📱", "#f472b6"),
    ];

    for (id, name, icon, color) in &default_collections {
        conn.execute(
            "INSERT OR IGNORE INTO collections (id, name, icon, color, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            rusqlite::params![id, name, icon, color],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ---- Track B · 智能体演进（AG-02 追加）：AI 工作室项目容器，见 docs/architecture.md ----
// projects 扁平分组容器：parent_id 预留（恒 NULL），未来填值即升级文件夹树，无 schema 迁移。
// project_documents 单一归属：PK = article_id → 一篇文档至多属于一个项目，
// 换项目 = INSERT OR REPLACE（move 语义，文件夹归属）。
fn create_project_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            parent_id TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS project_documents (
            project_id TEXT NOT NULL REFERENCES projects(id),
            article_id TEXT NOT NULL PRIMARY KEY REFERENCES articles(id),
            added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_proj_docs_project ON project_documents(project_id);
        "#,
    )
    .map_err(|e| e.to_string())?;

    // ---- Track A · NB-19（用户指令例外，AG-11 组织树落地）：project_documents 补 parent_id ----
    // Notion 式文档树：文档即树节点，parent_id 指向同项目另一 article_id，NULL = 项目根。
    ensure_columns(
        conn,
        "project_documents",
        &[("parent_id", "parent_id TEXT")],
    )?;

    Ok(())
}

// ---- Track B · Agent Runtime（AG-13 追加）：Agent 数据域表 ----
fn create_agent_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_threads (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'running',
            project_id TEXT,
            latest_run_id TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agent_runs (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            project_id TEXT,
            status TEXT NOT NULL DEFAULT 'queued',
            provider TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '',
            prompt_version TEXT,
            max_model_calls INTEGER NOT NULL DEFAULT 6,
            current_model_calls INTEGER NOT NULL DEFAULT 0,
            engine TEXT NOT NULL DEFAULT 'hermes',
            engine_version TEXT NOT NULL DEFAULT '0.20',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agent_messages (
            id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
            content TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT 'spike',
            created_at INTEGER NOT NULL,
            FOREIGN KEY(thread_id) REFERENCES agent_threads(id) ON DELETE CASCADE,
            FOREIGN KEY(run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_agent_messages_thread
            ON agent_messages(thread_id);

        CREATE TABLE IF NOT EXISTS agent_tool_calls (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            arguments_json TEXT,
            result_text TEXT,
            error_text TEXT,
            preresolved INTEGER DEFAULT 0,
            created_at INTEGER NOT NULL,
            structured_json TEXT,
            ui_artifact_json TEXT,
            provenance_json TEXT,
            truncated INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY(run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_approvals (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            approval_type TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            resource_summary TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL,
            resolved_at INTEGER,
            FOREIGN KEY(run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS agent_run_events (
            event_id TEXT PRIMARY KEY,
            thread_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            timestamp INTEGER NOT NULL,
            schema_version INTEGER NOT NULL DEFAULT 1,
            data TEXT NOT NULL,
            UNIQUE(run_id, seq),
            FOREIGN KEY(run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_seq
            ON agent_run_events(run_id, seq ASC);

        -- 收藏夹分类：会话经 agent_threads.collection_id 归入（至多一个分类）。
        -- 同名唯一索引兜底；store 层先查给出友好错误。
        CREATE TABLE IF NOT EXISTS thread_collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_thread_collections_name
            ON thread_collections(name);
        "#,
    )
    .map_err(|e| e.to_string())?;
    // Phase 2 扩字段 → ensure_columns 补列（幂等），与已有模式一致
    ensure_columns(
        conn,
        "agent_threads",
        &[
            ("project_id", "project_id TEXT"),
            // Hermes 会话映射：一个 SophoNote Thread 固定绑定一个 Hermes Session。
            // 仅保存外部引用；消息与长期记忆内容仍由各自真相源维护。
            ("external_session_id", "external_session_id TEXT"),
            ("closed_at", "closed_at INTEGER"),
            ("archived_at", "archived_at INTEGER"),
            // 置顶/收藏夹：组织性操作列，NULL = 未置顶/未收藏
            ("pinned_at", "pinned_at INTEGER"),
            ("collection_id", "collection_id TEXT"),
        ],
    )?;
    // AG-21：工具结果五件套持久化列（历史库无列 → NULL/0 兜底，读取侧按无处理）
    ensure_columns(
        conn,
        "agent_tool_calls",
        &[
            ("structured_json", "structured_json TEXT"),
            ("ui_artifact_json", "ui_artifact_json TEXT"),
            ("provenance_json", "provenance_json TEXT"),
            ("truncated", "truncated INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;
    // H4 / NEXT-021：Hermes 外部 Run 对账列（幂等补列）
    ensure_columns(
        conn,
        "agent_runs",
        &[
            ("engine_transport", "engine_transport TEXT"),
            ("external_run_id", "external_run_id TEXT"),
            ("external_session_id", "external_session_id TEXT"),
            (
                "external_protocol_version",
                "external_protocol_version TEXT",
            ),
            ("last_external_event_id", "last_external_event_id TEXT"),
        ],
    )?;
    Ok(())
}

// ---- Track B · AG-24（Phase 3 DocumentService）：文档域表 ----
// 设计基线 docs/architecture.md：物理文件仍按 UUID 平铺（notes/<id>.md），
// 版本/修订/操作日志全在 DB。Agent 不直碰 notes.rs/SQLite/文件——写入全走
// DocumentService（dry-run 预览 → 审批 → 冲突复检 → 提交 → 可撤销）。
// 表所有权：document_revisions / document_operations 归轨道 B（§3.9 规则 4）。
fn create_document_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        -- 修订快照：每次 Agent 写入前保存旧正文全量快照，undo 的真相源。
        -- version = 本次写入产生的新版本号；content_hash = 旧正文 FNV-1a（§3.3 同口径）
        CREATE TABLE IF NOT EXISTS document_revisions (
            id TEXT PRIMARY KEY,
            document_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            content_snapshot TEXT NOT NULL,
            operation_id TEXT,
            run_id TEXT,
            tool_call_id TEXT,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_doc_revisions_doc
            ON document_revisions(document_id, version DESC);

        -- 操作日志：幂等键去重 + prepared/committed 状态机 + 启动恢复。
        -- 文件与 SQLite 无法同一事务——操作日志 + 唯一临时文件（带 operation ID，
        -- 不用固定 <id>.md.tmp）+ 原子 rename 的补偿方案（docs/architecture.md）
        CREATE TABLE IF NOT EXISTS document_operations (
            id TEXT PRIMARY KEY,
            idempotency_key TEXT UNIQUE,
            document_id TEXT NOT NULL,
            operation_type TEXT NOT NULL,
            base_version INTEGER NOT NULL,
            target_version INTEGER,
            status TEXT NOT NULL,
            error TEXT,
            tmp_path TEXT,
            approval_id TEXT,
            run_id TEXT,
            payload_json TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_doc_operations_status
            ON document_operations(status);
        "#,
    )
    .map_err(|e| e.to_string())
}

/// 轻量迁移：表缺列时 ALTER 补齐（幂等）。历史库只有 CREATE IF NOT EXISTS，无迁移机制。
fn ensure_columns(
    conn: &rusqlite::Connection,
    table: &str,
    cols: &[(&str, &str)],
) -> Result<(), String> {
    let mut existing: Vec<String> = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({})", table)) {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
            existing = rows.filter_map(|r| r.ok()).collect();
        }
    }
    for (name, ddl) in cols {
        if !existing.iter().any(|c| c == name) {
            conn.execute(&format!("ALTER TABLE {} ADD COLUMN {}", table, ddl), [])
                .map_err(|e| format!("migrate {}.{}: {}", table, name, e))?;
        }
    }
    Ok(())
}

/// 删除笔记的库行：先清 project_documents 归属行，再删 articles 本行。
/// project_documents.article_id 外键指向 articles(id)，不清归属行直接删文章会
/// FOREIGN KEY constraint failed（AG-06 顺手修复的线上 bug）。
/// 语义：笔记删除即解除项目归属；悬空 parent 在读取侧自动降根（NB-19）。
/// 同步清理该文档的 revision/operation 孤儿行（无 FK，不清理会残留审计垃圾）。
/// 文件与向量索引的清理不在这里——由调用方尽力而为，失败不回滚。
pub fn delete_article_rows(conn: &rusqlite::Connection, id: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM project_documents WHERE article_id = ?1",
        rusqlite::params![id],
    )?;
    conn.execute(
        "DELETE FROM document_revisions WHERE document_id = ?1",
        rusqlite::params![id],
    )?;
    conn.execute(
        "DELETE FROM document_operations WHERE document_id = ?1",
        rusqlite::params![id],
    )?;
    conn.execute("DELETE FROM articles WHERE id = ?1", rusqlite::params![id])?;
    Ok(())
}

/// 在同一连接/事务中批量删除笔记库行，返回实际存在并被删除的 ID。
/// 调用方负责在事务提交后清理对应 Markdown 文件；这里不跨越数据库事务做文件 IO。
pub fn delete_articles_rows(
    conn: &rusqlite::Connection,
    ids: &[String],
) -> Result<Vec<String>, rusqlite::Error> {
    let mut seen = std::collections::HashSet::new();
    let mut deleted = Vec::new();

    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM articles WHERE id = ?1)",
            rusqlite::params![id],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            continue;
        }
        delete_article_rows(conn, id)?;
        deleted.push(id.clone());
    }

    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_items_gain_immutable_seven_day_ttl_and_ledger() {
        let conn = rusqlite::Connection::open_in_memory().expect("open legacy db");
        conn.execute_batch(
            "CREATE TABLE items (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL,
                item_type TEXT NOT NULL,
                title TEXT NOT NULL,
                fetched_at TIMESTAMP,
                status TEXT DEFAULT 'unread'
             );
             INSERT INTO items (id, source_id, item_type, title, fetched_at)
             VALUES ('legacy-1', 'legacy-source', 'article', 'Legacy', '2026-08-10 12:34:56');",
        )
        .expect("create legacy items");

        create_schema(&conn).expect("migrate schema");
        let ttl: (String, String, String) = conn
            .query_row(
                "SELECT first_fetched_at, last_seen_at, expires_at FROM items WHERE id = 'legacy-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read migrated ttl");
        assert_eq!(ttl.0, "2026-08-10 12:34:56");
        assert_eq!(ttl.1, "2026-08-10 12:34:56");
        assert_eq!(ttl.2, "2026-08-17 12:34:56");
        let ledger: (String, String) = conn
            .query_row(
                "SELECT first_fetched_at, expires_at FROM inbox_item_ttl WHERE item_id = 'legacy-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated ledger");
        assert_eq!(ledger, (ttl.0, ttl.2));
    }

    /// 内存库 + 强制外键 + 全量 schema。单测不播种默认数据。
    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory db");
        // rusqlite 默认不强制外键；测试里显式开启，与线上观察到的约束失败行为对齐
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign_keys");
        create_schema(&conn).expect("create_schema");
        conn
    }

    fn insert_article(conn: &rusqlite::Connection, id: &str) {
        conn.execute(
            "INSERT INTO articles (id, title, content) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, format!("title-{}", id), "body"],
        )
        .expect("insert article");
    }

    fn insert_project(conn: &rusqlite::Connection, id: &str) {
        conn.execute(
            "INSERT INTO projects (id, name) VALUES (?1, ?2)",
            rusqlite::params![id, format!("proj-{}", id)],
        )
        .expect("insert project");
    }

    fn count(conn: &rusqlite::Connection, sql: &str, id: &str) -> i64 {
        conn.query_row(sql, rusqlite::params![id], |r| r.get::<_, i64>(0))
            .expect("count query")
    }

    #[test]
    fn member_article_deletes_and_clears_project_membership() {
        let conn = mem_db();
        insert_article(&conn, "a1");
        insert_project(&conn, "p1");
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id) VALUES (?1, ?2)",
            rusqlite::params!["p1", "a1"],
        )
        .expect("insert membership");

        delete_article_rows(&conn, "a1").expect("delete member article");

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a1"),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM project_documents WHERE article_id = ?1",
                "a1"
            ),
            0
        );
    }

    #[test]
    fn article_delete_clears_revision_and_operation_orphans() {
        let conn = mem_db();
        insert_article(&conn, "a1");
        conn.execute(
            "INSERT INTO document_revisions
             (id, document_id, version, content_hash, content_snapshot, created_at)
             VALUES ('rev-1', 'a1', 1, 'h', 'old', 1)",
            [],
        )
        .expect("insert revision");
        conn.execute(
            "INSERT INTO document_operations
             (id, idempotency_key, document_id, operation_type, base_version, status,
              created_at, updated_at)
             VALUES ('op-1', 'key-1', 'a1', 'patch', 0, 'committed', 1, 1)",
            [],
        )
        .expect("insert operation");

        delete_article_rows(&conn, "a1").expect("delete article");

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a1"),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM document_revisions WHERE document_id = ?1",
                "a1"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM document_operations WHERE document_id = ?1",
                "a1"
            ),
            0
        );
    }

    #[test]
    fn plain_article_without_membership_deletes_fine() {
        let conn = mem_db();
        insert_article(&conn, "a2");

        delete_article_rows(&conn, "a2").expect("delete plain article");

        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a2"),
            0
        );
    }

    #[test]
    fn regression_raw_delete_of_member_article_hits_foreign_key() {
        // 回归基线：不经 delete_article_rows、直接删归属过项目的笔记，
        // 在 foreign_keys=ON 时必须失败——证明上面的先清归属是必要修复，不是多余步骤
        let conn = mem_db();
        insert_article(&conn, "a3");
        insert_project(&conn, "p3");
        conn.execute(
            "INSERT INTO project_documents (project_id, article_id) VALUES (?1, ?2)",
            rusqlite::params!["p3", "a3"],
        )
        .expect("insert membership");

        let err = conn
            .execute(
                "DELETE FROM articles WHERE id = ?1",
                rusqlite::params!["a3"],
            )
            .expect_err("raw delete must fail under FK");
        assert!(
            err.to_string().contains("FOREIGN KEY"),
            "unexpected error: {}",
            err
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a3"),
            1
        );
    }

    #[test]
    fn batch_delete_removes_every_selected_article_once() {
        let conn = mem_db();
        insert_article(&conn, "a1");
        insert_article(&conn, "a2");
        insert_article(&conn, "keep");
        insert_project(&conn, "p1");
        for id in ["a1", "a2"] {
            conn.execute(
                "INSERT INTO project_documents (project_id, article_id) VALUES (?1, ?2)",
                rusqlite::params!["p1", id],
            )
            .expect("insert membership");
        }

        let deleted = delete_articles_rows(&conn, &["a1".into(), "a2".into(), "a1".into()])
            .expect("batch delete");

        assert_eq!(deleted, vec!["a1".to_string(), "a2".to_string()]);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a1"),
            0
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "a2"),
            0
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM articles WHERE id = ?1", "keep"),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM project_documents WHERE project_id = ?1",
                "p1"
            ),
            0
        );
    }

    fn pomodoro(
        id: &str,
        task_id: Option<&str>,
        started_at: &str,
        completed: bool,
    ) -> PomodoroSession {
        PomodoroSession {
            id: id.to_string(),
            task_id: task_id.map(|s| s.to_string()),
            planned_minutes: 25,
            started_at: started_at.to_string(),
            ended_at: Some(started_at.to_string()),
            completed,
        }
    }

    #[test]
    fn pomodoro_sessions_roundtrip_and_filter_since() {
        let conn = mem_db();
        insert_pomodoro_session(
            &conn,
            &pomodoro("p1", Some("t1"), "2026-08-19T09:00:00Z", true),
        )
        .expect("insert p1");
        insert_pomodoro_session(&conn, &pomodoro("p2", None, "2026-08-20T08:00:00Z", false))
            .expect("insert p2");
        insert_pomodoro_session(
            &conn,
            &pomodoro("p3", Some("t2"), "2026-08-20T10:00:00Z", true),
        )
        .expect("insert p3");

        let all = list_pomodoro_sessions(&conn, None).expect("list all");
        assert_eq!(all.len(), 3);
        // 倒序：最新在前
        assert_eq!(all[0].id, "p3");
        assert_eq!(all[2].id, "p1");
        assert_eq!(all[1].task_id, None);
        assert!(!all[1].completed);

        let today =
            list_pomodoro_sessions(&conn, Some("2026-08-20T00:00:00Z")).expect("list since");
        assert_eq!(today.len(), 2);
        assert_eq!(today[0].id, "p3");
        assert_eq!(today[1].id, "p2");
    }

    #[test]
    fn pomodoro_insert_is_idempotent_replace() {
        let conn = mem_db();
        let mut session = pomodoro("p1", Some("t1"), "2026-08-20T08:00:00Z", false);
        insert_pomodoro_session(&conn, &session).expect("insert");
        // 同一 id 再次写入 = 覆盖（放弃→完成）
        session.completed = true;
        insert_pomodoro_session(&conn, &session).expect("replace");

        let all = list_pomodoro_sessions(&conn, None).expect("list");
        assert_eq!(all.len(), 1);
        assert!(all[0].completed);
    }
}
