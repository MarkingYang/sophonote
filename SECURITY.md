# Security

**English** | [简体中文](./SECURITY.zh-CN.md)

Do not paste API keys, session tokens, keychain exports, `.env.hermes.local`, or user note bodies into public issues, PRs, logs, or screenshots.

## How to report

Use GitHub private vulnerability reporting / Security Advisories when it is enabled for this repository. If no private channel is available, open only a minimal, non-sensitive issue asking the maintainers for a private contact path. Do not publish exploit details or attach secrets, tokens, databases, conversations, Studio state, notes, or other user data.

## Security boundaries

- Only DocumentService writes documents. Agent / Skill / MCP cannot edit `.md` directly.
- Hermes Runtime binds loopback only and uses a random session token. Release builds do not read `~/.hermes` or `hermes` on `PATH`.
- Provider keys must not be written to source, SQLite, logs, or the WebView. Release builds use the OS keychain only.
- External MCP is deny-by-default. After user authorization, Hermes still owns the connection. SophoNote does not store a second copy of the secret.
- WKWebView does not call third-party APIs directly. Model and OpenRouter requests go through Rust only.

User data lives in local Application Support (see the root [README](./README.md)). Keep the Hermes / CPython notices in [NOTICE](./NOTICE) when you redistribute a package.
