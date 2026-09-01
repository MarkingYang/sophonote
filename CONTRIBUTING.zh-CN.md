# 贡献指南

[English](./CONTRIBUTING.md) | **简体中文**

SophoNote 是 macOS 本地优先的 Hermes Client Surface。产品界面为中文。社区入口文档成对维护：英文无后缀，中文为 `.zh-CN.md`。产品介绍与快速开始见 [README.zh-CN.md](./README.zh-CN.md)。

欢迎 Issue 和 Pull Request。请先在现有 Issue 里搜索，避免重复。安全漏洞走 [SECURITY.zh-CN.md](./SECURITY.zh-CN.md)，不要开公开 Issue。

## 环境

- Node.js 与 [pnpm](https://pnpm.io/)（不要用 npm 装依赖）
- Rust：`cargo` 在 `PATH` 中（可使用 `$HOME/.cargo/bin`）
- 验证基线：Hermes Agent 0.20.0（钉扎 Sidecar，见下文）
- Apple 目标只能在 macOS 上构建；Windows NSIS 只能在 Windows x64 上构建

```bash
node --version
pnpm --version
rustc --version
```

## 仓库边界

- 应用源码与文档只在本仓库根目录维护，不要在仓库外再建一份 SophoNote 文档树。
- 产品需求、架构、进展只改 `docs/PRD.md`、`docs/architecture.md`、`docs/project-ledger.md`。文档地图见 [docs/README.zh-CN.md](./docs/README.zh-CN.md)。不要另建 TODO / PROGRESS / 日期审计或专项长期 PRD。
- 现行文档平铺在 `docs/`。历史草稿不在本仓库，不要重建 `docs/history/`、`docs/current/` 或 `docs/guides/`。
- 钉扎 Hermes 二进制不入库。先克隆 [Hermes Agent](https://github.com/NousResearch/hermes-agent) 到钉扎 commit，再 `export HERMES_SOURCE_DIR=...` 后执行 `pnpm hermes:bundle`。

## 开发

```bash
pnpm install
./scripts/sophonote.sh start
pnpm exec tsc --noEmit
pnpm test --run
```

启停只用 `scripts/sophonote.sh`，不要前台常驻 `pnpm tauri dev`。包管理器使用 pnpm。仅前端改动依赖 Vite HMR；改了 Rust 或 Tauri 契约再用 `./scripts/sophonote.sh restart`。

```bash
./scripts/sophonote.sh status
./scripts/sophonote.sh logs
./scripts/sophonote.sh stop
```

Rust：

```bash
cd src-tauri
export PATH="$HOME/.cargo/bin:$PATH"
cargo check
cargo test --all-targets
```

Hermes 真链路成功时，`logs/dev.log` 应出现 `resolve=Use(Hermes)` 和 `surface=gateway`。验收必须覆盖 Session 创建/恢复、Skill、附件、审批/澄清、取消和最终答案，不能只测端口可连接。

## 官网

对外官网源码固定放在 `website/`，与客户端在同一仓库维护。它是无依赖静态站，从仓库根目录预览：

```bash
pnpm website:dev
```

宣传事实必须与 PRD、架构和台账一致。`main` 上涉及 `website/` 的提交通过 GitHub Pages 部署；`website/CNAME` 声明 `sophonote.com`，下载入口从本仓库 GitHub Releases 动态解析最新公开 DMG，不把安装包复制进网站目录。

## 提 PR

1. 从当前 `main` 开分支，范围尽量小。
2. 需求或架构变化先改 PRD / 架构，再改代码；事项状态更新台账原记录。
3. 仅前端：`pnpm exec tsc --noEmit` 与相关 `pnpm test --run`。仅 Rust：`cargo check` 与相关测试。Tauri 契约变化：双端检查后再 `./scripts/sophonote.sh restart`。
4. 提交前运行 `./scripts/oss-hygiene.sh` 与 `git diff --check`。
5. PR 说明写清为什么改、如何验证；不要贴密钥、本机绝对路径或用户笔记。
6. 产品界面文案使用中文。社区入口文档成对维护英文 + `.zh-CN.md`。PRD / 架构 / 台账正文以中文为准。

## 架构红线

- Rust 拥有文件、SQLite、第三方 HTTP、模型、embedding 和子进程；前端只渲染与 invoke。
- `.md` 是笔记正文真相源；Agent 只能提出 Patch，用户批准后由 DocumentService 落盘。
- Skill 不直接授予权限；MCP 默认拒绝；模型不能安装或授权 MCP。
- API Key 不得写入源码、文档、日志或回传 WebView。Release 只走 Keychain。

## Hermes sidecar

Hermes 0.20.0 + CPython 3.11 随 macOS `.app` 与 Windows NSIS 分发。Release 只启动包内 Runtime，不读取机器 `PATH`、`~/.hermes`、源码 checkout 或 `SOPHONOTE_HERMES_*`。Debug 仍可显式附着外部 Gateway。

钉扎二进制不入库，贡献者在本机构建：

```bash
git clone https://github.com/NousResearch/hermes-agent
cd hermes-agent
git checkout 07da945c214481083049500bd29f45cabc5a04b2
export HERMES_SOURCE_DIR="$PWD"
# 回到 SophoNote 仓库
pnpm hermes:bundle
```

`HERMES_SOURCE_DIR` 必须指向该 commit，且 `uv.lock` / 源码树哈希需与 `scripts/build-hermes-sidecar.sh` 中的钉扎值一致。

## 附着外部 Hermes（可选，仅 Debug）

直接 `./scripts/sophonote.sh start` 时，未配置正式 Gateway 会启动包内钉扎 Sidecar。只有需要复用已有 Gateway 或非默认端口时才创建本地文件：

```bash
cp .env.hermes.example .env.hermes.local
```

使 `SOPHONOTE_HERMES_GATEWAY_TOKEN` 与 Hermes 的 `HERMES_DASHBOARD_SESSION_TOKEN` 完全一致：

```dotenv
SOPHONOTE_HERMES_ATTACH_EXTERNAL=1
SOPHONOTE_HERMES_GATEWAY_URL=ws://127.0.0.1:9119/api/ws
SOPHONOTE_HERMES_GATEWAY_TOKEN=<同一个随机 token>
SOPHONOTE_HERMES_HOME=
```

`SOPHONOTE_HERMES_HOME` 必须写展开后的绝对路径。Hermes Gateway 只绑定环回地址。Token 可用 `openssl rand -hex 32` 生成，放入 Hermes 自己的私有环境文件，不要提交到本仓库。

```bash
HERMES_DASHBOARD_SESSION_TOKEN=<同一个随机 token> hermes serve --host 127.0.0.1 --port 9119 --skip-build
```

未设置 `SOPHONOTE_HERMES_ATTACH_EXTERNAL=1` 时，Debug 也只启动包内 Sidecar，避免污染 Hermes Desktop 的独立 Session/Memory/附件。`*.local` 已被 Git 忽略。

当前开发附着包从 Finder 启动时不会加载 `.env.hermes.local`。需要验证外置 Hermes 时，从项目根执行：

```bash
set -a
. ./.env.hermes.local
set +a
./src-tauri/target/debug/bundle.noindex/macos/SophoNote.app/Contents/MacOS/sophonote
```

## 开源卫生

提交前在仓库根目录运行：

```bash
./scripts/oss-hygiene.sh
```

门禁：

1. 没有密钥、token、本机绝对路径作为默认配置。
2. 没有把 `logs/`、`.env*`（示例除外）、数据库、会话、工作室状态、用户 `notes/`、Hermes Home 或生成的 Runtime 包加入版本库。
3. 新增第三方代码保留其许可证，并在需要时更新 `NOTICE`。
4. Git 作者身份使用仓库级、保护隐私的配置（例如 GitHub noreply 地址）；没有维护者明确决策时，不改写已发布历史或 force-push。

安全问题见 [SECURITY.zh-CN.md](./SECURITY.zh-CN.md)。

## 打包

打包必须在对应宿主上进行，Hermes CPython 不能交叉编译。无证书产物不是 RC。

| 命令 | 宿主 | 产物 | 是否 RC |
|---|---|---|---|
| `pnpm pack:macos` | Apple Silicon macOS（Intel Mac 会打出 x86_64 第二架构） | `.app` + DMG | 否 |
| `pnpm pack:windows` | Windows x64（Git Bash / CI bash） | NSIS `setup.exe` | 否 |
| `pnpm release:macos` | 具备 Developer ID 的 macOS 发布机 | 公证 DMG | 是 |
| `pnpm release:windows` | 具备 Authenticode 的 Windows 发布机 | 签名 NSIS | 是 |

```bash
export HERMES_SOURCE_DIR=/path/to/hermes-agent   # 钉扎 commit
pnpm pack:macos     # 仅 macOS
pnpm pack:windows   # 仅 Windows
```

正式 RC：`pnpm release:macos` 或 `pnpm release:windows`。GitHub Actions `pack.yml` 在版本标签上把 unsigned pack 发布为 prerelease，手动运行则只保留 Actions artifact；两者都不会让无签名产物自动成为 RC。

macOS 构建脚本会在 `src-tauri/target/` 写入 Spotlight 排除标记，并在构建完成后将 `.app` 放入 `bundle.noindex/`。安装目标始终是 `/Applications/SophoNote.app`：

```bash
pnpm install:macos -- "$PWD/src-tauri/target/release/bundle.noindex/macos/SophoNote.app"
```

脚本校验应用名与 Bundle ID 后完整替换旧包，不合并旧目录，也不会修改 `~/Library/Application Support/com.fei.sophonote/`。

本地无 Apple 凭据的可复现验证、公证步骤和干净机器矩阵见[架构 §17](./docs/architecture.md#17-部署环境与基础设施)与[架构 §20](./docs/architecture.md#20-测试与验证方案)。
