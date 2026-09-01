// ============================================================
// Track B · 智能体演进（AG-07 追加）：版本化内部 AgentEvent + EventEmitter
// 实施基线：docs/architecture.md 事件协议（Spike 子集）+
// 硬性限制⑥（事件先可恢复：event_id/run_id/seq/timestamp/schema_version/终态）。
//
// 口径：
// - seq 在「发射时」分配（与 rig turn() 计数口径一致）；transport 失败即成
//   序号缺口，Phase 2 由 RunStore 重放/Snapshot 修复——不在 Spike 内静默吞掉。
// - 终态事件（run.completed / run.failed / run.cancelled）之后禁止再发普通
//   事件：Rust 侧硬约束，不靠消费端自觉。
// - 本模块零 rig 类型（硬性限制⑤）：payload 全部是自有可序列化结构。
// ============================================================
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 事件协议版本：payload 结构变更必须升版（前端 reducer 按版本分流）。
/// 口径（AG-21/AG-26 定案）：**增量** serde(default) 字段不升版——旧事件
/// 重放链路原样可解析；新增 payload 变体或删改既有字段才升版。
/// H4 / NEXT-021：升至 2（message_delta / engine_degraded 等）。
pub const AGENT_EVENT_SCHEMA_VERSION: u32 = 4;

/// 前端/重放可接受的最低 schema（含历史 v1 事件）
pub const AGENT_EVENT_SCHEMA_MIN: u32 = 1;

/// AG-26 Run 选区上下文快照（run_started 增量字段；前端 Chat 头部渲染
/// 「绑定文章/选区/版本」与审批卡定位用）。字段与前端 SelectionSnapshot 对齐，
/// 但只带展示与审计所需子集——编辑器坐标（proseFrom/proseTo）不进事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunContext {
    pub article_id: String,
    pub title: String,
    /// 选区捕获时刻的文档版本（patch baseVersion 审计链的起点）
    pub base_version: i64,
    /// 选中的 Markdown 原文（Diff 审批卡的「范围」展示）
    pub selected_markdown: String,
    pub selected_text_hash: String,
    /// 选区前后文（各 ≤80 字符，审批卡上下文展示）
    pub before_context: String,
    pub after_context: String,
}

/// AG-27 激活 Skill 引用（run_started 增量字段；Worklog「Run 可见版本与来源」
/// 的数据源）。只带展示/审计所需子集——清单正文与工具清单不进事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSkillRef {
    pub name: String,
    pub version: u32,
    /// bundled / user / workspace（skills::SkillSource 的字符串口径）
    pub source: String,
}

/// Spike/正式事件 payload（docs/architecture.md）。
/// serde 契约：tag = snake_case；字段 = camelCase。
/// H4：新增 message_delta / message_completed / approval_required / engine_degraded。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEventPayload {
    #[serde(rename_all = "camelCase")]
    RunStarted {
        user_message: String,
        max_turns: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<RunContext>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skill: Option<RunSkillRef>,
    },
    #[serde(rename_all = "camelCase")]
    ModelStarted { turn: usize },
    #[serde(rename_all = "camelCase")]
    ToolStarted {
        call_id: String,
        name: String,
        arguments_json: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolCompleted {
        call_id: String,
        name: String,
        ok: bool,
        error: Option<String>,
        preresolved: bool,
        #[serde(default)]
        structured: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui_artifact: Option<crate::tools::UiArtifact>,
        #[serde(default)]
        truncated: bool,
        #[serde(default)]
        provenance: Vec<crate::tools::ProvenanceRef>,
    },
    /// H4：流式 token / 文本增量
    #[serde(rename_all = "camelCase")]
    MessageDelta {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u32>,
    },
    /// H4：一条助手消息定稿（可与后续 run_completed 并存）
    #[serde(rename_all = "camelCase")]
    MessageCompleted { text: String },
    /// Hermes `message.interim`：本轮仍在继续，但当前助手说明已成为独立消息。
    /// Surface 必须保留这个边界，否则后续 `message.complete` 会覆盖过程中已展示的说明。
    #[serde(rename_all = "camelCase")]
    MessageInterim {
        text: String,
        #[serde(default)]
        already_streamed: bool,
    },
    /// Bridge 挂载：模型推理/思考增量（不进 Markdown 正文）
    #[serde(rename_all = "camelCase")]
    ReasoningDelta { text: String },
    /// 推理结束边界（Hermes `reasoning.end` / `reasoning.completed`；
    /// 前端也可由首条 message_delta 合成 thinking_end）
    #[serde(rename_all = "camelCase")]
    ReasoningCompleted {},
    /// H4/H5 占位：审批请求（本轮可不由 stub 发出）
    #[serde(rename_all = "camelCase")]
    ApprovalRequired {
        approval_id: String,
        tool_name: String,
        arguments_json: String,
        #[serde(default)]
        choices: Vec<String>,
    },
    /// Hermes `clarify.request`：Agent 等待用户补充信息，不能降级成普通文本。
    #[serde(rename_all = "camelCase")]
    ClarifyRequired {
        request_id: String,
        question: String,
        #[serde(default)]
        choices: Vec<String>,
    },
    /// H4：引擎降级/重连中（**非终态**）
    #[serde(rename_all = "camelCase")]
    EngineDegraded { reason: String, reconnecting: bool },
    #[serde(rename_all = "camelCase")]
    RunCompleted {
        outcome: String,
        final_answer: String,
        model_calls: usize,
    },
    #[serde(rename_all = "camelCase")]
    RunFailed {
        /// 含 `"interrupted"`（SSE 对账不可恢复，禁止假 completed）
        outcome: String,
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    RunCancelled { reason: String },
}

impl AgentEventPayload {
    /// 终态事件：其后禁止再写普通事件。EngineDegraded **不是**终态。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentEventPayload::RunCompleted { .. }
                | AgentEventPayload::RunFailed { .. }
                | AgentEventPayload::RunCancelled { .. }
        )
    }
}

/// 事件信封：可恢复六要素 + 类型化 payload（信封字段 camelCase 对齐前端惯例）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    /// 全局唯一；格式 {run_id}:{seq}，无需外部 ID 服务即可幂等去重
    pub event_id: String,
    /// Spike 期恒为 "spike"（Phase 2 真实 Thread 注入）
    pub thread_id: String,
    pub run_id: String,
    /// 单 Run 内从 0 严格递增；消费端据此检测缺口与乱序
    pub seq: u64,
    /// 毫秒时间戳（UTC epoch）
    pub timestamp: u64,
    pub schema_version: u32,
    pub payload: AgentEventPayload,
}

/// 事件传输层抽象：单测用内存录制，命令层用 Tauri Channel，
/// Phase 2 换 RunStore 持久化——EventEmitter 不感知具体去向。
pub trait EventTransport: Send + Sync {
    fn send(&self, event: AgentEvent) -> Result<(), String>;
}

/// 发射失败（终态锁定 / transport 拒绝）。发射是观测面：
/// 驱动循环对 EmitError 一律忽略不中断运行（RunStore 落地前不阻塞主流程）。
#[derive(Debug)]
pub struct EmitError(pub String);

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for EmitError {}

/// 单 Run 事件发射器：分配递增 seq、拼装信封、执行终态锁定。
/// 线程安全（Atomic seq + AtomicBool），允许驱动循环跨 await 持有。
pub struct EventEmitter {
    thread_id: String,
    run_id: String,
    next_seq: AtomicU64,
    terminated: AtomicBool,
    transport: Arc<dyn EventTransport>,
}

impl EventEmitter {
    pub fn new(
        thread_id: impl Into<String>,
        run_id: impl Into<String>,
        transport: Arc<dyn EventTransport>,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            run_id: run_id.into(),
            next_seq: AtomicU64::new(0),
            terminated: AtomicBool::new(false),
            transport,
        }
    }

    /// 从持久事件流的下一个序号继续发射。用于宿主重启后重新附着 Hermes
    /// Session；若仍从 0 开始，RunStore 的 event_id 唯一键会把恢复事件静默去重。
    pub fn resume_at(
        thread_id: impl Into<String>,
        run_id: impl Into<String>,
        next_seq: u64,
        transport: Arc<dyn EventTransport>,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            run_id: run_id.into(),
            next_seq: AtomicU64::new(next_seq),
            terminated: AtomicBool::new(false),
            transport,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// 发射一个事件。终态已发过后返回 Err；transport 失败返回 Err 且
    /// seq 不回滚（缺口语义，见模块头口径）。
    pub fn emit(&self, payload: AgentEventPayload) -> Result<(), EmitError> {
        if self.terminated.load(Ordering::Acquire) {
            return Err(EmitError(format!(
                "run {} 已发终态事件，禁止再发普通事件",
                self.run_id
            )));
        }
        let seq = self.next_seq.fetch_add(1, Ordering::AcqRel);
        let terminal = payload.is_terminal();
        let event = AgentEvent {
            event_id: format!("{}:{}", self.run_id, seq),
            thread_id: self.thread_id.clone(),
            run_id: self.run_id.clone(),
            seq,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload,
        };
        self.transport.send(event).map_err(EmitError)?;
        if terminal {
            self.terminated.store(true, Ordering::Release);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 内存录制 transport（单测断言面）
    #[derive(Default)]
    pub(crate) struct RecordingTransport {
        pub(crate) events: Mutex<Vec<AgentEvent>>,
    }

    impl EventTransport for RecordingTransport {
        fn send(&self, event: AgentEvent) -> Result<(), String> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    /// 总是失败的 transport（验证 transport 拒绝的传播与 seq 缺口语义）
    struct FailingTransport;

    impl EventTransport for FailingTransport {
        fn send(&self, _event: AgentEvent) -> Result<(), String> {
            Err("transport 拒收".into())
        }
    }

    fn emitter_with_recorder() -> (Arc<EventEmitter>, Arc<RecordingTransport>) {
        let rec = Arc::new(RecordingTransport::default());
        let em = Arc::new(EventEmitter::new("spike", "run-1", rec.clone()));
        (em, rec)
    }

    #[test]
    fn seq_starts_at_zero_and_increments_with_unique_event_ids() {
        let (em, rec) = emitter_with_recorder();
        for turn in 1..=3 {
            em.emit(AgentEventPayload::ModelStarted { turn })
                .expect("emit");
        }
        let events = rec.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        for (i, e) in events.iter().enumerate() {
            assert_eq!(e.seq, i as u64);
            assert_eq!(e.event_id, format!("run-1:{}", i));
        }
    }

    #[test]
    fn resumed_emitter_continues_after_durable_sequence() {
        let rec = Arc::new(RecordingTransport::default());
        let em = EventEmitter::resume_at("thread-1", "run-1", 42, rec.clone());
        em.emit(AgentEventPayload::ModelStarted { turn: 2 })
            .expect("emit");
        let events = rec.events.lock().unwrap();
        assert_eq!(events[0].seq, 42);
        assert_eq!(events[0].event_id, "run-1:42");
    }

    #[test]
    fn envelope_carries_recoverability_fields() {
        let (em, rec) = emitter_with_recorder();
        em.emit(AgentEventPayload::RunStarted {
            user_message: "查天气".into(),
            max_turns: 6,
            context: None,
            skill: None,
        })
        .expect("emit");
        let e = &rec.events.lock().unwrap()[0];
        assert_eq!(e.thread_id, "spike");
        assert_eq!(e.run_id, "run-1");
        assert_eq!(e.schema_version, AGENT_EVENT_SCHEMA_VERSION);
        assert!(e.timestamp > 0);
        assert_eq!(
            e.payload,
            AgentEventPayload::RunStarted {
                user_message: "查天气".into(),
                max_turns: 6,
                context: None,
                skill: None
            }
        );
    }

    #[test]
    fn terminal_event_locks_the_stream() {
        let (em, rec) = emitter_with_recorder();
        em.emit(AgentEventPayload::ModelStarted { turn: 1 })
            .expect("emit");
        em.emit(AgentEventPayload::RunCompleted {
            outcome: "completed".into(),
            final_answer: "done".into(),
            model_calls: 1,
        })
        .expect("terminal emit");
        // 终态后普通事件被 Rust 侧硬拒
        let err = em
            .emit(AgentEventPayload::ModelStarted { turn: 2 })
            .expect_err("post-terminal emit must fail");
        assert!(err.to_string().contains("终态"));
        // 终态事件本身送达且是最后一个
        let events = rec.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[1].payload.is_terminal());
    }

    #[test]
    fn transport_failure_propagates_and_seq_advances() {
        let em = EventEmitter::new("spike", "run-f", Arc::new(FailingTransport));
        em.emit(AgentEventPayload::ModelStarted { turn: 1 })
            .expect_err("transport 拒收应传播为 EmitError");
        // seq 不回滚：下一次发射 seq=1（缺口 = 丢失事件，Phase 2 RunStore 修复口径）
        let (em2, rec) = emitter_with_recorder();
        em2.emit(AgentEventPayload::ModelStarted { turn: 1 })
            .unwrap();
        em2.emit(AgentEventPayload::ModelStarted { turn: 2 })
            .unwrap();
        assert_eq!(rec.events.lock().unwrap()[1].seq, 1);
    }

    #[test]
    fn payload_serializes_as_snake_case_tagged_json() {
        // 前端契约钉死：payload.type = snake_case，payload 字段与信封 = camelCase
        let e = AgentEvent {
            event_id: "r:0".into(),
            thread_id: "spike".into(),
            run_id: "r".into(),
            seq: 0,
            timestamp: 1,
            schema_version: AGENT_EVENT_SCHEMA_VERSION,
            payload: AgentEventPayload::ToolCompleted {
                call_id: "call-1".into(),
                name: "calculator".into(),
                ok: false,
                error: Some("参数无效".into()),
                preresolved: false,
                structured: serde_json::json!({"result": null}),
                ui_artifact: None,
                truncated: false,
                provenance: Vec::new(),
            },
        };
        let json = serde_json::to_value(&e).expect("serialize");
        assert_eq!(json["payload"]["type"], "tool_completed");
        assert_eq!(json["payload"]["callId"], "call-1");
        assert_eq!(json["payload"]["preresolved"], false);
        assert_eq!(json["eventId"], "r:0");
        assert_eq!(json["schemaVersion"], AGENT_EVENT_SCHEMA_VERSION);
        // snake_case 字段名不得泄漏进 JSON（防惯例回退）
        assert!(json["payload"].get("call_id").is_none());

        // 多字段变体同样 camelCase（user_message/max_turns/final_answer/model_calls）
        let started = serde_json::to_value(AgentEventPayload::RunStarted {
            user_message: "查天气".into(),
            max_turns: 6,
            context: None,
            skill: None,
        })
        .expect("serialize");
        assert_eq!(started["type"], "run_started");
        assert_eq!(started["userMessage"], "查天气");
        assert_eq!(started["maxTurns"], 6);
        // None 选区/技能不序列化（skip_serializing_if），旧消费端零感知
        assert!(started.get("context").is_none());
        assert!(started.get("skill").is_none());
        let completed = serde_json::to_value(AgentEventPayload::RunCompleted {
            outcome: "completed".into(),
            final_answer: "答".into(),
            model_calls: 3,
        })
        .expect("serialize");
        assert_eq!(completed["finalAnswer"], "答");
        assert_eq!(completed["modelCalls"], 3);
    }

    /// AG-21：ToolCompleted 新字段的序列化契约 + 旧事件反序列化兼容。
    /// 事件一经落库永不迁移——AG-21 前的 JSON 没有 structured/uiArtifact/
    /// truncated/provenance，重放链路必须能原样解析（serde(default) 兜底）。
    #[test]
    fn tool_completed_ag21_fields_camel_case_and_backward_compatible() {
        let payload = AgentEventPayload::ToolCompleted {
            call_id: "call-9".into(),
            name: "get_weather".into(),
            ok: true,
            error: None,
            preresolved: false,
            structured: serde_json::json!({"city": "杭州"}),
            ui_artifact: Some(
                crate::tools::UiArtifact::new(
                    "key-value",
                    serde_json::json!({"rows": [["city", "杭州"]]}),
                    "杭州当前天气…",
                    vec![crate::tools::ProvenanceRef::new("tool").with_id("get_weather")],
                )
                .expect("allowlist kind"),
            ),
            truncated: true,
            provenance: vec![crate::tools::ProvenanceRef::new("tool").with_id("get_weather")],
        };
        let json = serde_json::to_value(&payload).expect("serialize");
        assert_eq!(json["structured"]["city"], "杭州");
        assert_eq!(json["uiArtifact"]["kind"], "key-value");
        assert_eq!(json["uiArtifact"]["fallbackMarkdown"], "杭州当前天气…");
        assert_eq!(json["truncated"], true);
        assert_eq!(json["provenance"][0]["source"], "tool");
        assert!(json.get("ui_artifact").is_none());
        assert!(json["uiArtifact"].get("fallback_markdown").is_none());

        // 旧事件（无新字段）反序列化：default 兜底，不得报错
        let legacy = r#"{
            "type": "tool_completed",
            "callId": "call-old",
            "name": "calculator",
            "ok": true,
            "error": null,
            "preresolved": false
        }"#;
        let parsed: AgentEventPayload = serde_json::from_str(legacy).expect("旧事件必须可解析");
        match parsed {
            AgentEventPayload::ToolCompleted {
                structured,
                ui_artifact,
                truncated,
                provenance,
                ..
            } => {
                assert_eq!(structured, serde_json::Value::Null);
                assert!(ui_artifact.is_none());
                assert!(!truncated);
                assert!(provenance.is_empty());
            }
            other => panic!("应为 ToolCompleted，得到 {:?}", other),
        }
    }

    /// AG-26：run_started 选区上下文为增量字段——camelCase 序列化 +
    /// 旧事件（无 context 键）反序列化兼容，schemaVersion 保持 1。
    #[test]
    fn run_started_context_field_camel_case_and_backward_compatible() {
        let payload = AgentEventPayload::RunStarted {
            user_message: "压缩成三句话并保留数字".into(),
            max_turns: 6,
            context: Some(RunContext {
                article_id: "doc-1".into(),
                title: "测试笔记".into(),
                base_version: 3,
                selected_markdown: "这是被选中的段落。".into(),
                selected_text_hash: "hash-abc".into(),
                before_context: "前文…".into(),
                after_context: "后文…".into(),
            }),
            skill: None,
        };
        let json = serde_json::to_value(&payload).expect("serialize");
        assert_eq!(json["context"]["articleId"], "doc-1");
        assert_eq!(json["context"]["baseVersion"], 3);
        assert_eq!(json["context"]["selectedMarkdown"], "这是被选中的段落。");
        assert_eq!(json["context"]["selectedTextHash"], "hash-abc");
        assert_eq!(json["context"]["beforeContext"], "前文…");
        assert_eq!(json["context"]["afterContext"], "后文…");
        // snake_case 字段名不得泄漏
        assert!(json["context"].get("article_id").is_none());
        assert!(json["context"].get("base_version").is_none());

        // 旧事件（AG-26 前落库，无 context 键）反序列化：default = None
        let legacy = r#"{
            "type": "run_started",
            "userMessage": "旧事件",
            "maxTurns": 4
        }"#;
        let parsed: AgentEventPayload = serde_json::from_str(legacy).expect("旧事件必须可解析");
        match parsed {
            AgentEventPayload::RunStarted { context, .. } => assert!(context.is_none()),
            other => panic!("应为 RunStarted，得到 {:?}", other),
        }
    }

    /// AG-27：run_started 激活技能为增量字段——camelCase 序列化（name/version/
    /// source）+ 旧事件（无 skill 键）反序列化兼容，schemaVersion 保持 1。
    /// 验收「Run 可见版本与来源」的事件侧契约。
    #[test]
    fn run_started_skill_field_camel_case_and_backward_compatible() {
        let payload = AgentEventPayload::RunStarted {
            user_message: "整理一份研究笔记".into(),
            max_turns: 5,
            context: None,
            skill: Some(RunSkillRef {
                name: "research-note".into(),
                version: 1,
                source: "bundled".into(),
            }),
        };
        let json = serde_json::to_value(&payload).expect("serialize");
        assert_eq!(json["skill"]["name"], "research-note");
        assert_eq!(json["skill"]["version"], 1);
        assert_eq!(json["skill"]["source"], "bundled");
        assert!(json.get("skill").is_some());

        // 旧事件（AG-27 前落库，无 skill 键）反序列化：default = None
        let legacy = r#"{
            "type": "run_started",
            "userMessage": "旧事件",
            "maxTurns": 4,
            "context": null
        }"#;
        let parsed: AgentEventPayload = serde_json::from_str(legacy).expect("旧事件必须可解析");
        match parsed {
            AgentEventPayload::RunStarted { skill, context, .. } => {
                assert!(skill.is_none());
                assert!(context.is_none());
            }
            other => panic!("应为 RunStarted，得到 {:?}", other),
        }
    }
}
