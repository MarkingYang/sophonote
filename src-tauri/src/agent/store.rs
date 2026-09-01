// ============================================================
// Track B · Phase 2 (AG-13 追加)：RunStore — AgentEvent 持久化层
// 实施基线：docs/architecture.md
// 核心职责：单 Run 内按 seq 顺序存事件 + 支持 after_seq 重放 + Snapshot 快照；
//           提供 Thread/Run/Message/ToolCall 的基础 CRUD（非 UI 直用，供命令层调）。
//
// 约束：
// - SQLite 为唯一真相源；表通过 CREATE IF NOT EXISTS + ensure_columns(§db.rs) 幂等创建；
// - Event 表有唯一索引 (run_id, seq)；插入前检查防重复（seq 递增由 Emitter 保证，
//   但跨进程需去重）；
// - 事务边界：一个 run 批量写入多个事件时用单个事务包起来（recovery = 全部回滚或全部提交）；
// - 不阻塞主循环：emit_opt 在 Spike 期可静默失败（暂不在本模块硬拒，发失败仅 println）；
// - 零 rig 类型（硬性限制⑤）。
// ============================================================
use rusqlite::{Connection, OptionalExtension};

use crate::agent::events::AgentEvent;
use crate::agent::types::{
    AgentApproval, AgentMessage, AgentRun as DBAgentRun, AgentRunEvent, AgentThread, AgentToolCall,
    RunStatus, ThreadCollection, ThreadStatus,
};

/// 占位标题：空会话 / 新建默认，不可进入历史列表展示
pub fn is_placeholder_thread_title(title: &str) -> bool {
    let t = title.trim();
    t.is_empty() || matches!(t, "新会话" | "新对话" | "未命名会话")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// 由首条用户 Query 与（可选）首条**有效**助手回复生成会话标题
pub fn derive_thread_title_from_messages(msgs: &[AgentMessage]) -> String {
    let user = msgs
        .iter()
        .find(|m| m.role == "user" && !m.content.trim().is_empty())
        .map(|m| collapse_ws(&m.content))
        .unwrap_or_default();
    let assistant = msgs
        .iter()
        .find(|m| {
            m.role == "assistant"
                && !m.content.trim().is_empty()
                && !m.content.starts_with("运行失败：")
                && !m.content.starts_with("运行已取消")
        })
        .map(|m| {
            let line = m
                .content
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_string();
            collapse_ws(&line)
        });
    let u = clip_chars(&user, 36);
    if u.is_empty() {
        return "对话".into();
    }
    if let Some(a) = assistant {
        let a = clip_chars(&a, 20);
        if !a.is_empty() {
            return format!("{u} · {a}");
        }
    }
    u
}

/// 运行时错误分类（RunStore 暴露给命令层的 error domain）
/// 注意：不能 derive Clone——rusqlite::Error 非 Clone。
#[derive(Debug)]
pub enum RunStoreError {
    Sql(rusqlite::Error),
    Generic(String),
}

impl std::fmt::Display for RunStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "SQLite 错误: {e}"),
            Self::Generic(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RunStoreError {}

impl From<rusqlite::Error> for RunStoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

/// AG-20：Snapshot 内消息尾部条数上限（Thread 级消息只带最近 N 条，
/// 完整历史走 agent_thread_history 全量路径）
pub const SNAPSHOT_MESSAGE_TAIL: usize = 50;

/// AG-20（审计 P0-3 整改项⑤）：真正的 Run 状态快照（可重建 UI 状态）。
/// 与旧版「最后一条事件」伪快照的区别：每一路都直接取自真相源表，
/// 前端拿到即可完整重同步（events 经 handleEvent 回灌，其余供 Worklog/审批面消费）。
/// serde camelCase 对齐前端惯例（runId/threadId/runStatus/latestSeq/…）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run_id: String,
    pub thread_id: String,
    /// Run 状态（来自 agent_runs 表，不经事件推断）
    pub run_status: crate::agent::types::RunStatus,
    /// 已持久化事件的最新 seq（无事件 = 0）
    pub latest_seq: u64,
    /// Run 全量事件 JSON（seq 升序、含 seq=0）——前端经同一 handleEvent 链路回灌，
    /// eventId 幂等去重保证与实时 Channel 并存不重复
    pub events: Vec<String>,
    /// Thread 消息尾部（最近 SNAPSHOT_MESSAGE_TAIL 条，时序升序）
    pub messages: Vec<AgentMessage>,
    /// 工具调用状态（created_at 升序；含参数/结果/错误/preresolved）
    pub tool_calls: Vec<AgentToolCall>,
    /// 待审批项（status = pending）
    pub pending_approvals: Vec<AgentApproval>,
}

/// 从 Connection 构建 RunStore 的便捷方法。
/// Spike 期通常传 app.handle().state::<DbConn>() 拿到 connection ref；
/// Phase 2 完整化后改为注入 shared Arc<Mutex<Connection>>。
pub type DbConn = Connection;

/// RunStore 直接持有连接（命令层每次 open 独立连接再 move 进来，见 commands.rs）。
/// 需要事务的方法（save_events_batch）用 &mut self；Phase 2 完整化后若需跨命令
/// 共享，再改为注入 shared Arc<Mutex<Connection>>。
pub struct RunStore {
    conn: Connection,
}

impl RunStore {
    /// 新建 RunStore（传入已打开的 SQLite 连接）
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    // ------------------- Thread CRUD -------------------

    fn map_thread_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentThread> {
        Ok(AgentThread {
            id: row.get(0)?,
            title: row.get(1)?,
            status: serde_json::from_str::<ThreadStatus>(&row.get::<_, String>(2)?)
                .unwrap_or(ThreadStatus::Failed),
            project_id: row.get(3)?,
            latest_run_id: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            closed_at: row.get(7)?,
            archived_at: row.get(8)?,
            pinned_at: row.get(9)?,
            collection_id: row.get(10)?,
        })
    }

    /// 创建一个新 Thread
    pub fn create_thread(
        &self,
        id: &str,
        title: &str,
        project_id: Option<&str>,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let thread = AgentThread::new(
            id.into(),
            title.into(),
            project_id.map(|s| s.to_string()),
            now_ms,
        );
        self.conn.execute(
            r#"INSERT OR REPLACE INTO agent_threads
               (id, title, status, project_id, latest_run_id, created_at, updated_at, closed_at, archived_at, pinned_at, collection_id)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, NULL)"#,
            rusqlite::params![
                thread.id,
                thread.title,
                serde_json::to_string(&thread.status).unwrap_or_default(),
                thread.project_id,
                thread.latest_run_id,
                thread.created_at,
                thread.updated_at,
            ],
        )?;
        Ok(())
    }

    /// 获取指定 Thread
    pub fn get_thread(&self, id: &str) -> Result<Option<AgentThread>, RunStoreError> {
        let sql = "SELECT id, title, status, project_id, latest_run_id, created_at, updated_at, closed_at, archived_at, pinned_at, collection_id FROM agent_threads WHERE id = ?1";
        let mut stmt = self.conn.prepare(sql)?;
        let row = stmt
            .query_row(rusqlite::params![id], Self::map_thread_row)
            .optional()?;
        Ok(row)
    }

    /// Hermes Session 只在 Thread 上保存一次映射；Run 通过该映射复用同一会话。
    pub fn external_session_id_for_thread(
        &self,
        id: &str,
    ) -> Result<Option<String>, RunStoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT external_session_id FROM agent_threads WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn bind_thread_external_session(
        &self,
        id: &str,
        external_session_id: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let changed = self.conn.execute(
            "UPDATE agent_threads SET external_session_id = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![external_session_id, now_ms, id],
        )?;
        if changed == 0 {
            return Err(RunStoreError::Generic(format!("Thread {id} 不存在")));
        }
        Ok(())
    }

    /// 列出某个项目下的 Thread（None = 全局话题）；按 scope 过滤活跃/历史，永不返回已归档。
    pub fn list_threads(
        &self,
        project_id: Option<&str>,
        scope: crate::agent::types::ThreadListScope,
    ) -> Result<Vec<AgentThread>, RunStoreError> {
        use crate::agent::types::ThreadListScope;
        let scope_sql = match scope {
            ThreadListScope::Active => "closed_at IS NULL AND archived_at IS NULL",
            // 历史：须有用户对话 + 非占位标题（空会话关闭时已硬删，此过滤为兜底）
            ThreadListScope::History => {
                "closed_at IS NOT NULL AND archived_at IS NULL \
                 AND EXISTS (SELECT 1 FROM agent_messages m \
                     WHERE m.thread_id = agent_threads.id AND m.role = 'user' \
                       AND length(trim(m.content)) > 0) \
                 AND length(trim(title)) > 0 \
                 AND trim(title) NOT IN ('新会话', '新对话', '未命名会话')"
            }
        };
        let sql = match project_id {
            Some(_pid) => format!(
                "SELECT id, title, status, project_id, latest_run_id, created_at, updated_at, closed_at, archived_at, pinned_at, collection_id \
                 FROM agent_threads WHERE project_id = ?1 AND {scope_sql} ORDER BY updated_at DESC"
            ),
            None => format!(
                "SELECT id, title, status, project_id, latest_run_id, created_at, updated_at, closed_at, archived_at, pinned_at, collection_id \
                 FROM agent_threads WHERE project_id IS NULL AND {scope_sql} ORDER BY updated_at DESC"
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = match project_id {
            Some(pid) => stmt.query_map(rusqlite::params![pid], Self::map_thread_row)?,
            None => stmt.query_map(rusqlite::params![], Self::map_thread_row)?,
        };
        let mut threads = Vec::new();
        for r in rows {
            threads.push(r?);
        }
        Ok(threads)
    }

    /// 关闭会话 → 进入历史（可恢复）。
    /// 无任何用户对话的空会话直接硬删，不进历史；返回 `true`=进历史，`false`=已丢弃。
    /// 进历史前若标题仍为占位，则按首条 Query（及回复）生成明确标题。
    pub fn close_thread(&self, id: &str, now_ms: u64) -> Result<bool, RunStoreError> {
        let msgs = self.get_messages(id)?;
        let has_user = msgs
            .iter()
            .any(|m| m.role == "user" && !m.content.trim().is_empty());
        if !has_user {
            self.delete_thread(id)?;
            return Ok(false);
        }
        // 进历史前须已有用户对话；标题若仍占位则用 Query+回复生成（无回复时仅用 Query）
        if let Some(thread) = self.get_thread(id)? {
            if is_placeholder_thread_title(&thread.title) {
                let title = derive_thread_title_from_messages(&msgs);
                self.update_thread_title(id, &title, now_ms)?;
            }
        }
        self.conn.execute(
            "UPDATE agent_threads SET closed_at = ?1, updated_at = ?1 WHERE id = ?2 AND archived_at IS NULL",
            rusqlite::params![now_ms, id],
        )?;
        Ok(true)
    }

    /// 硬删 Thread（CASCADE 清 messages；runs/events 显式清理防孤儿）
    pub fn delete_thread(&self, id: &str) -> Result<(), RunStoreError> {
        let runs = self.list_runs_by_thread(id)?;
        for run in &runs {
            let _ = self
                .conn
                .execute("DELETE FROM agent_run_events WHERE run_id = ?1", [&run.id]);
            let _ = self
                .conn
                .execute("DELETE FROM agent_tool_calls WHERE run_id = ?1", [&run.id]);
            let _ = self
                .conn
                .execute("DELETE FROM agent_approvals WHERE run_id = ?1", [&run.id]);
            let _ = self
                .conn
                .execute("DELETE FROM agent_messages WHERE run_id = ?1", [&run.id]);
            let _ = self
                .conn
                .execute("DELETE FROM agent_runs WHERE id = ?1", [&run.id]);
        }
        let _ = self
            .conn
            .execute("DELETE FROM agent_messages WHERE thread_id = ?1", [id]);
        self.conn
            .execute("DELETE FROM agent_threads WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn update_thread_title(
        &self,
        id: &str,
        title: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE agent_threads SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![trimmed, now_ms, id],
        )?;
        Ok(())
    }

    /// 有完整对话（用户 Query + 助手回复）后生成/精炼标题。
    /// 仅用户消息时不改标题，避免「未返回就定名」。
    pub fn refresh_thread_title_from_messages(
        &self,
        thread_id: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let Some(thread) = self.get_thread(thread_id)? else {
            return Ok(());
        };
        let msgs = self.get_messages(thread_id)?;
        let has_user = msgs
            .iter()
            .any(|m| m.role == "user" && !m.content.trim().is_empty());
        let has_assistant = msgs
            .iter()
            .any(|m| m.role == "assistant" && !m.content.trim().is_empty());
        if !(has_user && has_assistant) {
            return Ok(());
        }
        // 失败文案不算有效回复
        let has_real_assistant = msgs.iter().any(|m| {
            m.role == "assistant"
                && !m.content.trim().is_empty()
                && !m.content.starts_with("运行失败：")
                && !m.content.starts_with("运行已取消")
        });
        if !has_real_assistant {
            return Ok(());
        }
        let title = derive_thread_title_from_messages(&msgs);
        if title == thread.title {
            return Ok(());
        }
        // 占位标题，或仅有 Query 截断标题（无 · 回复摘要）时更新
        if is_placeholder_thread_title(&thread.title) || !thread.title.contains('·') {
            self.update_thread_title(thread_id, &title, now_ms)?;
        }
        Ok(())
    }

    /// 从历史恢复为活跃 tab
    pub fn reopen_thread(&self, id: &str, now_ms: u64) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_threads SET closed_at = NULL, updated_at = ?1 WHERE id = ?2 AND archived_at IS NULL",
            rusqlite::params![now_ms, id],
        )?;
        Ok(())
    }

    /// 归档：UI 不可见，待 TTL 硬删
    pub fn archive_thread(&self, id: &str, now_ms: u64) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_threads SET archived_at = ?1, closed_at = COALESCE(closed_at, ?1), updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now_ms, id],
        )?;
        Ok(())
    }

    /// 置顶/取消置顶。组织性操作不更新 updated_at，避免打扰「最近」时序。
    pub fn set_thread_pinned(
        &self,
        id: &str,
        pinned: bool,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let pinned_at = if pinned { Some(now_ms) } else { None };
        self.conn.execute(
            "UPDATE agent_threads SET pinned_at = ?1 WHERE id = ?2",
            rusqlite::params![pinned_at, id],
        )?;
        Ok(())
    }

    /// 收藏夹分类列表（创建时间升序，稳定展示）
    pub fn list_collections(&self) -> Result<Vec<ThreadCollection>, RunStoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at FROM thread_collections ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ThreadCollection {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?;
        let mut collections = Vec::new();
        for r in rows {
            collections.push(r?);
        }
        Ok(collections)
    }

    /// 新建收藏夹分类：名称 trim 后 1–40 字符、同名拒绝（友好错误先于唯一索引兜底）。
    pub fn create_collection(
        &self,
        id: &str,
        name: &str,
        now_ms: u64,
    ) -> Result<ThreadCollection, RunStoreError> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 40 {
            return Err(RunStoreError::Generic("分类名称须为 1–40 个字符".into()));
        }
        let dup: i64 = self.conn.query_row(
            "SELECT COUNT(1) FROM thread_collections WHERE name = ?1",
            rusqlite::params![trimmed],
            |row| row.get(0),
        )?;
        if dup > 0 {
            return Err(RunStoreError::Generic(format!(
                "已存在同名分类「{trimmed}」"
            )));
        }
        self.conn.execute(
            "INSERT INTO thread_collections (id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, trimmed, now_ms],
        )?;
        Ok(ThreadCollection {
            id: id.to_string(),
            name: trimmed.to_string(),
            created_at: now_ms,
        })
    }

    /// 会话加入/移动/移出收藏夹（collection_id = None 即移出）。目标分类须存在。
    pub fn set_thread_collection(
        &self,
        id: &str,
        collection_id: Option<&str>,
    ) -> Result<(), RunStoreError> {
        if let Some(cid) = collection_id {
            let exists: i64 = self.conn.query_row(
                "SELECT COUNT(1) FROM thread_collections WHERE id = ?1",
                rusqlite::params![cid],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(RunStoreError::Generic("目标收藏夹分类不存在".into()));
            }
        }
        self.conn.execute(
            "UPDATE agent_threads SET collection_id = ?1 WHERE id = ?2",
            rusqlite::params![collection_id, id],
        )?;
        Ok(())
    }

    /// 硬删逾 TTL 的**已归档**会话（CASCADE 清子表）。
    /// 普通活跃与仅关闭进历史的会话永久保留，不在此清理。
    /// `ttl_days == 0` 表示不自动清理。
    pub fn gc_expired_threads(&self, ttl_days: u32, now_ms: u64) -> Result<usize, RunStoreError> {
        if ttl_days == 0 {
            return Ok(0);
        }
        let ttl_ms = (ttl_days as u64).saturating_mul(24 * 60 * 60 * 1000);
        let cutoff = now_ms.saturating_sub(ttl_ms);
        let n = self.conn.execute(
            r#"DELETE FROM agent_threads
               WHERE archived_at IS NOT NULL AND archived_at < ?1"#,
            rusqlite::params![cutoff],
        )?;
        Ok(n)
    }

    /// 更新 Thread 状态
    pub fn update_thread_status(
        &self,
        id: &str,
        status: &ThreadStatus,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_threads SET status = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![
                serde_json::to_string(status).unwrap_or_default(),
                now_ms,
                id,
            ],
        )?;
        Ok(())
    }

    /// 设置最新运行 ID
    /// AG-22：updated_at 用真实时间戳（审计 P1-2「部分 updated_at 写入为 0」根治）
    pub fn set_latest_run_id(
        &self,
        thread_id: &str,
        run_id: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_threads SET latest_run_id = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![run_id, now_ms, thread_id],
        )?;
        Ok(())
    }

    // ------------------- Run CRUD -------------------

    /// 创建一个新 Run
    /// AG-22：prompt_version 随创建落库（命令层从 PromptRegistry 取真实版本，
    /// 审计 P1-2「运行元数据与真实调用一致」——此前恒 NULL）
    #[allow(clippy::too_many_arguments)] // 参数一一对应 agent_runs 创建列，拆结构体反增间接层
    pub fn create_run(
        &self,
        id: &str,
        thread_id: &str,
        project_id: Option<&str>,
        provider: &str,
        model: &str,
        prompt_version: Option<&str>,
        max_model_calls: usize,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let mut run = DBAgentRun::new(
            id.into(),
            thread_id.into(),
            project_id.map(|s| s.to_string()),
            provider.into(),
            model.into(),
            max_model_calls,
            now_ms,
        );
        run.prompt_version = prompt_version.map(|s| s.to_string());
        self.conn.execute(
            r#"INSERT OR REPLACE INTO agent_runs
               (id, thread_id, project_id, status, provider, model, prompt_version,
                max_model_calls, current_model_calls, engine, engine_version,
                created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
            rusqlite::params![
                run.id,
                run.thread_id,
                run.project_id,
                serde_json::to_string(&run.status).unwrap_or_default(),
                run.provider,
                run.model,
                run.prompt_version,
                run.max_model_calls,
                run.current_model_calls,
                run.engine,
                run.engine_version,
                run.created_at,
                run.updated_at,
            ],
        )?;
        Ok(())
    }

    /// 根据 run_id 获取 Run
    pub fn get_run(&self, id: &str) -> Result<Option<DBAgentRun>, RunStoreError> {
        let sql = "SELECT id, thread_id, project_id, status, provider, model, prompt_version,
                           max_model_calls, current_model_calls, engine, engine_version,
                           created_at, updated_at
                   FROM agent_runs WHERE id = ?1";
        let mut stmt = self.conn.prepare(sql)?;
        let row = stmt
            .query_row(rusqlite::params![id], |row: &rusqlite::Row| {
                Ok(DBAgentRun {
                    id: row.get(0)?,
                    thread_id: row.get(1)?,
                    project_id: row.get(2)?,
                    status: serde_json::from_str::<RunStatus>(&row.get::<_, String>(3)?)
                        .unwrap_or(RunStatus::Failed),
                    provider: row.get(4)?,
                    model: row.get(5)?,
                    prompt_version: row.get(6)?,
                    max_model_calls: row.get(7)?,
                    current_model_calls: row.get(8)?,
                    engine: row.get(9)?,
                    engine_version: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            })
            .optional()?;
        Ok(row)
    }

    /// 更新 Run 状态
    pub fn update_run_status(
        &self,
        run_id: &str,
        status: &RunStatus,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_runs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![
                serde_json::to_string(status).unwrap_or_default(),
                now_ms,
                run_id,
            ],
        )?;
        Ok(())
    }

    /// H8：写入本次选型的 engine / engine_version（create_run 后覆盖默认 rig）
    pub fn set_run_engine(
        &self,
        run_id: &str,
        engine: &str,
        engine_version: &str,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_runs SET engine = ?1, engine_version = ?2 WHERE id = ?3",
            rusqlite::params![engine, engine_version, run_id],
        )?;
        Ok(())
    }

    /// 增加 model call 计数（AG-22：updated_at 真实时间戳）
    pub fn increment_model_calls(
        &self,
        run_id: &str,
        count: usize,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_runs SET current_model_calls = current_model_calls + ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![count, now_ms, run_id],
        )?;
        Ok(())
    }

    /// AG-22：终态对账写回真实模型调用数（与 rig turn() 计数一致；
    /// 审计 P1-2「Run 记录与真实请求一致」——current_model_calls 不再恒 0）
    pub fn set_model_calls(
        &self,
        run_id: &str,
        count: usize,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_runs SET current_model_calls = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![count, now_ms, run_id],
        )?;
        Ok(())
    }

    /// 设置 prompt_version（AG-22：updated_at 真实时间戳）
    pub fn set_prompt_version(
        &self,
        run_id: &str,
        version: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_runs SET prompt_version = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![version, now_ms, run_id],
        )?;
        Ok(())
    }

    /// H4：更新 Hermes 外部 Run 对账元数据（列经 ensure_columns 幂等补齐）
    #[allow(clippy::too_many_arguments)]
    pub fn update_run_external_meta(
        &self,
        run_id: &str,
        engine_transport: Option<&str>,
        external_run_id: Option<&str>,
        external_session_id: Option<&str>,
        external_protocol_version: Option<&str>,
        last_external_event_id: Option<&str>,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            r#"UPDATE agent_runs SET
                engine_transport = COALESCE(?1, engine_transport),
                external_run_id = COALESCE(?2, external_run_id),
                external_session_id = COALESCE(?3, external_session_id),
                external_protocol_version = COALESCE(?4, external_protocol_version),
                last_external_event_id = COALESCE(?5, last_external_event_id),
                updated_at = ?6
               WHERE id = ?7"#,
            rusqlite::params![
                engine_transport,
                external_run_id,
                external_session_id,
                external_protocol_version,
                last_external_event_id,
                now_ms,
                run_id,
            ],
        )?;
        Ok(())
    }

    /// 列出某 Thread 的所有 Run（按创建时间降序）
    pub fn list_runs_by_thread(&self, thread_id: &str) -> Result<Vec<DBAgentRun>, RunStoreError> {
        let sql = "SELECT id, thread_id, project_id, status, provider, model, prompt_version,
                              max_model_calls, current_model_calls, engine, engine_version,
                              created_at, updated_at
                      FROM agent_runs WHERE thread_id = ?1 ORDER BY created_at DESC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([thread_id], |row| {
            Ok(DBAgentRun {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                project_id: row.get(2)?,
                status: serde_json::from_str::<RunStatus>(&row.get::<_, String>(3)?)
                    .unwrap_or(RunStatus::Failed),
                provider: row.get(4)?,
                model: row.get(5)?,
                prompt_version: row.get(6)?,
                max_model_calls: row.get(7)?,
                current_model_calls: row.get(8)?,
                engine: row.get(9)?,
                engine_version: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?;
        let mut runs = Vec::new();
        for r in rows {
            runs.push(r?);
        }
        Ok(runs)
    }

    // ------------------- Message CRUD -------------------

    /// 保存一条消息到 Thread
    pub fn save_message(
        &self,
        id: &str,
        thread_id: &str,
        run_id: &str,
        role: &str,
        content: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            r#"INSERT OR REPLACE INTO agent_messages
               (id, thread_id, run_id, role, content, source, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            rusqlite::params![id, thread_id, run_id, role, content, "spike", now_ms,],
        )?;
        Ok(())
    }

    /// 获取 Thread 的所有消息（按创建时间升序）
    pub fn get_messages(&self, thread_id: &str) -> Result<Vec<AgentMessage>, RunStoreError> {
        let sql = "SELECT id, thread_id, run_id, role, content, source, created_at
                   FROM agent_messages WHERE thread_id = ?1 ORDER BY created_at ASC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([thread_id], |row| {
            Ok(AgentMessage {
                id: row.get(0)?,
                thread_id: row.get(1)?,
                run_id: row.get(2)?,
                role: row.get(3)?,
                content: row.get(4)?,
                source: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        let mut msgs = Vec::new();
        for r in rows {
            msgs.push(r?);
        }
        Ok(msgs)
    }

    // ------------------- Tool Call CRUD -------------------

    /// 记录工具调用开始
    pub fn record_tool_call_start(
        &self,
        call_id: &str,
        run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        arguments_json: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        let tc = AgentToolCall::new(
            call_id.into(),
            run_id.into(),
            tool_call_id,
            tool_name,
            Some(arguments_json.to_string()),
            now_ms,
        );
        self.conn.execute(
            r#"INSERT OR REPLACE INTO agent_tool_calls
               (id, run_id, tool_call_id, tool_name, arguments_json, result_text, error_text, preresolved, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            rusqlite::params![
                tc.id, tc.run_id, tc.tool_call_id, tc.tool_name,
                tc.arguments_json, tc.result_text, tc.error_text,
                tc.preresolved, tc.created_at,
            ],
        )?;
        Ok(())
    }

    /// 更新工具调用结果（AG-21：五件套持久化列一并写入）
    #[allow(clippy::too_many_arguments)]
    pub fn update_tool_call_result(
        &self,
        call_id: &str,
        result_text: Option<&str>,
        error_text: Option<&str>,
        preresolved: bool,
        structured_json: Option<&str>,
        ui_artifact_json: Option<&str>,
        provenance_json: Option<&str>,
        truncated: bool,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_tool_calls SET result_text = ?1, error_text = ?2, preresolved = ?3,
             structured_json = ?4, ui_artifact_json = ?5, provenance_json = ?6, truncated = ?7
             WHERE id = ?8",
            rusqlite::params![
                result_text,
                error_text,
                preresolved,
                structured_json,
                ui_artifact_json,
                provenance_json,
                truncated,
                call_id
            ],
        )?;
        Ok(())
    }

    /// AG-21：整行登记 preresolved（跳过执行）的工具调用——
    /// preresolved 不触发 on_start，无起始行，这里一次性落完整行（结果列为空）。
    pub fn record_preresolved_tool_call(
        &self,
        id: &str,
        run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            r#"INSERT OR REPLACE INTO agent_tool_calls
               (id, run_id, tool_call_id, tool_name, arguments_json, result_text, error_text,
                preresolved, created_at, structured_json, ui_artifact_json, provenance_json, truncated)
               VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, 1, ?5, NULL, NULL, NULL, 0)"#,
            rusqlite::params![id, run_id, tool_call_id, tool_name, now_ms],
        )?;
        Ok(())
    }

    /// 获取某个 Run 的所有工具调用（AG-21：含五件套持久化列）
    pub fn get_tool_calls(&self, run_id: &str) -> Result<Vec<AgentToolCall>, RunStoreError> {
        let sql = "SELECT id, run_id, tool_call_id, tool_name, arguments_json, result_text,
                          error_text, preresolved, created_at,
                          structured_json, ui_artifact_json, provenance_json, truncated
                   FROM agent_tool_calls WHERE run_id = ?1 ORDER BY created_at ASC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([run_id], |row| {
            Ok(AgentToolCall {
                id: row.get(0)?,
                run_id: row.get(1)?,
                tool_call_id: row.get(2)?,
                tool_name: row.get(3)?,
                arguments_json: row.get(4)?,
                result_text: row.get(5)?,
                error_text: row.get(6)?,
                preresolved: row.get(7)?,
                created_at: row.get(8)?,
                structured_json: row.get(9)?,
                ui_artifact_json: row.get(10)?,
                provenance_json: row.get(11)?,
                truncated: row.get(12)?,
            })
        })?;
        let mut calls = Vec::new();
        for r in rows {
            calls.push(r?);
        }
        Ok(calls)
    }

    // ------------------- Approval CRUD (预留) -------------------

    /// 创建审批请求
    pub fn create_approval(
        &self,
        id: &str,
        run_id: &str,
        approval_type: &str,
        resource_summary: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            r#"INSERT OR REPLACE INTO agent_approvals
               (id, run_id, approval_type, status, resource_summary, created_at, resolved_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            rusqlite::params![
                id,
                run_id,
                approval_type,
                "pending",
                resource_summary,
                now_ms,
                Option::<u64>::None, // resolved_at：新建审批未决，NULL
            ],
        )?;
        Ok(())
    }

    /// 解决审批
    pub fn resolve_approval(
        &self,
        id: &str,
        decision: &str,
        now_ms: u64,
    ) -> Result<(), RunStoreError> {
        self.conn.execute(
            "UPDATE agent_approvals SET status = ?1, resolved_at = ?2 WHERE id = ?3 AND status = 'pending'",
            rusqlite::params![decision, if decision == "approved" || decision == "rejected" { Some(now_ms) } else { None }, id],
        )?;
        Ok(())
    }

    /// 获取某个 Run 的待处理审批
    pub fn get_pending_approvals(&self, run_id: &str) -> Result<Vec<AgentApproval>, RunStoreError> {
        let sql =
            "SELECT id, run_id, approval_type, status, resource_summary, created_at, resolved_at
                   FROM agent_approvals WHERE run_id = ?1 AND status = 'pending'";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([run_id], |row| {
            Ok(AgentApproval {
                id: row.get(0)?,
                run_id: row.get(1)?,
                approval_type: row.get(2)?,
                status: row.get(3)?,
                resource_summary: row.get(4)?,
                created_at: row.get(5)?,
                resolved_at: row.get(6)?,
            })
        })?;
        let mut approvals = Vec::new();
        for r in rows {
            approvals.push(r?);
        }
        Ok(approvals)
    }

    // ------------------- Event Persistence (核心) -------------------

    /// 保存单个事件（幂等：run_id+seq 已有则跳过）
    /// Spike 期 emit_opt 路径可忽略此失败（暂不硬拒，println 记录即可）
    pub fn save_event(&self, event: &AgentEvent, json_blob: &str) -> Result<(), RunStoreError> {
        let ev = AgentRunEvent::from_agent_event(event, json_blob);
        self.conn.execute(
            r#"INSERT OR IGNORE INTO agent_run_events
               (event_id, thread_id, run_id, seq, timestamp, schema_version, data)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            rusqlite::params![
                ev.event_id,
                ev.thread_id,
                ev.run_id,
                ev.seq,
                ev.timestamp,
                ev.schema_version,
                ev.data,
            ],
        )?;
        Ok(())
    }

    /// 批量保存事件（事务模式——全部成功或全部回滚，用于 recovery 场景）
    /// 需要 &mut self：rusqlite 的 transaction() 要求可变借用（RunStore 独占连接，安全）
    pub fn save_events_batch(
        &mut self,
        events: &[(AgentEvent, String)], // (AgentEvent, json_blob)
    ) -> Result<usize, RunStoreError> {
        let tx = self.conn.transaction()?;
        let mut count = 0;
        for (event, json_blob) in events {
            tx.execute(
                r#"INSERT OR IGNORE INTO agent_run_events
                   (event_id, thread_id, run_id, seq, timestamp, schema_version, data)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                rusqlite::params![
                    &event.event_id,
                    &event.thread_id,
                    &event.run_id,
                    event.seq,
                    event.timestamp,
                    event.schema_version,
                    json_blob,
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    /// 按 after_seq 重放事件（取大于 seq 的所有事件，按 seq 升序）
    /// 前端发现 seq 缺口时请求此接口补全
    pub fn replay_after_seq(
        &self,
        run_id: &str,
        after_seq: u64,
    ) -> Result<Vec<String>, RunStoreError> {
        let sql = "SELECT data FROM agent_run_events
                   WHERE run_id = ?1 AND seq > ?2
                   ORDER BY seq ASC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params![run_id, after_seq], |row| {
            row.get::<_, String>(0)
        })?;
        let mut events = Vec::new();
        for r in rows {
            events.push(r?);
        }
        Ok(events)
    }

    /// 获取某 Run 的全部事件（**含 seq=0**，按 seq 升序）——窗口重挂载全量恢复专用。
    /// replay_after_seq 是排他语义（seq > after_seq），seq=0 的 run_started
    ///（= 用户消息）永远无法经它取回，故全量路径独立存在（AG-17）。
    pub fn all_events_of_run(&self, run_id: &str) -> Result<Vec<String>, RunStoreError> {
        let sql = "SELECT data FROM agent_run_events
                   WHERE run_id = ?1
                   ORDER BY seq ASC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| row.get::<_, String>(0))?;
        let mut events = Vec::new();
        for r in rows {
            events.push(r?);
        }
        Ok(events)
    }

    /// 获取某个 Run 最后一条事件的 seq（用于 Snapshot 判断）
    pub fn latest_seq(&self, run_id: &str) -> Result<u64, RunStoreError> {
        let sql = "SELECT COALESCE(MAX(seq), 0) FROM agent_run_events WHERE run_id = ?1";
        let seq: u64 =
            self.conn
                .query_row(sql, rusqlite::params![run_id], |row: &rusqlite::Row| {
                    row.get(0)
                })?;
        Ok(seq)
    }

    /// AG-20（审计 P0-3 整改项⑤）：真正的 Run 状态快照。
    /// 旧版 snapshot() 只返回最后一条事件，不可重建 UI 状态，已按整改要求移除。
    ///
    /// Snapshot = 从真相源各表直接重建的可恢复状态，至少包含：
    /// Run 状态（agent_runs 表，不经事件推断）、最新 seq、Run 全量事件
    ///（升序含 seq=0，前端经同一 handleEvent 链路回灌）、Thread 消息尾部、
    /// 工具调用状态、待审批项。Run 不存在返回 Ok(None)。
    pub fn state_snapshot(&self, run_id: &str) -> Result<Option<RunSnapshot>, RunStoreError> {
        let run = match self.get_run(run_id)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let latest_seq = self.latest_seq(run_id)?;
        let events = self.all_events_of_run(run_id)?;
        // 消息尾部：Thread 级消息只取最近 N 条（Snapshot 体积有界；
        // 完整历史由 agent_thread_history 全量路径负责）
        let mut messages = self.get_messages(&run.thread_id)?;
        if messages.len() > SNAPSHOT_MESSAGE_TAIL {
            messages = messages.split_off(messages.len() - SNAPSHOT_MESSAGE_TAIL);
        }
        let tool_calls = self.get_tool_calls(run_id)?;
        let pending_approvals = self.get_pending_approvals(run_id)?;
        Ok(Some(RunSnapshot {
            run_id: run.id.clone(),
            thread_id: run.thread_id.clone(),
            run_status: run.status,
            latest_seq,
            events,
            messages,
            tool_calls,
            pending_approvals,
        }))
    }

    /// ISSUE-019：宿主进程重启后，数据库可能遗留 queued/running/waiting_approval，
    /// 但对应执行任务与 CancellationToken 已不存在。此时不能猜 completed，也不能
    /// 让会话永久锁死：以单事务补写 `run_failed{outcome:interrupted}` 并把 Run/Thread
    /// 收敛到失败终态。调用方必须先确认本进程不存在该 Run 的活跃令牌。
    pub fn interrupt_orphaned_run(
        &mut self,
        run_id: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<bool, RunStoreError> {
        let tx = self.conn.transaction()?;
        let row = tx
            .query_row(
                "SELECT thread_id, status FROM agent_runs WHERE id = ?1",
                rusqlite::params![run_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((thread_id, status_json)) = row else {
            return Ok(false);
        };
        let status = serde_json::from_str::<RunStatus>(&status_json).unwrap_or(RunStatus::Failed);
        if matches!(
            status,
            RunStatus::Completed
                | RunStatus::Failed
                | RunStatus::Cancelled
                | RunStatus::Interrupted
        ) {
            return Ok(false);
        }

        let next_seq: u64 = tx.query_row(
            "SELECT COALESCE(MAX(seq) + 1, 0) FROM agent_run_events WHERE run_id = ?1",
            rusqlite::params![run_id],
            |row| row.get(0),
        )?;
        let event = AgentEvent {
            event_id: format!("{}:{}", run_id, next_seq),
            thread_id: thread_id.clone(),
            run_id: run_id.to_string(),
            seq: next_seq,
            timestamp: now_ms,
            schema_version: crate::agent::events::AGENT_EVENT_SCHEMA_VERSION,
            payload: crate::agent::events::AgentEventPayload::RunFailed {
                outcome: "interrupted".into(),
                error: reason.to_string(),
            },
        };
        let json =
            serde_json::to_string(&event).map_err(|e| RunStoreError::Generic(e.to_string()))?;
        tx.execute(
            r#"INSERT OR IGNORE INTO agent_run_events
               (event_id, thread_id, run_id, seq, timestamp, schema_version, data)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            rusqlite::params![
                event.event_id,
                event.thread_id,
                event.run_id,
                event.seq,
                event.timestamp,
                event.schema_version,
                json,
            ],
        )?;
        tx.execute(
            "UPDATE agent_runs SET status = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![
                serde_json::to_string(&RunStatus::Interrupted).unwrap_or_default(),
                now_ms,
                run_id,
            ],
        )?;
        tx.execute(
            "UPDATE agent_threads SET status = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![
                serde_json::to_string(&crate::agent::types::ThreadStatus::Failed)
                    .unwrap_or_default(),
                now_ms,
                thread_id,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 清除某个 Run 的全部事件（清理用，Debug 命令可用）
    pub fn clear_events(&self, run_id: &str) -> Result<(), RunStoreError> {
        self.conn
            .execute("DELETE FROM agent_run_events WHERE run_id = ?1", [run_id])?;
        Ok(())
    }

    /// 删除某个 Run（级联删事件、消息、工具调用）
    pub fn delete_run_cascade(&self, run_id: &str) -> Result<(), RunStoreError> {
        self.conn
            .execute("DELETE FROM agent_run_events WHERE run_id = ?1", [run_id])?;
        self.conn
            .execute("DELETE FROM agent_messages WHERE run_id = ?1", [run_id])?;
        self.conn
            .execute("DELETE FROM agent_tool_calls WHERE run_id = ?1", [run_id])?;
        self.conn
            .execute("DELETE FROM agent_approvals WHERE run_id = ?1", [run_id])?;
        self.conn
            .execute("DELETE FROM agent_runs WHERE id = ?1", [run_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::{AgentEventPayload, AGENT_EVENT_SCHEMA_VERSION};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 内存库辅助（建表内联，不依赖外部文件）
    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn
    }

    const AGENT_SCHEMA_DDL: &str = r#"
        CREATE TABLE IF NOT EXISTS agent_threads (
            id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'running',
            project_id TEXT, latest_run_id TEXT, external_session_id TEXT,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            closed_at INTEGER, archived_at INTEGER,
            pinned_at INTEGER, collection_id TEXT
        );
        CREATE TABLE IF NOT EXISTS thread_collections (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_runs (
            id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, project_id TEXT,
            status TEXT NOT NULL DEFAULT 'queued', provider TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '', prompt_version TEXT,
            max_model_calls INTEGER NOT NULL DEFAULT 6, current_model_calls INTEGER NOT NULL DEFAULT 0,
            engine TEXT NOT NULL DEFAULT 'hermes', engine_version TEXT NOT NULL DEFAULT '0.20',
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_messages (
            id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, run_id TEXT NOT NULL,
            role TEXT NOT NULL, content TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT 'spike',
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_agent_messages_thread ON agent_messages(thread_id);
        CREATE TABLE IF NOT EXISTS agent_tool_calls (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL, arguments_json TEXT, result_text TEXT, error_text TEXT,
            preresolved INTEGER DEFAULT 0, created_at INTEGER NOT NULL,
            structured_json TEXT, ui_artifact_json TEXT, provenance_json TEXT,
            truncated INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS agent_approvals (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, approval_type TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending', resource_summary TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL, resolved_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS agent_run_events (
            event_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, run_id TEXT NOT NULL,
            seq INTEGER NOT NULL, timestamp INTEGER NOT NULL,
            schema_version INTEGER NOT NULL DEFAULT 1, data TEXT NOT NULL,
            UNIQUE(run_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_seq ON agent_run_events(run_id, seq ASC);
    "#;

    /// 初始化内存库并返回 RunStore
    fn test_store() -> RunStore {
        let conn = mem_conn();
        conn.execute_batch(AGENT_SCHEMA_DDL).unwrap();
        RunStore::new(conn)
    }

    #[test]
    fn create_and_read_thread() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store.create_thread("t-1", "测试话题", None, now).unwrap();
        let thread = store.get_thread("t-1").unwrap().unwrap();
        assert_eq!(thread.id, "t-1");
        assert_eq!(thread.title, "测试话题");
        assert!(matches!(thread.status, ThreadStatus::Running));
    }

    #[test]
    fn thread_binds_one_external_hermes_session() {
        let store = test_store();
        store
            .create_thread("t-hermes", "新会话", Some("p-1"), 10)
            .unwrap();
        assert_eq!(
            store.external_session_id_for_thread("t-hermes").unwrap(),
            None
        );
        store
            .bind_thread_external_session("t-hermes", "sophonote-t-hermes", 20)
            .unwrap();
        assert_eq!(
            store
                .external_session_id_for_thread("t-hermes")
                .unwrap()
                .as_deref(),
            Some("sophonote-t-hermes")
        );
    }

    #[test]
    fn thread_pin_and_collection_are_organizational_ops() {
        let store = test_store();
        store.create_thread("t-1", "测试话题", None, 100).unwrap();

        // 置顶不扰动「最近」时序（updated_at 不变）
        store.set_thread_pinned("t-1", true, 200).unwrap();
        let thread = store.get_thread("t-1").unwrap().unwrap();
        assert_eq!(thread.pinned_at, Some(200));
        assert_eq!(thread.updated_at, 100);
        store.set_thread_pinned("t-1", false, 300).unwrap();
        assert_eq!(store.get_thread("t-1").unwrap().unwrap().pinned_at, None);

        // 收藏夹：名称 trim、同名拒绝、空名拒绝
        let col = store
            .create_collection("c-1", "  算法工程师  ", 110)
            .unwrap();
        assert_eq!(col.name, "算法工程师");
        assert!(store.create_collection("c-2", "算法工程师", 120).is_err());
        assert!(store.create_collection("c-3", "   ", 130).is_err());

        // 归属：加入 → 移动校验（目标须存在）→ 移出
        store.set_thread_collection("t-1", Some("c-1")).unwrap();
        assert_eq!(
            store
                .get_thread("t-1")
                .unwrap()
                .unwrap()
                .collection_id
                .as_deref(),
            Some("c-1")
        );
        assert!(store.set_thread_collection("t-1", Some("nope")).is_err());
        store.set_thread_collection("t-1", None).unwrap();
        assert_eq!(
            store.get_thread("t-1").unwrap().unwrap().collection_id,
            None
        );
        assert_eq!(store.list_collections().unwrap().len(), 1);
    }

    #[test]
    fn update_thread_status() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store.create_thread("t-1", "测试话题", None, now).unwrap();
        store
            .update_thread_status("t-1", &ThreadStatus::Completed, now)
            .unwrap();
        let thread = store.get_thread("t-1").unwrap().unwrap();
        assert!(matches!(thread.status, ThreadStatus::Completed));
    }

    #[test]
    fn create_and_read_run() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // AG-22：prompt_version 随创建落库（真实运行元数据，审计 P1-2）
        store
            .create_run(
                "r-1",
                "t-1",
                None,
                "deepseek",
                "deepseek-chat-v3.1",
                Some("agent-chat@v1"),
                6,
                now,
            )
            .unwrap();
        let run = store.get_run("r-1").unwrap().unwrap();
        assert_eq!(run.id, "r-1");
        assert_eq!(run.provider, "deepseek");
        assert_eq!(run.model, "deepseek-chat-v3.1");
        assert_eq!(run.prompt_version.as_deref(), Some("agent-chat@v1"));
        assert_eq!(run.max_model_calls, 6);
        assert_eq!(run.current_model_calls, 0);
    }

    #[test]
    fn increment_model_calls() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .create_run("r-1", "t-1", None, "deepseek", "v3.1", None, 6, now)
            .unwrap();
        store.increment_model_calls("r-1", 1, now + 10).unwrap();
        store.increment_model_calls("r-1", 2, now + 20).unwrap();
        let run = store.get_run("r-1").unwrap().unwrap();
        assert_eq!(run.current_model_calls, 3);
        // AG-22：updated_at 跟随真实时间戳，不再写 0
        assert_eq!(run.updated_at, now + 20);
    }

    /// AG-22（审计 P1-2 整改③）：时间戳与状态迁移真实性——
    /// set_latest_run_id / set_model_calls / set_prompt_version 全写真实 now_ms
    #[test]
    fn ag22_timestamps_and_metadata_are_real() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store.create_thread("t-1", "标题", None, now).unwrap();
        store
            .create_run("r-1", "t-1", None, "kimi", "kimi-latest", None, 6, now)
            .unwrap();

        store.set_latest_run_id("t-1", "r-1", now + 5).unwrap();
        let thread = store.get_thread("t-1").unwrap().unwrap();
        assert_eq!(thread.latest_run_id.as_deref(), Some("r-1"));
        assert_eq!(thread.updated_at, now + 5); // 不再是 0

        store.set_model_calls("r-1", 3, now + 7).unwrap();
        store
            .set_prompt_version("r-1", "agent-chat@v1", now + 8)
            .unwrap();
        let run = store.get_run("r-1").unwrap().unwrap();
        assert_eq!(run.current_model_calls, 3);
        assert_eq!(run.prompt_version.as_deref(), Some("agent-chat@v1"));
        assert_eq!(run.updated_at, now + 8);
    }

    #[test]
    fn save_and_replay_events() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let run_id = "r-1";
        store
            .create_run(run_id, "t-1", None, "deepseek", "v3.1", None, 6, now)
            .unwrap();

        let event1 = AgentEvent {
            event_id: format!("{run_id}:0"),
            thread_id: "t-1".into(),
            run_id: run_id.into(),
            seq: 0,
            timestamp: now,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunStarted {
                user_message: "查天气".into(),
                max_turns: 6,
                context: None,
                skill: None,
            },
        };
        let json1 = serde_json::to_string(&event1).unwrap();
        store.save_event(&event1, &json1).unwrap();

        let event2 = AgentEvent {
            event_id: format!("{run_id}:1"),
            thread_id: "t-1".into(),
            run_id: run_id.into(),
            seq: 1,
            timestamp: now,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::ModelStarted { turn: 1 },
        };
        let json2 = serde_json::to_string(&event2).unwrap();
        store.save_event(&event2, &json2).unwrap();

        // 验证初始 seq
        assert_eq!(store.latest_seq(run_id).unwrap(), 1);

        // 重放 seq 之后
        let replayed = store.replay_after_seq(run_id, 0).unwrap();
        assert_eq!(replayed.len(), 1);
        assert!(replayed[0].contains("model_started"));

        // 重放 seq = 0（应该包含两个）
        let all = store.replay_after_seq(run_id, 0).unwrap();
        assert_eq!(all.len(), 1); // seq > 0

        // 重放 seq = -1 (0) → 应拿到全部
        let all_before_zero = store.replay_after_seq(run_id, 0).unwrap();
        assert_eq!(all_before_zero.len(), 1);

        // 插入最后一个事件
        let event3 = AgentEvent {
            event_id: format!("{run_id}:2"),
            thread_id: "t-1".into(),
            run_id: run_id.into(),
            seq: 2,
            timestamp: now + 100,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunCompleted {
                outcome: "completed".into(),
                final_answer: "done".into(),
                model_calls: 1,
            },
        };
        let json3 = serde_json::to_string(&event3).unwrap();
        store.save_event(&event3, &json3).unwrap();

        // 验证三个事件都能拿到
        let full_replay = store.replay_after_seq(run_id, 0).unwrap();
        assert_eq!(full_replay.len(), 2); // seq > 0 的是 seq 1 和 2
        assert!(full_replay.iter().any(|j| j.contains("model_started")));
        assert!(full_replay.iter().any(|j| j.contains("run_completed")));
    }

    #[test]
    fn save_event_ignore_duplicate() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let json = serde_json::to_string(&AgentEventPayload::RunStarted {
            user_message: "x".into(),
            max_turns: 1,
            context: None,
            skill: None,
        })
        .unwrap();
        let evt = AgentEvent {
            event_id: "r:dup".into(),
            thread_id: "t".into(),
            run_id: "r".into(),
            // seq=1：replay_after_seq 为排他语义（seq > after_seq），
            // seq=0 无法经 after_seq=0 取回（与 transport_tests 同口径）
            seq: 1,
            timestamp: now,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunStarted {
                user_message: "x".into(),
                max_turns: 1,
                context: None,
                skill: None,
            },
        };
        store.save_event(&evt, &json).unwrap();
        store.save_event(&evt, &json).unwrap(); // 二次插入应不影响计数
        let cnt = store.replay_after_seq("r", 0).unwrap().len();
        assert_eq!(cnt, 1); // INSERT OR IGNORE → 不重复
    }

    #[test]
    fn message_crud() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .save_message("m-1", "t-1", "r-1", "user", "hello", now)
            .unwrap();
        store
            .save_message("m-2", "t-1", "r-1", "assistant", "world", now + 100)
            .unwrap();
        let msgs = store.get_messages("t-1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].content, "world");
    }

    #[test]
    fn close_empty_thread_discards_without_history() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .create_thread("t-empty", "新会话", Some("p1"), now)
            .unwrap();
        let kept = store.close_thread("t-empty", now + 1).unwrap();
        assert!(!kept);
        assert!(store.get_thread("t-empty").unwrap().is_none());
        let hist = store
            .list_threads(Some("p1"), crate::agent::types::ThreadListScope::History)
            .unwrap();
        assert!(hist.is_empty());
    }

    #[test]
    fn close_with_messages_enters_history_with_derived_title() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .create_thread("t-chat", "新会话", Some("p1"), now)
            .unwrap();
        store
            .create_run("r-1", "t-chat", Some("p1"), "p", "m", None, 6, now)
            .unwrap();
        store
            .save_message("m-u", "t-chat", "r-1", "user", "分析当前文档结构", now)
            .unwrap();
        store
            .save_message(
                "m-a",
                "t-chat",
                "r-1",
                "assistant",
                "文档分为三部分。\n细节如下",
                now + 1,
            )
            .unwrap();
        let kept = store.close_thread("t-chat", now + 2).unwrap();
        assert!(kept);
        let hist = store
            .list_threads(Some("p1"), crate::agent::types::ThreadListScope::History)
            .unwrap();
        assert_eq!(hist.len(), 1);
        assert!(hist[0].title.contains("分析当前文档结构"));
        assert!(hist[0].title.contains("文档分为三部分"));
        assert!(!is_placeholder_thread_title(&hist[0].title));
    }

    #[test]
    fn refresh_title_waits_for_real_assistant_reply() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .create_thread("t-title", "新会话", Some("p1"), now)
            .unwrap();
        store
            .create_run("r-t", "t-title", Some("p1"), "p", "m", None, 6, now)
            .unwrap();
        store
            .save_message(
                "m-u",
                "t-title",
                "r-t",
                "user",
                "总结一下当前的这篇文档",
                now,
            )
            .unwrap();
        store
            .refresh_thread_title_from_messages("t-title", now + 1)
            .unwrap();
        assert_eq!(
            store.get_thread("t-title").unwrap().unwrap().title,
            "新会话"
        );

        store
            .save_message(
                "m-fail",
                "t-title",
                "r-t",
                "assistant",
                "运行失败：Hermes 引擎暂时不可用",
                now + 2,
            )
            .unwrap();
        store
            .refresh_thread_title_from_messages("t-title", now + 3)
            .unwrap();
        assert_eq!(
            store.get_thread("t-title").unwrap().unwrap().title,
            "新会话"
        );

        store
            .save_message(
                "m-ok",
                "t-title",
                "r-t",
                "assistant",
                "这篇文档讲的是架构分层。",
                now + 4,
            )
            .unwrap();
        store
            .refresh_thread_title_from_messages("t-title", now + 5)
            .unwrap();
        let title = store.get_thread("t-title").unwrap().unwrap().title;
        assert!(title.contains("总结一下当前的这篇文档"));
        assert!(title.contains("这篇文档讲的是架构分层"));
        assert!(title.contains('·'));
    }

    /// AG-17：全量恢复路径必须含 seq=0（run_started = 用户消息），
    /// 且升序返回；同时对照排他语义确实取不到 seq=0（两条路径边界钉死）。
    #[test]
    fn all_events_of_run_includes_seq_zero() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let run_id = "r-hist";

        let e0 = AgentEvent {
            event_id: format!("{run_id}:0"),
            thread_id: "t-hist".into(),
            run_id: run_id.into(),
            seq: 0,
            timestamp: now,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunStarted {
                user_message: "你好".into(),
                max_turns: 3,
                context: None,
                skill: None,
            },
        };
        store
            .save_event(&e0, &serde_json::to_string(&e0).unwrap())
            .unwrap();

        let e1 = AgentEvent {
            event_id: format!("{run_id}:1"),
            thread_id: "t-hist".into(),
            run_id: run_id.into(),
            seq: 1,
            timestamp: now + 100,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunCompleted {
                outcome: "completed".into(),
                final_answer: "你好！".into(),
                model_calls: 1,
            },
        };
        store
            .save_event(&e1, &serde_json::to_string(&e1).unwrap())
            .unwrap();

        // 排他语义：after_seq=0 取不到 seq=0（契约不变）
        assert_eq!(store.replay_after_seq(run_id, 0).unwrap().len(), 1);

        // 全量路径：含 seq=0 且升序
        let all = store.all_events_of_run(run_id).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all[0].contains("run_started"));
        assert!(all[1].contains("run_completed"));

        // 无事件的 Run 返回空（不报错）
        assert!(store.all_events_of_run("r-none").unwrap().is_empty());
    }

    /// AG-20：state_snapshot 必须是可重建状态——Run 状态取自 agent_runs 表
    ///（不经事件推断）、事件全量升序含 seq=0、消息尾部/工具态/待审批齐备
    #[test]
    fn state_snapshot_reconstructs_run_state() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .create_thread("t-snap", "快照测试", None, now)
            .unwrap();
        store
            .create_run("r-snap", "t-snap", None, "deepseek", "v3.1", None, 6, now)
            .unwrap();
        // Run 状态以 runs 表为真相源：显式更新后必须在快照中如实呈现
        store
            .update_run_status("r-snap", &RunStatus::Completed, now)
            .unwrap();

        for (seq, payload) in [
            (
                0u64,
                AgentEventPayload::RunStarted {
                    user_message: "你好".into(),
                    max_turns: 3,
                    context: None,
                    skill: None,
                },
            ),
            (1u64, AgentEventPayload::ModelStarted { turn: 1 }),
            (
                2u64,
                AgentEventPayload::RunCompleted {
                    outcome: "completed".into(),
                    final_answer: "完成".into(),
                    model_calls: 1,
                },
            ),
        ] {
            let e = AgentEvent {
                event_id: format!("r-snap:{seq}"),
                thread_id: "t-snap".into(),
                run_id: "r-snap".into(),
                seq,
                timestamp: now + seq,
                schema_version: AGENT_EVENT_SCHEMA_VERSION,
                payload,
            };
            store
                .save_event(&e, &serde_json::to_string(&e).unwrap())
                .unwrap();
        }

        store
            .save_message("m-1", "t-snap", "r-snap", "user", "你好", now)
            .unwrap();
        store
            .save_message("m-2", "t-snap", "r-snap", "assistant", "完成", now + 10)
            .unwrap();
        store
            .record_tool_call_start(
                "tc-1",
                "r-snap",
                "call-1",
                "calculator",
                "{\"expr\":\"1+1\"}",
                now,
            )
            .unwrap();
        store
            .update_tool_call_result("tc-1", Some("2"), None, false, None, None, None, false)
            .unwrap();
        store
            .create_approval("ap-1", "r-snap", "document_patch", "修改笔记", now)
            .unwrap();

        let snap = store.state_snapshot("r-snap").unwrap().expect("快照应存在");
        assert_eq!(snap.run_id, "r-snap");
        assert_eq!(snap.thread_id, "t-snap");
        assert!(
            matches!(snap.run_status, RunStatus::Completed),
            "Run 状态来自 agent_runs 表"
        );
        assert_eq!(snap.latest_seq, 2);
        assert_eq!(snap.events.len(), 3, "全量事件含 seq=0");
        assert!(snap.events[0].contains("run_started"));
        assert!(snap.events[2].contains("run_completed"));
        assert_eq!(snap.messages.len(), 2);
        assert_eq!(snap.messages[0].role, "user");
        assert_eq!(snap.tool_calls.len(), 1);
        assert_eq!(snap.tool_calls[0].result_text.as_deref(), Some("2"));
        assert_eq!(snap.pending_approvals.len(), 1);
        assert_eq!(snap.pending_approvals[0].id, "ap-1");
    }

    /// AG-20：空 Run 快照可用（latest_seq=0、事件空）；不存在的 Run 返回 None
    #[test]
    fn state_snapshot_empty_run_and_missing_run() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store.create_thread("t-e", "空", None, now).unwrap();
        store
            .create_run("r-empty", "t-e", None, "deepseek", "v3.1", None, 6, now)
            .unwrap();
        let snap = store
            .state_snapshot("r-empty")
            .unwrap()
            .expect("空 Run 也应有快照");
        assert_eq!(snap.latest_seq, 0);
        assert!(snap.events.is_empty());
        assert!(snap.messages.is_empty());
        assert!(store.state_snapshot("r-ghost").unwrap().is_none());
    }

    #[test]
    fn orphaned_nonterminal_run_becomes_interrupted_with_terminal_event() {
        let mut store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store
            .create_thread("t-orphan", "未完成", None, now)
            .unwrap();
        store
            .create_run(
                "r-orphan", "t-orphan", None, "deepseek", "v3.1", None, 6, now,
            )
            .unwrap();
        store
            .update_run_status("r-orphan", &RunStatus::Running, now + 1)
            .unwrap();
        let started = AgentEvent {
            event_id: "r-orphan:0".into(),
            thread_id: "t-orphan".into(),
            run_id: "r-orphan".into(),
            seq: 0,
            timestamp: now,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunStarted {
                user_message: "继续生成".into(),
                max_turns: 3,
                context: None,
                skill: None,
            },
        };
        store
            .save_event(&started, &serde_json::to_string(&started).unwrap())
            .unwrap();

        assert!(store
            .interrupt_orphaned_run("r-orphan", "宿主任务已丢失", now + 2)
            .unwrap());
        let snap = store.state_snapshot("r-orphan").unwrap().unwrap();
        assert!(matches!(snap.run_status, RunStatus::Interrupted));
        assert_eq!(snap.events.len(), 2);
        assert!(snap.events[1].contains("run_failed"));
        assert!(snap.events[1].contains("interrupted"));
        assert!(matches!(
            store.get_thread("t-orphan").unwrap().unwrap().status,
            ThreadStatus::Failed
        ));
        // 已终态再次恢复是 no-op，不得追加第二个终态事件。
        assert!(!store
            .interrupt_orphaned_run("r-orphan", "重复恢复", now + 3)
            .unwrap());
        assert_eq!(store.all_events_of_run("r-orphan").unwrap().len(), 2);
    }

    /// AG-21：五件套持久化往返——start 行默认空五件套，update 后原样读出；
    /// preresolved 整行登记亦可在 get_tool_calls 中追溯（RunStore 可追溯来源）
    #[test]
    fn tool_call_five_pieces_roundtrip_and_preresolved_row() {
        let store = test_store();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store.create_thread("t-tc", "工具", None, now).unwrap();
        store
            .create_run("r-tc", "t-tc", None, "deepseek", "v3.1", None, 6, now)
            .unwrap();

        // 真实执行路径：start → completed（带五件套）
        store
            .record_tool_call_start(
                "tc-a",
                "r-tc",
                "call-a",
                "read_document",
                "{\"articleId\":\"a1\"}",
                now,
            )
            .unwrap();
        let structured = r#"{"articleId":"a1","title":"测试笔记"}"#;
        let artifact = r#"{"kind":"markdown","schemaVersion":1}"#;
        let provenance = r#"[{"source":"project-document","sourceId":"a1"}]"#;
        store
            .update_tool_call_result(
                "tc-a",
                Some("正文…"),
                None,
                false,
                Some(structured),
                Some(artifact),
                Some(provenance),
                true,
            )
            .unwrap();
        // preresolved 路径：无 start，一次性整行登记
        store
            .record_preresolved_tool_call("tc-b", "r-tc", "call-b", "calculator", now + 1)
            .unwrap();

        let calls = store.get_tool_calls("r-tc").unwrap();
        assert_eq!(calls.len(), 2);
        let a = &calls[0];
        assert_eq!(a.tool_call_id, "call-a");
        assert_eq!(a.structured_json.as_deref(), Some(structured));
        assert_eq!(a.ui_artifact_json.as_deref(), Some(artifact));
        assert_eq!(a.provenance_json.as_deref(), Some(provenance));
        assert!(a.truncated);
        assert!(!a.preresolved);
        let b = &calls[1];
        assert!(b.preresolved);
        assert!(b.structured_json.is_none());
        assert!(!b.truncated);
    }
}

// ------------------- AG-14：RunStoreTransport（EventEmitter → RunStore 桥接）-------------------

use std::sync::Arc;

use crate::agent::events::EventTransport;

/// Phase 2 事件持久化传输层：将 EventEmitter 发射的事件写入 RunStore。
/// 线程安全，可跨 await 持有。
///
/// 设计口径：
/// - 每个 Transport 首次写入时惰性打开连接，并在整轮事件热路径复用；不同命令/
///   Run 仍持有各自连接，避免把全局 SQLite 锁跨异步任务共享；
/// - AG-20：本层是事件真相源的写入口——DurableFirstTransport 的主路。
///   写库失败经有限重试（默认 3 次，针对 SQLite busy/locked 瞬态）后仍失败
///   则上抛，由 RunStore-first 语义保证该事件不广播给任何下游；
/// - INSERT OR IGNORE 防重复（event_id 唯一索引）。
pub struct RunStoreTransport {
    db_path: String,
    /// 事件热路径复用同一 SQLite 连接。Hermes 的 1～3 字符 delta 可能在一轮内
    /// 产生数千事件；逐事件 Connection::open 会把文件打开与 schema 探测成本
    /// 直接串进 Gateway 读循环。Mutex 仍保持 EventTransport 的 Send + Sync。
    store: std::sync::Mutex<Option<RunStore>>,
    /// 写入尝试次数上限（含首次）。AG-20：瞬态失败有限重试，
    /// 不无限重试（避免 DB 持续不可用时卡死事件循环）
    max_attempts: u32,
}

impl RunStoreTransport {
    /// 从数据库路径创建传输层（默认 3 次尝试）
    pub fn new(db_path: impl Into<String>) -> Self {
        Self::with_max_attempts(db_path, 3)
    }

    /// 显式指定尝试次数（测试用；<1 按 1 处理）
    pub fn with_max_attempts(db_path: impl Into<String>, max_attempts: u32) -> Self {
        Self {
            db_path: db_path.into(),
            store: std::sync::Mutex::new(None),
            max_attempts: max_attempts.max(1),
        }
    }

    /// 单次写入。首次惰性打开，之后复用连接；若首次打开失败，Option 保持 None，
    /// 有限重试仍可重新连接，且 durable-first 语义不变。
    fn send_once(&self, event: &AgentEvent) -> Result<(), String> {
        let json_blob = serde_json::to_string(event).map_err(|e| e.to_string())?;
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "RunStore 连接锁已损坏".to_string())?;
        if guard.is_none() {
            let conn = Connection::open(&self.db_path).map_err(|e| e.to_string())?;
            *guard = Some(RunStore::new(conn));
        }
        guard
            .as_ref()
            .expect("RunStore 已在上方初始化")
            .save_event(event, &json_blob)
            .map_err(|e| e.to_string())
    }
}

impl EventTransport for RunStoreTransport {
    fn send(&self, event: AgentEvent) -> Result<(), String> {
        let mut last_err = String::new();
        for attempt in 1..=self.max_attempts {
            match self.send_once(&event) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_err = e;
                    if attempt < self.max_attempts {
                        // 瞬态退避（30ms）：SQLite busy/locked 的典型恢复窗口
                        std::thread::sleep(std::time::Duration::from_millis(30));
                    }
                }
            }
        }
        Err(format!(
            "RunStore 写入失败（已尝试 {} 次）: {}",
            self.max_attempts, last_err
        ))
    }
}

/// AG-20（审计 P0-3 整改项①②）：持久化优先组合传输层（RunStore-first）。
/// 取代 agent_run_start 旧的「任一路成功即成功」CompositeTransport：
/// - 主路（RunStore）成功 = 事件已提交，随后才推送次路（Channel）；
/// - 主路失败 = 事件未提交：返回 Err，**任何下游都不会收到该事件**——
///   杜绝「仅屏幕可见」事件（当时看得见、重启后无法恢复）；
/// - 次路失败只造成实时流缺口（DB 已有该事件），前端 seq 缺口检测
///   触发 after_seq 重放即可补齐；
/// - 主路失败时 EventEmitter::emit 返回 EmitError，驱动循环按观测面口径
///   忽略继续运行——该 seq 成为 DB 永久缺口，前端补齐阶梯升级到
///   Snapshot 后仍无法填充时进入显式降级（不猜测、不静默）。
pub struct DurableFirstTransport {
    primary: Arc<dyn EventTransport>,
    secondaries: Vec<Arc<dyn EventTransport>>,
}

impl DurableFirstTransport {
    pub fn new(
        primary: Arc<dyn EventTransport>,
        secondaries: Vec<Arc<dyn EventTransport>>,
    ) -> Self {
        Self {
            primary,
            secondaries,
        }
    }
}

impl EventTransport for DurableFirstTransport {
    fn send(&self, event: AgentEvent) -> Result<(), String> {
        // 主路必须成功（?），否则事件不提交、不广播
        self.primary.send(event.clone())?;
        for transport in &self.secondaries {
            if let Err(e) = transport.send(event.clone()) {
                // 次路尽力而为：缺口由前端重放修复（DB 已提交）
                eprintln!("[agent] 次级传输发送失败（事件已入库，缺口重放可补）: {e}");
            }
        }
        Ok(())
    }
}

/// 组合传输层：同时发送到多个下游，任一失败不影响其他（尽力而为）。
/// 注意：AG-20 起正式运行链路（agent_run_start）改用 DurableFirstTransport
///（RunStore-first，持久化成功才广播）；本类型保留为无持久化要求的
/// 调试/旁路场景使用，不得再用于需要恢复保证的事件链路。
pub struct CompositeTransport {
    transports: Vec<Arc<dyn EventTransport>>,
}

impl CompositeTransport {
    pub fn new(transports: Vec<Arc<dyn EventTransport>>) -> Self {
        Self { transports }
    }
}

impl EventTransport for CompositeTransport {
    fn send(&self, event: AgentEvent) -> Result<(), String> {
        let mut last_err: Option<String> = None;
        let mut ok_count = 0usize;
        for transport in &self.transports {
            match transport.send(event.clone()) {
                Ok(()) => ok_count += 1,
                Err(e) => last_err = Some(e),
            }
        }
        // 任一成功即视为成功（尽力而为）；全部失败才上抛最后一个错误
        // （缺口语义由消费端 after_seq 重放/Snapshot 修复）
        if ok_count == 0 && !self.transports.is_empty() {
            return Err(last_err.unwrap_or_else(|| "所有传输层均失败".into()));
        }
        Ok(())
    }
}

// ------------------- AG-21：RunStoreToolObserver（工具调用 → RunStore 持久化桥） -------------------

use crate::agent::run_controller::ToolCallObserver;
use crate::tools::ToolOutput;

/// 驱动循环经 ToolCallObserver 登记工具执行节奏，本层把五件套写进
/// agent_tool_calls（RunStore 可追溯来源）。
///
/// 口径（与 RunStoreTransport 同款）：
/// - 观测面语义：持久化失败 eprintln 吞掉，**绝不中断 agent 主循环**；
/// - 每次回调开独立连接（Spike 期口径，Phase 2 换注入共享连接）；
/// - 行 id = "tc-{tool_call_id}"：call_id 单 Run 内唯一，start/completed
///   两端可确定性派生同一行 id，无需跨回调传递；
/// - preresolved 调用无 on_start（从未执行）→ on_completed 一次性整行登记。
pub struct RunStoreToolObserver {
    db_path: String,
    run_id: String,
}

impl RunStoreToolObserver {
    pub fn new(db_path: impl Into<String>, run_id: impl Into<String>) -> Self {
        Self {
            db_path: db_path.into(),
            run_id: run_id.into(),
        }
    }

    fn store(&self) -> Result<RunStore, String> {
        let conn = Connection::open(&self.db_path).map_err(|e| e.to_string())?;
        Ok(RunStore::new(conn))
    }

    fn row_id(call_id: &str) -> String {
        format!("tc-{}", call_id)
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

impl ToolCallObserver for RunStoreToolObserver {
    fn on_start(&self, call_id: &str, name: &str, arguments_json: &str) {
        let store = match self.store() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[agent] 工具观察者打开数据库失败（不中断运行）: {e}");
                return;
            }
        };
        if let Err(e) = store.record_tool_call_start(
            &Self::row_id(call_id),
            &self.run_id,
            call_id,
            name,
            arguments_json,
            Self::now_ms(),
        ) {
            eprintln!("[agent] 工具调用开始持久化失败（不中断运行）: {e}");
        }
    }

    fn on_completed(
        &self,
        call_id: &str,
        name: &str,
        _ok: bool,
        error: Option<&str>,
        preresolved: bool,
        output: Option<&ToolOutput>,
    ) {
        let store = match self.store() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[agent] 工具观察者打开数据库失败（不中断运行）: {e}");
                return;
            }
        };
        // preresolved：从未执行 → 无起始行，整行一次性登记
        if preresolved {
            if let Err(e) = store.record_preresolved_tool_call(
                &Self::row_id(call_id),
                &self.run_id,
                call_id,
                name,
                Self::now_ms(),
            ) {
                eprintln!("[agent] preresolved 工具调用持久化失败（不中断运行）: {e}");
            }
            return;
        }
        // 五件套序列化（失败降级 None，不阻断结果登记）
        let (result_text, structured_json, ui_artifact_json, provenance_json, truncated) =
            match output {
                Some(out) => (
                    Some(out.model_text.clone()),
                    serde_json::to_string(&out.structured)
                        .map_err(|e| eprintln!("[agent] structured 序列化失败: {e}"))
                        .ok(),
                    out.ui_artifact.as_ref().and_then(|a| {
                        serde_json::to_string(a)
                            .map_err(|e| eprintln!("[agent] uiArtifact 序列化失败: {e}"))
                            .ok()
                    }),
                    serde_json::to_string(&out.provenance)
                        .map_err(|e| eprintln!("[agent] provenance 序列化失败: {e}"))
                        .ok(),
                    out.truncated,
                ),
                None => (None, None, None, None, false),
            };
        if let Err(e) = store.update_tool_call_result(
            &Self::row_id(call_id),
            result_text.as_deref(),
            error,
            false,
            structured_json.as_deref(),
            ui_artifact_json.as_deref(),
            provenance_json.as_deref(),
            truncated,
        ) {
            eprintln!("[agent] 工具调用结果持久化失败（不中断运行）: {e}");
        }
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use crate::agent::events::{
        AgentEvent, AgentEventPayload, EventTransport, AGENT_EVENT_SCHEMA_VERSION,
    };
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    const AGENT_SCHEMA_DDL: &str = r#"
        CREATE TABLE IF NOT EXISTS agent_threads (
            id TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'running',
            project_id TEXT, latest_run_id TEXT,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            closed_at INTEGER, archived_at INTEGER,
            pinned_at INTEGER, collection_id TEXT
        );
        CREATE TABLE IF NOT EXISTS agent_runs (
            id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, project_id TEXT,
            status TEXT NOT NULL DEFAULT 'queued', provider TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '', prompt_version TEXT,
            max_model_calls INTEGER NOT NULL DEFAULT 6, current_model_calls INTEGER NOT NULL DEFAULT 0,
            engine TEXT NOT NULL DEFAULT 'hermes', engine_version TEXT NOT NULL DEFAULT '0.20',
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS agent_messages (
            id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, run_id TEXT NOT NULL,
            role TEXT NOT NULL, content TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT 'spike',
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_agent_messages_thread ON agent_messages(thread_id);
        CREATE TABLE IF NOT EXISTS agent_tool_calls (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL, arguments_json TEXT, result_text TEXT, error_text TEXT,
            preresolved INTEGER DEFAULT 0, created_at INTEGER NOT NULL,
            structured_json TEXT, ui_artifact_json TEXT, provenance_json TEXT,
            truncated INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS agent_approvals (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, approval_type TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending', resource_summary TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL, resolved_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS agent_run_events (
            event_id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, run_id TEXT NOT NULL,
            seq INTEGER NOT NULL, timestamp INTEGER NOT NULL,
            schema_version INTEGER NOT NULL DEFAULT 1, data TEXT NOT NULL,
            UNIQUE(run_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_seq ON agent_run_events(run_id, seq ASC);
    "#;

    /// 内存录制 transport（验证 CompositeTransport 多路广播）
    struct MockTransport {
        events: Mutex<Vec<AgentEvent>>,
    }

    impl MockTransport {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn count(&self) -> usize {
            self.events.lock().unwrap().len()
        }
    }

    impl EventTransport for MockTransport {
        fn send(&self, event: AgentEvent) -> Result<(), String> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    fn make_test_event(run_id: &str, seq: u64) -> AgentEvent {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        AgentEvent {
            event_id: format!("{}:{}", run_id, seq),
            thread_id: "t-test".into(),
            run_id: run_id.into(),
            seq,
            timestamp: now,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::RunStarted {
                user_message: "test".into(),
                max_turns: 1,
                context: None,
                skill: None,
            },
        }
    }

    #[test]
    fn run_store_transport_writes_event() {
        // 创建临时数据库
        let db_path = "/tmp/sophonote_transport_test.db";
        let _ = std::fs::remove_file(db_path);

        // 初始化 schema（复用 test DDL）
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(AGENT_SCHEMA_DDL).unwrap();

        // 创建 transport 并发送事件（seq=1：replay_after_seq 为排他语义 seq > after_seq，
        // seq=0 的事件无法经 after_seq=0 重放取回）
        let transport = RunStoreTransport::new(db_path);
        let event = make_test_event("r-1", 1);
        transport.send(event).unwrap();

        // 验证事件已写入
        let store = RunStore::new(Connection::open(db_path).unwrap());
        let events = store.replay_after_seq("r-1", 0).unwrap();
        assert_eq!(events.len(), 1);

        // 清理
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn run_store_transport_idempotent() {
        let db_path = "/tmp/sophonote_transport_idem.db";
        let _ = std::fs::remove_file(db_path);
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(AGENT_SCHEMA_DDL).unwrap();

        let transport = RunStoreTransport::new(db_path);
        let event = make_test_event("r-2", 1);

        // 发送两次（event_id 相同）
        transport.send(event.clone()).unwrap();
        transport.send(event).unwrap();

        // 验证只写入一次（INSERT OR IGNORE）
        let store = RunStore::new(Connection::open(db_path).unwrap());
        let events = store.replay_after_seq("r-2", 0).unwrap();
        assert_eq!(events.len(), 1);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn composite_transport_broadcasts() {
        let t1 = Arc::new(MockTransport::new());
        let t2 = Arc::new(MockTransport::new());

        let composite = CompositeTransport::new(vec![
            Arc::clone(&t1) as Arc<dyn EventTransport>,
            Arc::clone(&t2) as Arc<dyn EventTransport>,
        ]);

        let event = make_test_event("r-3", 0);
        composite.send(event).unwrap();

        // 两个 transport 都应收到事件
        assert_eq!(t1.count(), 1);
        assert_eq!(t2.count(), 1);
    }

    #[test]
    fn composite_transport_partial_failure() {
        // 一个成功、一个失败的 transport
        struct FailTransport;
        impl EventTransport for FailTransport {
            fn send(&self, _event: AgentEvent) -> Result<(), String> {
                Err("mock failure".into())
            }
        }

        let t1 = Arc::new(MockTransport::new());
        let t2 = Arc::new(FailTransport) as Arc<dyn EventTransport>;

        let composite =
            CompositeTransport::new(vec![Arc::clone(&t1) as Arc<dyn EventTransport>, t2]);

        let event = make_test_event("r-4", 0);
        let result = composite.send(event);

        // CompositeTransport 不因部分失败而返回错误（尽力而为）
        assert!(result.is_ok());
        // 成功的 transport 仍收到事件
        assert_eq!(t1.count(), 1);
    }

    // ---------------- AG-20：DurableFirstTransport（RunStore-first）护栏 ----------------

    /// AG-20 核心护栏（审计 P0-3 整改项②）：主路失败 → 事件不提交，
    /// 次级（屏幕）什么都收不到，且 send 返回 Err——
    /// 「数据库写失败不产生仅屏幕可见事件」的直接钉死
    #[test]
    fn durable_first_primary_failure_blocks_all_secondaries() {
        struct FailTransport;
        impl EventTransport for FailTransport {
            fn send(&self, _event: AgentEvent) -> Result<(), String> {
                Err("db unavailable".into())
            }
        }
        let secondary = Arc::new(MockTransport::new());
        let durable = DurableFirstTransport::new(
            Arc::new(FailTransport) as Arc<dyn EventTransport>,
            vec![Arc::clone(&secondary) as Arc<dyn EventTransport>],
        );
        let err = durable.send(make_test_event("r-df-deny", 1)).unwrap_err();
        assert!(err.contains("db unavailable"));
        assert_eq!(secondary.count(), 0, "未持久化的事件不得到达屏幕");
    }

    /// 主路成功后次级按序送达（主在前）；某次级失败不影响其余次级与整体成功
    #[test]
    fn durable_first_delivers_secondaries_after_primary() {
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        struct TagTransport {
            tag: &'static str,
            order: Arc<Mutex<Vec<&'static str>>>,
        }
        impl EventTransport for TagTransport {
            fn send(&self, _event: AgentEvent) -> Result<(), String> {
                self.order.lock().unwrap().push(self.tag);
                Ok(())
            }
        }
        struct FailTransport;
        impl EventTransport for FailTransport {
            fn send(&self, _event: AgentEvent) -> Result<(), String> {
                Err("secondary failure".into())
            }
        }
        let durable = DurableFirstTransport::new(
            Arc::new(TagTransport {
                tag: "primary",
                order: Arc::clone(&order),
            }) as Arc<dyn EventTransport>,
            vec![
                Arc::new(FailTransport) as Arc<dyn EventTransport>,
                Arc::new(TagTransport {
                    tag: "secondary",
                    order: Arc::clone(&order),
                }) as Arc<dyn EventTransport>,
            ],
        );
        durable.send(make_test_event("r-df-order", 1)).unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["primary", "secondary"]);
    }

    /// AG-20 有限重试：持续失败（路径不可达）按配置次数尝试后明确 Err——
    /// 不静默吞错，也不无限重试卡死事件循环
    #[test]
    fn run_store_transport_retry_exhaustion_errors() {
        let transport = RunStoreTransport::with_max_attempts("/nonexistent-dir-ag20/sophonote.db", 2);
        let err = transport.send(make_test_event("r-retry", 1)).unwrap_err();
        assert!(err.contains("RunStore 写入失败"), "实际: {err}");
        assert!(err.contains("已尝试 2 次"), "实际: {err}");
    }

    /// 主路 = 真实 RunStoreTransport：端到端钉死「入库成功才广播」的提交序
    #[test]
    fn durable_first_with_real_runstore_commits_before_broadcast() {
        let db_path = "/tmp/sophonote_durable_first.db";
        let _ = std::fs::remove_file(db_path);
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(AGENT_SCHEMA_DDL).unwrap();
        drop(conn);

        let secondary = Arc::new(MockTransport::new());
        let durable = DurableFirstTransport::new(
            Arc::new(RunStoreTransport::new(db_path)) as Arc<dyn EventTransport>,
            vec![Arc::clone(&secondary) as Arc<dyn EventTransport>],
        );
        durable.send(make_test_event("r-df-real", 1)).unwrap();

        // DB 已提交 + 屏幕也送达（两路同事件）
        let store = RunStore::new(Connection::open(db_path).unwrap());
        assert_eq!(store.replay_after_seq("r-df-real", 0).unwrap().len(), 1);
        assert_eq!(secondary.count(), 1);
        let _ = std::fs::remove_file(db_path);
    }

    // ---------------- AG-21：RunStoreToolObserver（五件套落库可追溯） ----------------

    /// AG-21 端到端：真实执行（start→completed 带五件套）、preresolved 整行登记、
    /// 失败路径（error_text）三条通道都进 agent_tool_calls，get_tool_calls 可追溯
    #[test]
    fn tool_observer_persists_five_pieces_and_preresolved() {
        let db_path = "/tmp/sophonote_tool_observer.db";
        let _ = std::fs::remove_file(db_path);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        {
            let conn = Connection::open(db_path).unwrap();
            conn.execute_batch(AGENT_SCHEMA_DDL).unwrap();
            let store = RunStore::new(conn);
            store.create_thread("t-obs", "观察", None, now).unwrap();
            store
                .create_run("r-obs", "t-obs", None, "deepseek", "v3.1", None, 6, now)
                .unwrap();
        }

        let observer = RunStoreToolObserver::new(db_path, "r-obs");

        // 通道一：真实执行成功（start → completed 带五件套）
        observer.on_start("call-1", "read_document", "{\"articleId\":\"a1\"}");
        let artifact = crate::tools::UiArtifact::new(
            "markdown",
            serde_json::json!({"markdown": "正文"}),
            "正文",
            vec![crate::tools::ProvenanceRef::new("project-document").with_id("a1")],
        )
        .expect("allowlist kind");
        let out = ToolOutput {
            model_text: "模型文本".into(),
            structured: serde_json::json!({"articleId": "a1"}),
            ui_artifact: Some(artifact),
            provenance: vec![crate::tools::ProvenanceRef::new("project-document").with_id("a1")],
            truncated: true,
        };
        observer.on_completed("call-1", "read_document", true, None, false, Some(&out));

        // 通道二：preresolved（无 on_start，整行登记）
        observer.on_completed("call-2", "calculator", true, None, true, None);

        // 通道三：执行失败（error_text，无 output）
        observer.on_start(
            "call-3",
            "calculator",
            "{\"op\":\"divide\",\"a\":1,\"b\":0}",
        );
        observer.on_completed(
            "call-3",
            "calculator",
            false,
            Some("除数不能为零"),
            false,
            None,
        );

        let store = RunStore::new(Connection::open(db_path).unwrap());
        let calls = store.get_tool_calls("r-obs").unwrap();
        assert_eq!(calls.len(), 3);
        let by_id = |tid: &str| {
            calls
                .iter()
                .find(|c| c.tool_call_id == tid)
                .unwrap_or_else(|| panic!("缺少 tool_call_id={tid}"))
                .clone()
        };

        let c1 = by_id("call-1");
        assert_eq!(c1.result_text.as_deref(), Some("模型文本"));
        assert!(c1
            .structured_json
            .as_deref()
            .unwrap()
            .contains("\"articleId\":\"a1\""));
        assert!(c1
            .ui_artifact_json
            .as_deref()
            .unwrap()
            .contains("\"kind\":\"markdown\""));
        assert!(c1
            .provenance_json
            .as_deref()
            .unwrap()
            .contains("project-document"));
        assert!(c1.truncated);
        assert!(!c1.preresolved);

        let c2 = by_id("call-2");
        assert!(c2.preresolved);
        assert!(c2.result_text.is_none());
        assert!(c2.structured_json.is_none());

        let c3 = by_id("call-3");
        assert_eq!(c3.error_text.as_deref(), Some("除数不能为零"));
        assert!(c3.result_text.is_none());
        assert!(!c3.preresolved);

        let _ = std::fs::remove_file(db_path);
    }
}
