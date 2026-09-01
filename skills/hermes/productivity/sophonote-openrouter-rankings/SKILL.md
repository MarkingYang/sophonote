---
name: sophonote-openrouter-rankings
description: Maintain SophoNote's official OpenRouter model-ranking snapshot. Use when a user asks to refresh, update, inspect, or schedule the Discover model board, OpenRouter rankings, popular models, model market share, task leaders, coding-agent session cost, model capabilities, or benchmark data.
---

# SophoNote OpenRouter Rankings

Maintain the Discover model board from OpenRouter's official structured APIs. Do not infer rankings from AI news, AIHOT, prose, search results, or model opinion.

## Routes

- `action=refresh`: refresh the complete OpenRouter snapshot.
- `action=read`: read the latest stored snapshot metadata and section counts without network access.
- Natural-language equivalents such as “更新模型榜”“刷新 OpenRouter 排名”“模型榜现在是什么情况” map to these routes.
- Schedule maintenance with Hermes native Cron only; `skills=["sophonote-openrouter-rankings"]`, `deliver="local"`, and an authenticated provider/model. SophoNote does not keep a second scheduler.

## Refresh workflow

1. **First-action hard gate:** immediately call `mcp__sophonote_bridge__refresh_openrouter_rankings` exactly once with `{}`. This must be the first tool call of the run. Do not call `skill_view`, `tool_describe`, browser, terminal, or any other tool before it; do not spend a model round restating or planning this workflow.
2. Treat the returned `asOf`, `fetchedAt`, section counts, and citation as authoritative. The Host reads the API key from Keychain, calls OpenRouter, validates every response, and atomically replaces the snapshot. The static contract reference is for maintenance only and is not a prerequisite tool read during refresh.
3. If refresh succeeds, reply concisely: `OpenRouter 模型榜已更新：热门模型 N、任务分类 N、会话成本 N、基准 N；数据截至 <asOf>。`
4. If refresh fails, state the returned reason. Never claim that a previous snapshot was refreshed, and never fall back to scraping, browser automation, terminal HTTP, AIHOT, or guessed data.

## Read workflow

1. Call `mcp__sophonote_bridge__read_openrouter_rankings`.
2. Summarize only fields present in the snapshot. Keep model identifiers, units, dates, and shares exact.
3. For usage and task-ranking data, include: `Source: OpenRouter (openrouter.ai/rankings), as of <asOf>.`
4. If no snapshot exists, ask the user to configure an OpenRouter rankings key in SophoNote and run `action=refresh`.

## Safety and fidelity

- Never request, print, echo, persist, or transform the API key. It is not a Skill argument.
- Never call OpenRouter directly. Third-party HTTP belongs to the SophoNote Rust Host.
- Do not modify discovery items, scores, quick/deep articles, picks, reports, or AIHOT data.
- Do not recompute or rename OpenRouter's `total_tokens`, task shares, median session cost, context length, supported parameters, modalities, or benchmark scores.
- Preserve the most recent successful snapshot when any endpoint fails; partial refreshes are invalid.
- The model board is a native SophoNote view. Do not iframe or copy OpenRouter's website HTML.

## Recommended schedule

One daily task is sufficient because usage datasets and benchmarks refresh on their own upstream cadence:

```text
name=OpenRouter 模型榜更新
schedule=30 8 * * *
skills=["sophonote-openrouter-rankings"]
deliver=local
```

任务内容使用中文自然语言：

```markdown
请使用「sophonote-openrouter-rankings」Skill 更新完整的 OpenRouter 模型榜快照，并严格按照该 Skill 的 Markdown 说明执行。

仅在榜单发生变化时通知。
```

If the snapshot is unchanged, Cron may return `[SILENT]`. Never create a high-frequency retry loop for missing credentials or an unavailable upstream API.
