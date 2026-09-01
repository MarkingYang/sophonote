# SophoNote

**English** | [简体中文](./README.zh-CN.md)

SophoNote is a local-first AI knowledge workbench for macOS, built around open-source [Hermes Agent](https://github.com/NousResearch/hermes-agent). Notes, discovery, permissions, and document review stay in SophoNote; Hermes remains the agent runtime. This project is neither a Hermes Desktop fork nor a UI clone.

SophoNote is a **client surface** around a pinned Hermes sidecar:

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
| Conversation | Task-scoped Hermes sessions; optional temporary local-directory binding |
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

## Contributing and security

Issues and PRs are welcome. Read [CONTRIBUTING.md](./CONTRIBUTING.md) before changing code: build, tests, hygiene scan, and pack matrix. Before you commit:

```bash
./scripts/oss-hygiene.sh
```

Do not discuss security issues in public. See [SECURITY.md](./SECURITY.md).

Change product judgment in the [PRD](./docs/PRD.md), technical facts in [architecture](./docs/architecture.md), and item status in the [ledger](./docs/project-ledger.md) (all Chinese). Do not add a second TODO, per-round progress file, or long-lived special PRD. Historical drafts are not in this repository.
