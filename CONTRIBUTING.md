# Contributing

**English** | [简体中文](./CONTRIBUTING.zh-CN.md)

SophoNote is a local-first macOS Hermes client surface. The product UI is Chinese. Community docs ship as two files: English (no suffix) and Simplified Chinese (`.zh-CN.md`). Product intro and quick start: [README](./README.md).

Issues and pull requests are welcome. Search existing issues first. Report vulnerabilities via [SECURITY.md](./SECURITY.md), not a public issue.

## Environment

- Node.js and [pnpm](https://pnpm.io/) (do not install with npm)
- Rust: `cargo` on `PATH` (you may use `$HOME/.cargo/bin`)
- Verification baseline: Hermes Agent 0.20.0 (pinned sidecar, see below)
- Build Apple targets on macOS; build Windows NSIS on Windows x64

```bash
node --version
pnpm --version
rustc --version
```

## Repository boundary

- Keep application source and docs in this repository root. Do not create a second SophoNote docs tree outside the repo.
- Change product, architecture, and progress only in `docs/PRD.md`, `docs/architecture.md`, and `docs/project-ledger.md`. See the [docs map](./docs/README.md). Do not add a second TODO / PROGRESS / dated audit or long-lived special PRD.
- Current docs are flat under `docs/`. Historical drafts are not in this repository. Do not recreate `docs/history/`, `docs/current/`, or `docs/guides/`.
- Pinned Hermes binaries are not in git. Clone [Hermes Agent](https://github.com/NousResearch/hermes-agent) at the pinned commit, `export HERMES_SOURCE_DIR=...`, then run `pnpm hermes:bundle`.

## Development

```bash
pnpm install
./scripts/sophonote.sh start
pnpm exec tsc --noEmit
pnpm test --run
```

Start and stop only through `scripts/sophonote.sh`. Do not leave `pnpm tauri dev` running in the foreground. Use pnpm. Frontend-only changes rely on Vite HMR. Restart after Rust or Tauri contract changes.

```bash
./scripts/sophonote.sh status
./scripts/sophonote.sh logs
./scripts/sophonote.sh stop
```

Rust:

```bash
cd src-tauri
export PATH="$HOME/.cargo/bin:$PATH"
cargo check
cargo test --all-targets
```

On a real Hermes path, `logs/dev.log` should show `resolve=Use(Hermes)` and `surface=gateway`. Acceptance must cover session create/resume, Skills, attachments, approval/clarify, cancel, and a final answer. A reachable port is not enough.

## Pull requests

1. Branch from current `main`. Keep the change small.
2. Change the PRD or architecture before code when requirements or design change. Update the existing ledger row for status.
3. Frontend-only: `pnpm exec tsc --noEmit` and related `pnpm test --run`. Rust-only: `cargo check` and related tests. Tauri contract changes: check both sides, then `./scripts/sophonote.sh restart`.
4. Run `./scripts/oss-hygiene.sh` and `git diff --check` before commit.
5. Explain why and how to verify. Do not paste secrets, maintainer absolute paths, or user notes.
6. Product UI copy is Chinese. Community entry docs are paired English + `.zh-CN.md`. Canonical PRD / architecture / ledger body is Chinese.

## Architecture red lines

- Rust owns files, SQLite, third-party HTTP, models, embeddings, and subprocesses. The frontend only renders and invokes.
- `.md` is the note-body source of truth. An Agent may only propose a Patch. DocumentService writes after user approval.
- Skills do not grant permissions. MCP is deny-by-default. The model cannot install or authorize MCP.
- API keys must not be written to source, docs, logs, or the WebView. Release builds use Keychain only.

## Hermes sidecar

Hermes 0.20.0 + CPython 3.11 ships inside the macOS `.app` and Windows NSIS package. Release starts only the bundled runtime. It does not read machine `PATH`, `~/.hermes`, a source checkout, or `SOPHONOTE_HERMES_*`. Debug builds may still attach an external Gateway explicitly.

Pinned binaries are not in git. Contributors build locally:

```bash
git clone https://github.com/NousResearch/hermes-agent
cd hermes-agent
git checkout 07da945c214481083049500bd29f45cabc5a04b2
export HERMES_SOURCE_DIR="$PWD"
# return to the SophoNote repo
pnpm hermes:bundle
```

`HERMES_SOURCE_DIR` must point at that commit. `uv.lock` and the source-tree hash must match the pins in `scripts/build-hermes-sidecar.sh`.

## Attach external Hermes (optional, Debug only)

`./scripts/sophonote.sh start` starts the bundled pinned sidecar when no production Gateway is configured. Create a local file only if you need an existing Gateway or a non-default port:

```bash
cp .env.hermes.example .env.hermes.local
```

`SOPHONOTE_HERMES_GATEWAY_TOKEN` must equal Hermes `HERMES_DASHBOARD_SESSION_TOKEN`:

```dotenv
SOPHONOTE_HERMES_ATTACH_EXTERNAL=1
SOPHONOTE_HERMES_GATEWAY_URL=ws://127.0.0.1:9119/api/ws
SOPHONOTE_HERMES_GATEWAY_TOKEN=<the same random token>
SOPHONOTE_HERMES_HOME=
```

`SOPHONOTE_HERMES_HOME` must be an expanded absolute path. Hermes Gateway binds loopback only. Generate a token with `openssl rand -hex 32`, put it in Hermes’ own private env file, and do not commit it here.

```bash
HERMES_DASHBOARD_SESSION_TOKEN=<the same random token> hermes serve --host 127.0.0.1 --port 9119 --skip-build
```

Without `SOPHONOTE_HERMES_ATTACH_EXTERNAL=1`, Debug also starts only the bundled sidecar, so it does not pollute Hermes Desktop’s separate session, memory, or attachments. `*.local` is gitignored.

A Debug bundle launched from Finder does not load `.env.hermes.local`. To verify an external Hermes, run from the project root:

```bash
set -a
. ./.env.hermes.local
set +a
./src-tauri/target/debug/bundle.noindex/macos/SophoNote.app/Contents/MacOS/sophonote
```

## Open-source hygiene

Before commit, from the repository root:

```bash
./scripts/oss-hygiene.sh
```

Gates:

1. No secrets, tokens, or maintainer absolute paths as defaults.
2. Do not add `logs/`, `.env*` (except the example), databases, conversations, Studio state, user `notes/`, Hermes Home, or generated runtime bundles to git.
3. Keep third-party licenses and update `NOTICE` when needed.
4. Keep Git author identity repository-local and privacy-safe (for example, a GitHub noreply address). Never rewrite published history or force-push without an explicit maintainer decision.

Security issues: [SECURITY.md](./SECURITY.md).

## Packing

Pack on the matching host. Hermes CPython cannot be cross-compiled. An unsigned artifact is not an RC.

| Command | Host | Artifact | RC? |
|---|---|---|---|
| `pnpm pack:macos` | Apple Silicon macOS (Intel Macs produce the x86_64 second arch) | `.app` + DMG | No |
| `pnpm pack:windows` | Windows x64 (Git Bash / CI bash) | NSIS `setup.exe` | No |
| `pnpm release:macos` | macOS release machine with Developer ID | notarized DMG | Yes |
| `pnpm release:windows` | Windows release machine with Authenticode | signed NSIS | Yes |

```bash
export HERMES_SOURCE_DIR=/path/to/hermes-agent   # pinned commit
pnpm pack:macos     # macOS only
pnpm pack:windows   # Windows only
```

Official RC: `pnpm release:macos` or `pnpm release:windows`. GitHub Actions `pack.yml` produces unsigned packs only; it does not turn an artifact into an RC.

The macOS build script writes a Spotlight exclusion under `src-tauri/target/` and places the `.app` in `bundle.noindex/` when done. The install target is always `/Applications/SophoNote.app`:

```bash
pnpm install:macos -- "$PWD/src-tauri/target/release/bundle.noindex/macos/SophoNote.app"
```

The script checks the app name and Bundle ID, then replaces the old package entirely. It does not merge old directories and does not modify `~/Library/Application Support/com.fei.sophonote/`.

Reproducible verification without local Apple credentials, notarization steps, and the clean-machine matrix are in [architecture §17](./docs/architecture.md#17-部署环境与基础设施) and [architecture §20](./docs/architecture.md#20-测试与验证方案) (Chinese).
