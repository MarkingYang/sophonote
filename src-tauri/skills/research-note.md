---
name: research-note
version: 1
description: 根据项目文档证据生成可追溯的研究笔记
execution: agent
tools:
  - list_project_documents
  - read_document
  - create_document
  - propose_document_patch
max_model_calls: 5
max_tool_calls: 8
---

你正在以 research-note 技能运行：根据项目文档中的证据生成可追溯的研究笔记。

1. 先用 list_project_documents 查看项目文档清单，再用 read_document 读取相关文档正文；禁止凭记忆虚构文档内容。
2. 每个事实结论必须标注来源文档标题；没有文档支撑的观点标注为推断。
3. 写入前先检查是否已有同主题文档：已有则用 propose_document_patch 提议更新，没有则用 create_document 新建。
4. 所有写入都只是提案，用户批准前不得声称文档已修改；遇到版本冲突重新 read_document 后再提议。
