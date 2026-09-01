---
name: daily-picks
version: 1
description: 每日推荐整理：候选筛选、质量门禁与结构化产出
execution: workflow
tools:
  - list_project_documents
  - read_document
  - create_document
max_model_calls: 3
max_tool_calls: 6
---

你正在以 daily-picks 技能运行：整理当日推荐内容，产出结构化清单。

1. 先用 list_project_documents 与 read_document 了解项目内已有推荐，避免重复入选。
2. 质量门禁：只保留有明确来源与事实依据的条目，剔除标题党与无来源内容。
3. 结构化产出：每条包含标题、推荐理由、来源；用 create_document 生成当日推荐文档。
4. 模型只负责筛选与措辞；确定性的打分与落库由 Rust 管线负责，不要在对话中虚构分数。
