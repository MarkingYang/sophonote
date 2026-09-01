# SophoNote docs

**English** | [简体中文](./README.zh-CN.md)

Community entry: [README](../README.md). This page is the docs map.

The product UI is Chinese. Community docs ship as two files: English (no suffix) and Simplified Chinese (`.zh-CN.md`). The canonical body of the PRD, architecture, ledger, and widget spec is Chinese. There is no second full English edition of those files.

Current files are flat under `docs/`. Do not recreate `current/` or `guides/`. Historical drafts are not in this repository. The files listed here are authoritative.

## Users

| Doc | Role |
|---|---|
| [User manual](../skills/hermes/productivity/sophonote-help/references/user-manual.md) | Six surfaces, session permissions, Studio, Notes, Browser (Chinese, matches the UI) |
| [Markdown writing](./sophonote-markdown-writing.md) | Current document or selection, review-to-disk, titles, project folders |

Data directories, secrets, and sidecar boundaries are in the root README and [SECURITY.md](../SECURITY.md).

## Contributors

| Doc | Role |
|---|---|
| [Contributing](../CONTRIBUTING.md) | Environment, PRs, Hermes sidecar, attach, packing, hygiene |
| [License](../LICENSE) / [NOTICE](../NOTICE) | MIT; Hermes and CPython redistribution |
| [Security](../SECURITY.md) | Vulnerability reports; do not paste keys or notes in public issues |
| [Hermes runtime diagram](./hermes-runtime-architecture.html) | DEC-011 interactive diagram (source: same-name `.archify.json`) |
| [Current architecture diagram](./sophonote-current-architecture.html) | Product shell + Hermes execution plane |

## Product and architecture sources of truth

These files have different jobs. Do not copy them into each other. Do not add a second TODO, PROGRESS, dated audit, or long-lived special PRD.

| Doc | Sole job | When to edit |
|---|---|---|
| [PRD](./PRD.md) | Why, what, acceptance | When product judgment changes |
| [Architecture](./architecture.md) | How it works now, how it will change | When technical facts or design change |
| [Ledger](./project-ledger.md) | Progress, issues, decisions, evidence | When an item’s status changes |
| [Widget spec](./widget-component-library.spec.md) | WidgetKit / Control Center projection (macOS) | When DEC-042 design changes |

Status words (PRD and architecture): `已实现` means the code exists; `自动验证` means tests passed; `已验收` requires real macOS/Tauri host walkthrough evidence. Do not write goals as done.

Hermes attach, diagnosis, packing, signing/notarization, and clean-machine acceptance are in [architecture §§17, 20, 21](./architecture.md) and [CONTRIBUTING.md](../CONTRIBUTING.md). Agent working rules are in [CLAUDE.md](../CLAUDE.md) and [AGENTS.md](../AGENTS.md).
