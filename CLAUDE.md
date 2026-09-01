# SophoNote 协作者指南

本文只记录工程执行约束，不承担产品需求、架构状态或进度管理。产品与技术事实只维护在：

- `docs/PRD.md`：产品范围、优先级、指标与验收。
- `docs/architecture.md`：组件、协议、数据、安全、性能、部署与已知限制。
- `docs/project-ledger.md`：进展、问题、后续事项、决策与验证证据。

## 唯一工作目录

SophoNote 的唯一项目根目录就是**本仓库根目录**。

所有项目代码、文档和脚本只能写入该目录。不要在仓库的父级、同级或其它子目录创建 SophoNote 代码副本、设计稿、审计、TODO 或过程文章。

产品界面使用中文。社区入口文档成对维护：英文无后缀（`README.md`），中文为 `.zh-CN.md`。PRD / 架构 / 台账正文以中文为真相源，不要另写英文全译本。

## 开始工作前

1. 阅读 `docs/PRD.md` 中对应需求、范围和验收标准。
2. 阅读 `docs/architecture.md` 中对应组件边界、协议、数据和已知限制。
3. 阅读 `docs/project-ledger.md` 中对应进展、问题和下一门禁。
4. 需求或架构变化先修改对应真相源，再修改代码；状态变化同步更新台账原记录。
5. 不新建第二份专项 PRD、日期审计、TODO、PROGRESS 或项目状态。历史草稿不在本仓库，不要重建 `docs/history/`、`docs/current/` 或 `docs/guides/`。现行文档平铺在 `docs/`。

## 常用命令

```bash
# 应用生命周期（唯一入口，勿前台运行 tauri dev）
./scripts/sophonote.sh start
./scripts/sophonote.sh restart
./scripts/sophonote.sh status
./scripts/sophonote.sh logs
./scripts/sophonote.sh stop

# 前端
pnpm exec tsc --noEmit
pnpm test --run
pnpm build

# 打包（必须在对应宿主；无证书产物不是 RC）
pnpm pack:macos      # Apple Silicon 或当前 Darwin 架构
pnpm pack:windows    # 仅 Windows x64
pnpm release:macos   # Developer ID + 公证
pnpm release:windows # Authenticode

# Rust（在 src-tauri/ 下）
export PATH="$HOME/.cargo/bin:$PATH"
cargo check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

- 应用标识：`com.fei.sophonote`；Vite 端口：1420。
- 数据库：macOS `~/Library/Application Support/com.fei.sophonote/sophonote.db`；Windows `%APPDATA%\com.fei.sophonote\sophonote.db`。
- 包管理器使用 pnpm，不使用 npm 安装依赖。

## 执行与验证红线

- 单次验证目标 ≤60 秒；预估超时先缩小范围或改用增量检查。
- 禁止前台运行常驻 `pnpm tauri dev`；启停统一走 `scripts/sophonote.sh`。
- 仅前端改动：`tsc` + 相关 Vitest，不重启，依赖 Vite HMR。
- 仅 Rust 内部逻辑：先 `cargo check`；仅需观察运行行为时 restart。
- Tauri 命令或 invoke 契约变化：Rust check + tsc + 契约测试 + restart。
- 不删除 `node_modules/.tmp` 的 tsbuildinfo，不运行 `cargo clean`，不清理 `src-tauri/target`。
- 阶段完成前运行相关测试、生产构建、Clippy 和 `git diff --check`。
- 页面、编辑器、焦点、滚动、快捷键和性能必须真实 Tauri/WKWebView 走查；编译/jsdom 不能替代宿主验收。
- 结束调试时不遗留由本轮启动的后台进程。

## 不可破坏的架构边界

- Rust 拥有文件、SQLite、第三方 HTTP、模型、embedding 和子进程副作用；React 只渲染与 invoke。
- WKWebView 不直连第三方 API；模型调用统一走 Rust `ModelGateway`。
- `.md` 文件是文档正文真相源；SQLite 保存元数据、索引、运行和操作审计。
- `appStore`、`projectStore`、`agentStore`、`changeSessionStore` 按生命周期隔离，不合并状态。
- Inline Completion 不建 Thread/Run；ghost text 不进 Markdown/history，Tab 接受后才成为普通编辑。
- Agent 只能提出带 baseVersion、TextAnchor 和 hunks 的 Patch；落盘唯一入口是用户批准后的 DocumentService。
- Skill 不直接授予权限；MCP 默认拒绝，模型不能安装、启动或授权 MCP。
- 不做飞书式逐次 autosave 历史；内部 version/operation/checkpoint 只服务并发与事故恢复，语义 Git 版本只能由 VersionService 异步建立，恢复仍经 DocumentService Patch。

## 性能整改约束

- 当前 P0 是页签、文档、输入和预览性能，门禁见 PRD §3/§15、架构 §11/§20。
- 不得把当前 `await flush()` 直接改成无约束 fire-and-forget；先建立按 documentId 隔离的 DraftRecord/保存队列。
- 不得仅用 `display:none` 保活编辑器；必须同时解决外部正文同步、history、焦点、快捷键、滚动和可访问性。
- 不得为提速绕过 Patch 的 version、TextAnchor、审批、CAS 和保存失败门禁。
- 静态代码只能确认候选热路径，完成结论必须有同夹具改前/改后数据。
- Vite manualChunks 对 preload helper 边界敏感；修改构建配置必须复核启动依赖图和 chunk 形态。

## 测试约定

- 前端测试放 `src/**/__tests__/**/*.test.ts`，需要时在文件名保留稳定能力 ID。
- Rust 使用模块内单元/集成测试，运行 `cargo test --all-targets`。
- `rig-core`/`rig-agent = 0.41.0`、`rmcp = 3.1.2`；升级前必须跑完整 Agent Runtime、Skill、MCP 和文档 Patch 回归。
- 文档改动检查相对链接、Mermaid fence、旧文件引用和 `git diff --check`。

## 常见故障经验

- SQLite 可空字段运行时可能是 `null`；前端守卫使用 `!= null`，不要只判断 `undefined`。
- 页面改动必须实际打开；`main.tsx` 的错误浮层用于白屏取证。
- sqlite-vec 的 MATCH 查询显式提供 `k`；项目采用先 vec 后回表，避免 JOIN 约束问题。
- 批量外部 API 调用必须节流，错误保留 HTTP 状态码；GitHub 匿名调用有短窗口二级限流。
- HuggingFace 在部分网络环境依赖镜像，失败应降级而非阻塞写作主链路。
- Rust 尾表达式涉及 statement/rows 生命周期时，先绑定局部变量再返回，避免 E0597。
- dev 模式改 Rust 会自动重编译并中断长任务；索引等任务必须可增量恢复。
- 文件与 SQLite 不共享事务；文档写入坚持唯一 tmp、原子 rename、operation 补偿和启动恢复。
- `.md` 是 Article 正文真相源，`articles.content` 可按兼容策略为空；发现/报告的“正文存在”门禁必须以成功写盘后建立的 Article 索引或 repository 读取结果判断，禁止直接用 `trim(articles.content) != ''` 过滤。
- OpenRouter 模型榜第三方 HTTP 只允许 Rust 调用；API Key 使用 Host provider `openrouter-rankings`，Release 必须写入 Keychain，不得写入源码、文档、日志或回传 WebView。
- 开发期 Keychain 因未签名二进制可能反复授权；当前 `settings.apikey:*` 明文回退（含 OpenRouter）只允许 Debug 编译使用，发布前必须迁移回 Keychain。
