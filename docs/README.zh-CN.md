# SophoNote 文档

[English](./README.md) | **简体中文**

社区入口：[README.zh-CN.md](../README.zh-CN.md)。这里是文档地图。

产品界面为中文。社区入口文档成对维护：英文无后缀，中文为 `.zh-CN.md`。PRD / 架构 / 台账 / widget spec 正文以中文为真相源，不另维护英文全译本。

现行文件平铺在 `docs/`，不再使用 `current/` 或 `guides/` 子目录。历史草稿不在本仓库；现行结论以本页所列文件为准。

## 用户

| 文档 | 职责 |
|---|---|
| [使用手册](../skills/hermes/productivity/sophonote-help/references/user-manual.md) | 六个入口、会话权限、工作室、笔记本、浏览器 |
| [Markdown 写作与持久化](./sophonote-markdown-writing.zh-CN.md) | 当前文档/选区、审阅落盘、标题与项目目录 |

数据目录、密钥与 Sidecar 边界见根 [README.zh-CN.md](../README.zh-CN.md) 与 [SECURITY.zh-CN.md](../SECURITY.zh-CN.md)。

## 贡献者

| 文档 | 职责 |
|---|---|
| [贡献指南](../CONTRIBUTING.zh-CN.md) | 环境、PR、Hermes sidecar、附着、打包、开源卫生 |
| [许可证](../LICENSE) / [第三方声明](../NOTICE) | MIT；Hermes 与 CPython 再分发义务 |
| [安全披露](../SECURITY.zh-CN.md) | 漏洞报告；不要在公开 Issue 贴密钥或笔记 |
| [Hermes 执行平面图](./hermes-runtime-architecture.html) | DEC-011 交互式架构图（源：同名 `.archify.json`） |
| [当前实装架构图](./sophonote-current-architecture.html) | 产品壳 + Hermes 执行平面（源：同名 `.archify.json`） |

## 产品与架构真相源

三份文档分工不同，不互相复制。不要另建 TODO、PROGRESS、日期审计或专项长期 PRD。

| 文档 | 唯一职责 | 维护时机 |
|---|---|---|
| [产品需求文档（PRD）](./PRD.md) | 为什么、做什么、验收 | 产品判断变化时 |
| [技术架构设计](./architecture.md) | 现在如何实现、准备如何改 | 技术事实或设计变化时 |
| [项目台账](./project-ledger.md) | 进展、问题、决策、证据 | 每个事项状态变化时 |
| [桌面组件库规格](./widget-component-library.spec.md) | WidgetKit / 控制中心投影（macOS） | DEC-042 设计变化时 |

状态口径（PRD / 架构通用）：`已实现` 只表示代码存在；`自动验证` 表示测试通过；`已验收` 必须有真实 macOS/Tauri 宿主走查证据。不要把目标写成已完成。

Hermes 的开发附着、故障定位、打包、签名公证和干净机器验收步骤在[架构 §17、§20、§21](./architecture.md) 与 [CONTRIBUTING.zh-CN.md](../CONTRIBUTING.zh-CN.md)。Agent 工作约定见 [CLAUDE.md](../CLAUDE.md) 与 [AGENTS.md](../AGENTS.md)。
