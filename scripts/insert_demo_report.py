import os
import sqlite3
import uuid
from pathlib import Path

DB = Path(os.environ.get(
    "SOPHONOTE_DB",
    str(Path.home() / "Library/Application Support/com.fei.sophonote/sophonote.db"),
))
today = "2026-08-05"

content = """# SophoNote 技术日报

## 2026-08-05

### 今日概览

今日共追踪 3 个数据源，收录 50 条内容：GitHub 热门仓库 20 个、arXiv 新论文 15 篇、HackerNews 热帖 15 篇。今日主线清晰：**Agent 基础设施正在从「框架热」转向「平台化与工程化」**，同时推理效率与评测方法学是学术侧的两大焦点。

### 热点仓库

| 仓库 | Stars | 语言 | 一句话 |
|---|---|---|---|
| NousResearch/hermes-agent | 225.9k | Python | 号称「与你一起成长的 Agent」，个人智能体方向的现象级项目 |
| vllm-project/vllm | 88.3k | Python | 高吞吐 LLM 推理引擎，持续领跑 serving 层 |
| paperclipai/paperclip | 75.7k | TypeScript | 「管理你的工作 Agent」的开源应用，Agent 管理面走红 |
| cloudflare/agents | 5.4k | TypeScript | 在 Cloudflare 边缘网络上构建部署 Agent |
| tursodatabase/turso | 23.7k | Rust | Rust 重写的 SQLite，实验性兼容 Postgres，自称「数据库界的 LLVM」 |
| firecrawl/anydoc | 3.4k | Rust | 各类办公文档转 Markdown，RAG 数据管道刚需工具 |
| datawhalechina/diy-llm | 1.1k | Jupyter | DataWhale 的 LLM 全栈构建课程，覆盖预训练到 RLHF |

### 论文风向

- **Agent 评测密集出现**：SocietyBench（反事实社会演化预测）、WorldCup Arena（无泄漏的前瞻性评测）、PAST-Bench（个人 Agent 递归自我改进）—— 三篇不约而同指向「Agent 需要面向未来、面向经验的评测」，而非静态任务完成率。
- **推理时扩展（Test-Time Scaling）开始被系统化审视**：《Test-Time Scaling in Reasoning LLMs》梳理推理范式、评估与可复现性，这个方向正从技巧走向方法论。
- **基础设施层的隐性 bug**：《When Attention Goes Blind》发现 ALiBi 位置编码的浮点下溢会导致注意力失效，值得所有用 ALiBi 的长上下文方案自查。
- **多模态继续向视频进军**：Video-DeepResearch 把 DeepResearch 范式从静态图拓展到连续视频流。

### 社区热议（HackerNews）

- **Mistral 发布 Shieldstral**：3B 开源权重的多模态内容审核模型（457 热度）—— 小模型专精化的又一例证。
- **Stateless MCP 重新获得关注**（314 热度）—— MCP 协议的无状态用法正在成为工程实践的讨论焦点，与 Agent 平台化趋势呼应。
- **《Position: LLMs Can't Jump》**（125 热度）—— 对 LLM 能力边界的立场性论文引发争论。
- **TIME 为 AI 爬虫提供带广告的不同版本网站**（77 热度）—— 内容方与 AI 公司的博弈出现新玩法。

### 趋势洞察

1. **Agent 的「管理层」浮出水面**：paperclip（管理 Agent）、Cloudflare Agents（部署 Agent）、Stateless MCP（连接 Agent）三类项目同日走热，说明社区重心正从「做出 Agent」转向「运营一组 Agent」。
2. **小模型专精化 + 大模型推理扩展**并行：Shieldstral（3B 审核）与 Test-Time Scaling 论文代表了成本曲线的两端——能用小模型的不用大模型，必须用大模型的就在推理时加算力。
3. **值得关注**：hermes-agent 的 22.6 万 star 表明「个人长期 Agent」叙事有极强的社区共识，配合 PAST-Bench 的「经验积累 → 行为改进」研究，个人 Agent 的记忆与自我改进可能是下一个热点。

---
*素材来源：GitHub Trending · arXiv (cs.AI/CL/LG) · HackerNews | 共 50 条*
*本报告由 Kimi 基于本地抓取素材生成（演示），配置 Deepseek API Key 后可由应用自动生成*
"""

conn = sqlite3.connect(DB)
conn.execute(
    "INSERT OR REPLACE INTO daily_logs (id, date, log_type, content, sources, generated_by, created_at) VALUES (?, ?, 'daily', ?, ?, 'kimi-demo', datetime('now','localtime'))",
    (str(uuid.uuid4()), today, content, 'github-trending,arxiv-ai,hackernews'),
)
conn.commit()
row = conn.execute("SELECT id, date, log_type, length(content) FROM daily_logs").fetchall()
print(row)
conn.close()
