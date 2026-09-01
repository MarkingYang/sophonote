//! Track B · AG-24（Phase 3 DocumentService）：Agent-safe 文档域。
//!
//! 实施基线：docs/architecture.md。落地顺序：抽 Repository →
//! version/revision/operation/幂等 → diff-only → 审批/冲突/保存/undo → 工具注册）。
//!
//! 分层：
//! - repository.rs = 唯一底层文档读写层（从 notes/commands 基座抽出，
//!   既有 Tauri 命令签名不变；编辑器直写与 Agent 写入同源经过，
//!   「全应用单一文档写入路径」）；
//! - service.rs = DocumentService：version CAS / revision 快照 / 操作日志幂等 /
//!   dry-run diff / 审批闸门 / undo / 启动恢复；
//! - anchor.rs = TextAnchor 解析（AG-25：选中文本 hash + 前后文 + 唯一匹配，
//!   零/多候选 = 冲突不猜测）与 Markdown 结构校验（round-trip 可替换子集）；
//! - commands.rs = 用户侧命令（预览/批准/拒绝/撤销；AG-26 Worklog UI 的接线点）。
//!
//! 铁律：Agent 不直碰 notes.rs/SQLite/文件——模型只有「提议修改」工具
//! （propose_document_patch，恒 dry-run），落盘必须经用户批准 + 版本复检。
pub mod anchor;
pub mod commands;
pub mod repository;
pub mod service;
