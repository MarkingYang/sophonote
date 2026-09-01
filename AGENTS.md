# AGENTS.md

SophoNote 是 macOS 本地优先 AI 知识工作台。产品界面使用中文。社区入口文档成对维护：英文无后缀（`README.md`），中文为 `README.zh-CN.md`。PRD / 架构 / 台账正文以中文为真相源。

## 唯一工作目录

所有 SophoNote 代码、文档和脚本只允许写入**本仓库根目录**。不要在仓库外、父级或其它子目录创建 SophoNote 代码副本、设计稿、审计、TODO 或过程文章。

产品界面使用中文。不要把 PRD / 架构 / 台账改写成英文全译本。

## 必读真相源

开始工作前完整阅读：

1. `CLAUDE.md`：工程执行、验证红线和常见故障。
2. `docs/PRD.md`：唯一产品需求真相源。
3. `docs/architecture.md`：唯一技术架构真相源。
4. `docs/project-ledger.md`：唯一进展、问题与后续跟进台账。

不创建第二份 TODO、PROGRESS、项目状态、日期审计或专项长期设计；当前状态更新统一台账。历史草稿不在本仓库，不要重建 `docs/history/`、`docs/current/` 或 `docs/guides/`。现行文档平铺在 `docs/`。社区入口成对维护英文无后缀与 `.zh-CN.md`。

## 命令速查

```bash
./scripts/sophonote.sh {start|stop|restart|status|logs}
pnpm exec tsc --noEmit
pnpm test --run
pnpm build
pnpm pack:macos
pnpm pack:windows

cd src-tauri
export PATH="$HOME/.cargo/bin:$PATH"
cargo check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

使用 pnpm。禁止前台运行常驻 tauri dev；不运行 `cargo clean`，不删除 `src-tauri/target` 或增量缓存。

## 核心边界

- Rust 拥有文件、SQLite、第三方 HTTP、模型、embedding 和 MCP 子进程副作用；前端只渲染与 invoke。
- `.md` 是正文真相源；SQLite 是元数据、索引、运行和审计存储。
- `appStore`、`projectStore`、`agentStore`、`changeSessionStore` 不合并。
- Completion 不建 Agent Run；Agent/Skill/MCP 不能绕过 DocumentService 写正文。
- 文档 Patch 必须绑定 baseVersion/TextAnchor/hunks，冲突停止，不猜测覆盖。
- 不做飞书式逐次 autosave 历史；内部 version/checkpoint 仅用于并发与事故恢复，知识语义 Git 版本与恢复路径遵守 PRD/architecture 的 VersionService/DocumentService 边界。

## 验证

- 仅前端：tsc + 相关 Vitest，不重启。
- 仅 Rust：cargo check + 相关测试；观察运行时才重启。
- Tauri 契约变化：双端检查、契约测试并重启。
- 页面、编辑器、焦点、滚动、快捷键和性能必须在真实 Tauri 宿主验收。
- 需求/架构变化先改对应真相源，再写代码。
- 每个事项状态变化同步更新 `docs/project-ledger.md` 原记录，不新建平行台账。
