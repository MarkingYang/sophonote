// ============================================================
// Track B · 智能体演进 —— Agent Runtime 模型层（docs/architecture.md）
// Phase 0：统一 ModelGateway（messages / gateway / openai_compat / prompt_registry / commands）
// 后续 Phase 按设计文档在此扩展（不预建）。
// ============================================================
pub mod commands;
pub mod gateway;
pub mod messages;
pub mod openai_compat;
pub mod prompt_registry;
