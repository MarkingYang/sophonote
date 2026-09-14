# SophoNote

**English** | [简体中文](./README.zh-CN.md)

Official website: [sophonote.com](https://sophonote.com) · [GitHub Releases](https://github.com/MarkingYang/sophonote/releases)

SophoNote is a local-first AI knowledge workbench for macOS, using open-source [Hermes Agent](https://github.com/NousResearch/hermes-agent) and [Pi](https://github.com/earendil-works/pi) as selectable execution engines, with optional locally installed Claude Code. Notes, discovery, permissions, and document review stay in SophoNote. This project is neither a Hermes Desktop fork nor a UI clone.

SophoNote is a **client surface** around pinned agent sidecars:

- Hermes owns execution, model calls, Skills, MCP connections, and long-term memory.
- SophoNote owns notes, discovery, permissions, approvals, files, and audit.
- Document edits must go through scoped binding, a diff, a user decision, and conflict checks.

This repository is [MIT](./LICENSE)-licensed. Third-party notices are in [NOTICE](./NOTICE). We do not copy Hermes Desktop surfaces such as Bot Mode, HUD, voice, Memory Graph, worktrees, or message channels.

The product UI is Chinese. Community docs ship as two files: English (`README.md`) and Simplified Chinese (`README.zh-CN.md`). The canonical body of the PRD, architecture, and ledger remains Chinese.

## Project status

The repository is maintained on GitHub under the MIT license. The application is usable for local development and unsigned packaging, but current artifacts are community previews rather than notarized release candidates. macOS is the acceptance platform; Windows x64 packaging exists in CI and still needs Windows host acceptance and Authenticode signing.

## What it does

Primary navigation: Discover, Conversation, Studio, Notes, Scheduled Tasks, Tools. Inbox stays in Settings and is not a top-level entry.

| Surface | Role |
|---|---|
| Discover | Filtered and interpreted AI news, daily reports, and model rankings |
| Conversation | Task-scoped Hermes, Pi, or Claude Code sessions; optional temporary local-directory binding |
| Studio | IDE surface for a real local project: files, search, diffs, terminal, browser, and a right-side Agent |
| Notes | Edit Markdown; opening Notes never creates a document—creation requires an explicit New Note, template, or Import Examples action |
| Scheduled Tasks | Manage locally preserved Hermes cron jobs and run history; sanitized examples and jobs without an explicit model stay paused |
| Tools | Today view, todos, and pomodoro |

How to use: [user manual](./skills/hermes/productivity/sophonote-help/references/user-manual.md) (Chinese, matches the UI). Writing and persist: [Markdown writing guide](./docs/sophonote-markdown-writing.md). Docs map: [docs/README.md](./docs/README.md).

## Included examples

- Notes offers an explicit import of six feature examples covering Markdown, outline navigation, tasks, links/backlinks/embeds, search, and templates. Opening Notes never imports them automatically; repeat imports only add missing titles and do not overwrite same-title notes. Their Markdown sources are reviewable in [`scripts/walkthrough-samples/`](./scripts/walkthrough-samples/).
- Scheduled Tasks offers five public examples rewritten from the intent of legacy local jobs. They contain no private job IDs, run history, model choices, timestamps, or outputs, and are always created paused. Configure a model to run one manually; enable it explicitly to start its schedule. Review the catalog at [`examples/scheduled-tasks.json`](./examples/scheduled-tasks.json).

## Hard boundaries

- Rust owns files, SQLite, third-party HTTP, models, embeddings, and subprocesses. The frontend only renders and invokes.
- `.md` files are the source of truth for note bodies. SQLite stores metadata, indexes, runs, and audit only.
- An Agent may only propose a versioned, anchored Patch. DocumentService writes to disk after the user approves.
- Skills do not grant permissions. MCP is deny-by-default. The model cannot install or authorize MCP.
- API keys do not belong in source, docs, logs, or the WebView. Release builds use macOS Keychain only.
- Git contains source, sanitized examples, and sample configuration only. Local databases, conversations, Studio state, notes, logs, Hermes Home, and bundled runtime output are excluded.

Architecture diagram: [Hermes execution plane](./docs/hermes-runtime-architecture.html). Full technical facts: [architecture](./docs/architecture.md) (Chinese).

## Where data lives

App id: `com.fei.sophonote`. User data stays on this machine:

- macOS: `~/Library/Application Support/com.fei.sophonote/`

`sophonote.db` holds metadata and indexes. Note bodies, conversations, Studio state, scheduled-task definitions/history, and the private Hermes Home are stored under that application-data root. Existing data is migrated locally from the legacy app id when applicable; it is never imported into this Git repository. Do not commit that directory, `.env.hermes.local`, or session tokens, and do not paste them into issues or PRs.

## Quick start

Needs: Node.js, [pnpm](https://pnpm.io/), Rust (`cargo` on `PATH`). Build on macOS. Use pnpm only.

```bash
pnpm install
./scripts/sophonote.sh start
```

Start and stop only through `scripts/sophonote.sh`. Do not leave `pnpm tauri dev` running in the foreground. Frontend-only changes rely on Vite HMR. Restart with `./scripts/sophonote.sh restart` after Rust or Tauri contract changes.

Hermes ships as a pinned sidecar (Hermes 0.20.0 + CPython 3.11). Release builds start only the bundled runtime. Binaries are not in git. How to build the sidecar, attach an external Gateway, pack, and notarize is in [CONTRIBUTING.md](./CONTRIBUTING.md).


New conversations can also select **Pi** below the composer; the engine is fixed after the first Run. Pi 0.85.1 ships as a standalone executable and reuses OpenAI-compatible / Anthropic providers from AI model settings. For source development, run `pnpm pi:bundle` first (Python 3.12+; the initial download requires network access). Pi supports streaming, persistent sessions, cancellation, file approvals, and document diffs; shell commands require approval on macOS. This first integration does not expose Hermes browser, computer control, Skill/MCP management, or OpenViking memory through Pi.

**Claude Code** is also selectable in the same menu. Install the official [Claude Code CLI](https://code.claude.com/docs/en/setup) 2.1.233 or newer locally; SophoNote detects standard install locations (or the absolute `SOPHONOTE_CLAUDE_BIN` host environment override). It reuses Anthropic-protocol providers and official DeepSeek configurations from the shared AI model settings (DeepSeek is adapted to its official Anthropic endpoint without another API key), runs with isolated history and host-controlled tools, and does not use personal subscription login or global plugins. The CLI is not redistributed. Switching an existing session to another engine creates a new session and preserves unsent text and attachments.

Agent updates are grouped under **Settings → Agent configuration updates**. Hermes prepares a private version for the next application restart. Pi verifies an official stable archive and its host extension before activating a private slot for subsequent runs. Claude Code uses the recognized native/npm updater or original Homebrew cask; custom installations show manual instructions. Runtime updates keep model settings and conversation history intact.

## Contributing and security

Issues and PRs are welcome. Read [CONTRIBUTING.md](./CONTRIBUTING.md) before changing code: build, tests, hygiene scan, and pack matrix. Before you commit:

```bash
./scripts/oss-hygiene.sh
```

Do not discuss security issues in public. See [SECURITY.md](./SECURITY.md).

Change product judgment in the [PRD](./docs/PRD.md), technical facts in [architecture](./docs/architecture.md), and item status in the [ledger](./docs/project-ledger.md) (all Chinese). Do not add a second TODO, per-round progress file, or long-lived special PRD. Historical drafts are not in this repository.
