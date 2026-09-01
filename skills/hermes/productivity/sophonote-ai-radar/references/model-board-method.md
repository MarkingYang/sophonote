# 模型榜方法（v1 共识分）

## 数据面

- 窗口：7 天，周更（Cron 每周一）。
- 候选：model lane 入选条目 + HuggingFace 趋势榜位。
- 归并：同名模型跨条目按 `model_key` 归并（Skill 在打分/榜单趟标注）；名称与厂商取自最权威条目（官方博客 > 官方仓库 > 媒体）。

## 共识分（0-100）

```text
consensus = 0.5 × (AI 分 × 10)
          + 0.3 × HF 趋势位归一
          + 0.2 × 热度归一（stars 或下载量，窗口内增量优先）
```

- HF 趋势位归一：榜位 r（1 起）→ `1 - (r-1)/max(榜单长度, 20)`，截断到 [0,1]。
- 热度归一：窗口内候选的 min-max 归一。
- 因子缺失时按剩余权重归一（例如无 HF 榜位时 = 0.625×AI分 + 0.375×热度），不猜值、不补默认中位数。

## 快照契约

`save_model_board_snapshot`（🔜 NEXT-050）：

- 字段：`date`（周一日期）、`model_key`、`name`、`vendor`、`rank`、`consensus`、`meta_json`（各因子原始值与证据 itemId）。
- 唯一约束 `(date, model_key)`，重跑幂等覆盖。
- rank 按 consensus 倒序连续编号；并列时按 name 字典序定序，不跳号。

## 输出纪律（学习 aihot hot-topics）

- 只展示名次与共识分，不暴露内部热度原始值。
- v2 之前口径一律标注「SophoNote 共识分」；v2（另立后续）聚合外部公开榜单（LMSYS Arena、Artificial Analysis、OpenRouter 排行等）后改标「综合 N 家榜单」，分榜明细存 `meta_json`。
