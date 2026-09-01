# 安全问题披露

[English](./SECURITY.md) | **简体中文**

请勿在公开 Issue、PR、日志或截图中粘贴 API Key、Session Token、钥匙串导出、`.env.hermes.local` 或用户笔记正文。

## 如何报告

仓库启用 GitHub 私密漏洞报告 / Security Advisory 时请优先使用。若暂时没有私密通道，只开一个不含敏感细节的最小 Issue，请维护者提供私下联系方式。不要公开利用细节，也不要附带密钥、token、数据库、会话、工作室状态、笔记或其他用户数据。

## 安全边界

- 文档写入只有 DocumentService；Agent / Skill / MCP 不能直接改 `.md`。
- Hermes Runtime 只绑定环回地址，使用随机 Session Token；Release 不读取机器上的 `~/.hermes` 或 `PATH` 里的 hermes。
- Provider Key 不得写入源码、SQLite、日志或 WebView。Release 只走系统钥匙串。
- 外部 MCP 默认拒绝；用户授权后仍由 Hermes 持有连接，SophoNote 不存第二份密钥。
- WKWebView 不直连第三方 API；模型与 OpenRouter 等请求只经 Rust。

用户数据在本机 Application Support（见根 [README.zh-CN.md](./README.zh-CN.md)）。打包分发时保留 [NOTICE](./NOTICE) 中的 Hermes / CPython 声明。
