# aihot 信源契约（已接入）

> aihot（aihot.virxact.com）是高质量中文 AI 资讯策展池，覆盖本雷达五个原始信源（github/arxiv/hn/ph/hf）缺失的「行业 / 观点」面。
> 状态：**Rust 抓取器已就绪**（NEXT-051）——Host 通过官方匿名只读 v1 API 拉取 `mode=selected&window=24h` 精选池入库；Skill 一律经 Bridge 消费，不得绕过 Bridge 直接调用 aihot API 或抓网，也不得把训练记忆冒充 aihot 数据。

## 用途合规边界（必须遵守）

- aihot 公开使用规则 1.0：个人非商业、公益非商业和组织内部使用免费。SophoNote 本地采集入库属个人/内部使用；**不得**用于对外商业产品、数据转售、公开镜像或批量再分发。
- 标注来源（attribution）不代表取得授权；条目版权归原作者，经 aihot 不改变。
- API 匿名只读、无需 Key；但只说明访问方式，不改变上述用途边界。

## Host 抓取合同（Rust 已实现）

- 端点：`GET https://aihot.virxact.com/api/v1/items?mode=selected&window=24h&limit=50`；ETag/304 增量（ETag 存 `sources.config.aihotEtag`）；定时间隔 60 分钟（≥ 对方要求的 60 秒下限）。
- 抓取失败按部分失败降级（跳过该源、记入源健康），不切换其它新闻源冒充 aihot。
- 完整 API 参考（hot-topics / dailies / stories 等端点当前未接入）：khazix-skills 仓库 `aihot/references/api.md`。

## 入库映射（已实现于 scheduler.rs）

| aihot 字段 | SophoNote `items` 字段 |
| --- | --- |
| `id` | id（前缀 `aihot-`） |
| `title` / `originalTitle` | title / 原标题不同时入 description 头 |
| `summary` | description（不替代我方 AI 摘要） |
| `links.original` | url（主链接）；缺失时回退 `links.aihot` |
| `publishedAt` / `discoveredAt` | published_at（可空回退 discoveredAt）/ fetched_at |
| `source.name` | author（原始出处媒体） |
| `category` | topics 留档 `aihot:<slug>`，作 aspect 提示（见 aspect-rules.md），不是受控主题 |
| `score` | stars（0-100，仅源内候选排序热度，不覆盖我方打分） |

- item_type：`category=ai-models→model`、`paper→paper`、`ai-products→product`、其余 `article`。
- lane 门禁：aihot 条目可入 `model`（ai-models / paper）或 `product`（ai-products / industry / tip）lane；无 github lane。
- 候选排序：源内按 stars（aihot score）降序，与其它信源一致走 `prefilterLimitPerSource` 边界。
- aihot 条目同样走紧凑打分趟（我方按 0-10 口径重新打分），其 `selected=true` 与 aihot score 只作先验提示，不豁免门槛；与我方既有订阅信源语义重叠 ≥80% 同样拒绝。

## 时间口径（与本雷达一致）

`by=timeline`（默认）：原文发布 72 小时内收录按收录时间；超 72 小时回填归位原文发布日。与 SKILL.md「时间口径」同义，收窄窗口时用与 `by` 一致的时间轴值。
