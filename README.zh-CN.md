# SophoNote

[English](./README.md) | **简体中文**

SophoNote 是围绕开源 [Hermes Agent](https://github.com/NousResearch/hermes-agent) 构建的 macOS 本地优先 AI 知识工作台。笔记、发现、权限和文档审阅留在 SophoNote，Hermes 专注 Agent 运行时。它不是 Hermes Desktop 的分叉，也不复刻其界面。

SophoNote 是钉扎 Hermes Sidecar 外围的 **Client Surface**：

- Hermes 负责执行、模型调用、Skill、MCP 连接与长期记忆。
- SophoNote 拥有笔记、发现、权限、审批、文件和审计。
- 文档修改必须经过范围绑定、diff、用户决策和冲突检查。

本仓库以 [MIT](./LICENSE) 许可。第三方声明见 [NOTICE](./NOTICE)。不复制 Hermes Desktop 的 Bot Mode、HUD、语音、Memory Graph、worktree 或消息通道。

产品界面为中文。社区入口文档成对维护：英文无后缀（`README.md`），中文为 `README.zh-CN.md`。PRD / 架构 / 台账正文以中文为真相源。

## 项目状态

仓库以 MIT 许可证在 GitHub 维护。当前代码可用于本地开发和 unsigned 打包，但现有产物仍是社区预览，不等同于已公证的正式 RC。macOS 是完整体验与宿主验收平台；Windows x64 已有 CI 打包流程，仍待 Windows 宿主验收与 Authenticode 签名。

## 能做什么

一级导航是发现、会话、工作室、笔记本、计划任务、工具。收件箱留在设置里，不升为一级入口。

| 入口 | 作用 |
|---|---|
| 发现 | 看经过筛选和解读的 AI 动态、日报与模型榜 |
| 会话 | 以任务为单位使用 Hermes；可临时绑定本地目录 |
| 工作室 | 面向真实本地项目的 IDE 工作面：文件、搜索、变更、终端、浏览器与右侧 Agent |
| 笔记本 | 编辑 Markdown；进入页面不会创建文档，只有显式“新建笔记”、模板或“导入功能范例”才创建 |
| 计划任务 | 管理本机保留的 Hermes 定时任务与运行历史；可从脱敏范例开始，范例和未配置模型的任务默认暂停 |
| 工具 | 今日事项、待办与番茄钟 |

操作说明：[使用手册](./skills/hermes/productivity/sophonote-help/references/user-manual.md)。写作与落盘：[Markdown 写作指南](./docs/sophonote-markdown-writing.zh-CN.md)。文档地图：[docs/README.zh-CN.md](./docs/README.zh-CN.md)。

## 内置范例

- 笔记本空态或“新建笔记”右侧菜单可显式导入 6 篇功能范例，覆盖 Markdown、大纲、任务、双链/反链/嵌入、搜索与模板。进入笔记本不会自动导入；重复导入只补缺失项，不覆盖同名笔记。范例正文也保存在 [`scripts/walkthrough-samples/`](./scripts/walkthrough-samples/)，方便社区查看和验收。
- 计划任务页的“使用范例”提供 5 个由旧版任务意图重新整理的公开示例。它们不含私有任务 ID、历史、模型、时间戳或输出；保存后始终暂停。选择可用模型后可点“立即运行”单次执行，只有显式启用后才进入周期调度。可审阅的清单见 [`examples/scheduled-tasks.json`](./examples/scheduled-tasks.json)。

## 硬边界

- Rust 拥有文件、SQLite、第三方 HTTP、模型、embedding 和子进程；前端只渲染与 invoke。
- `.md` 是笔记正文真相源；SQLite 只存元数据、索引、运行和审计。
- Agent 只能提出带版本与锚点的 Patch，用户批准后由 DocumentService 落盘。
- Skill 不直接授予权限；MCP 默认拒绝；模型不能安装或授权 MCP。
- API Key 不进源码、文档、日志或 WebView。Release 只走 macOS Keychain。
- Git 只承载源码、脱敏范例与示例配置；本地数据库、会话、工作室状态、笔记、日志、Hermes Home 和构建 Runtime 均不得入库。

架构图：[Hermes 执行平面](./docs/hermes-runtime-architecture.html)。完整技术事实见[架构设计](./docs/architecture.md)。

## 数据存在哪

应用标识：`com.fei.sophonote`。用户数据在本机：

- macOS：`~/Library/Application Support/com.fei.sophonote/`

其中 `sophonote.db` 是元数据与索引；笔记正文、会话、工作室状态、计划任务定义/历史和私有 Hermes Home 都在该应用数据根内。需要时，旧应用标识下的数据只在本机完成迁移，不会进入 Git 仓库。不要把该目录、`.env.hermes.local` 或 Session Token 提交进 git，也不要贴到 Issue / PR。

## 快速开始

需要：Node.js、[pnpm](https://pnpm.io/)、Rust（`cargo` 在 `PATH` 中）。在 macOS 上构建。包管理器只用 pnpm。

```bash
pnpm install
./scripts/sophonote.sh start
```

启停只用 `scripts/sophonote.sh`，不要前台常驻 `pnpm tauri dev`。仅前端改动依赖 Vite HMR；改了 Rust 或 Tauri 契约再用 `./scripts/sophonote.sh restart`。

Hermes 以钉扎 Sidecar（Hermes 0.20.0 + CPython 3.11）随应用分发。Release 只启动包内 Runtime。二进制不入库：贡献者如何构建 sidecar、附着外部 Gateway、打包和公证，见 [CONTRIBUTING.zh-CN.md](./CONTRIBUTING.zh-CN.md)。

## 贡献与安全

欢迎 Issue 与 PR。改代码前请读 [CONTRIBUTING.zh-CN.md](./CONTRIBUTING.zh-CN.md)：构建、测试、开源卫生扫描、打包矩阵。提交前运行：

```bash
./scripts/oss-hygiene.sh
```

安全问题不要发到公开讨论区，见 [SECURITY.zh-CN.md](./SECURITY.zh-CN.md)。

产品判断改 [PRD](./docs/PRD.md)，技术事实改 [架构](./docs/architecture.md)，事项状态改 [台账](./docs/project-ledger.md)。不要另建 TODO、逐轮进展或专项长期 PRD。历史草稿不在本仓库。
