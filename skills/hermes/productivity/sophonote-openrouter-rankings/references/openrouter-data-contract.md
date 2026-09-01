# OpenRouter official data contract

SophoNote's Rust Host owns all calls and stores one atomic raw snapshot. The Skill does not call these URLs itself.

| Snapshot field | Official endpoint | Product use |
|---|---|---|
| `models` | `GET https://openrouter.ai/api/v1/models?sort=top-weekly` | names, vendors, pricing, context, modalities, supported parameters, capability filters |
| `rankingsDaily` | `GET https://openrouter.ai/api/v1/datasets/rankings-daily?period=day` | 30-day model usage, weekly top models, trend, vendor market share |
| `taskClassifications` | `GET https://openrouter.ai/api/v1/classifications/task?window=7d` | macro categories, task shares, leading models per task |
| `sessionCost` | `GET https://openrouter.ai/api/v1/datasets/session-cost?limit=500` | published harness/model median cost by turn range |
| `benchmarks` | `GET https://openrouter.ai/api/v1/benchmarks?source=artificial-analysis` | intelligence, coding and agentic benchmark comparison with pricing |

Required behavior:

- Authenticate in the Host with the Keychain secret `openrouter-rankings`.
- Use a bounded timeout and fail the whole refresh if any required response is non-2xx, invalid JSON, or missing its `data` field.
- Persist `fetchedAt`, the most conservative available `asOf`, raw section payloads, the official rankings URL, and the exact citation.
- Retain the previous successful snapshot on failure.
- Respect OpenRouter rate limits; one refresh uses five requests and should run daily, not continuously.
- Public usage data is CC BY 4.0 and requires attribution: `Source: OpenRouter (openrouter.ai/rankings), as of {as_of}.`

Rendering semantics:

- `total_tokens` is a decimal string; parse without silently truncating large values.
- Token totals originate from different provider tokenizers and are a traffic indicator, not a cross-model quality score.
- Task classification shares are sampled fractions; display as shares/percentages, not absolute traffic.
- `median_session_cost_usd` is an observed published-harness median, not an estimate.
- Benchmark indices are separate dimensions. Do not collapse them into a hidden SophoNote consensus score.
