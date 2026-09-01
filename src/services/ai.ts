import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '../stores/appStore';
import type { Item } from '../types';
// 提示词版本唯一真相源 = src-tauri/prompt_versions.json（Rust include_str! 同一文件，零手工同步）。
// 每次改动提示词时递增对应版本号，AI 产出入库时带版本，便于回归对比。
import promptVersions from '../../src-tauri/prompt_versions.json';

export const PROMPT_VERSIONS = promptVersions;

/**
 * 正文分片（借鉴 khoj chunk 管线）：按段落切分，单片目标 ~maxChars 字符。
 * 过短片段并入前一片；返回不含空片的数组。
 */
export function chunkText(text: string, maxChars = 800): string[] {
  const clean = (text || '').replace(/\r/g, '').trim();
  if (!clean) return [];
  const paragraphs = clean.split(/\n{2,}/).map((p) => p.trim()).filter(Boolean);
  const chunks: string[] = [];
  let buf = '';
  for (const p of paragraphs) {
    if (buf && buf.length + p.length + 2 > maxChars) {
      chunks.push(buf);
      buf = p;
    } else {
      buf = buf ? `${buf}\n\n${p}` : p;
    }
    // 单段超长时硬切
    while (buf.length > maxChars) {
      chunks.push(buf.slice(0, maxChars));
      buf = buf.slice(maxChars);
    }
  }
  if (buf) chunks.push(buf);
  return chunks;
}

interface ChatMessage {
  role: 'system' | 'user' | 'assistant';
  content: string;
}

interface AIResponse {
  content: string;
  reasoning?: string;
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

/**
 * 聊天补全统一入口（Phase 0 / AG-01）：经 Tauri 命令走 Rust ModelGateway，
 * 项目内不再存在第二套直接 /chat/completions 调用；API Key 只在 Rust 侧读取，
 * React 不再持有 Key。配置（供应商/baseUrl/model）来自 settings.ai_config。
 */
export async function chatCompletion(
  messages: ChatMessage[],
  options: {
    model?: string;
    temperature?: number;
    promptVersion?: string;
  } = {}
): Promise<AIResponse> {
  const { settings } = useAppStore.getState();
  const ai = settings.aiConfig;
  if (!ai?.providers || !ai.activeProvider) {
    throw new Error('AI 配置未初始化，请到 设置 → AI 配置 选择供应商');
  }
  if (!ai.providers[ai.activeProvider]) {
    throw new Error(`供应商 ${ai.activeProvider} 未配置，请到 设置 → AI 配置 检查`);
  }

  const res = await invoke<ApiResponse<AIResponse>>('ai_chat_completion', {
    request: {
      messages,
      model: options.model ?? null,
      // 保持历史默认值 0.7（原前端 fetch 口径）
      temperature: options.temperature ?? 0.7,
      provider: null,
      promptVersion: options.promptVersion ?? null,
    },
  });
  if (!res.success) throw new Error(res.error || 'AI 调用失败');
  return {
    content: res.data?.content || '',
    reasoning: res.data?.reasoning,
    usage: res.data?.usage,
  };
}

// ==================== 向量嵌入（语义搜索） ====================

interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

/**
 * 生成文本向量。实际请求由 Rust 侧发起（WKWebView 的 fetch 有跨域限制会 Load failed），
 * 配置与 API Key 由后端从 settings.ai_config 和钥匙串读取。
 */
export async function generateEmbedding(text: string): Promise<number[]> {
  const { settings } = useAppStore.getState();
  if (settings.semanticSearchEnabled === false) {
    throw new Error('语义搜索已在设置中关闭，请到 设置 → AI 配置 → 向量嵌入 开启');
  }
  const cfg = settings.aiConfig?.embedding;
  if (!cfg?.baseUrl || !cfg?.model) {
    throw new Error('未配置嵌入模型，请到 设置 → AI 配置 → 向量嵌入 填写接口地址和模型');
  }
  const res = await invoke<ApiResponse<number[]>>('ai_generate_embedding', { text });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 测试嵌入接口连通性（设置页「测试连接」用），返回延迟与向量维度 */
export async function testEmbeddingConnection(): Promise<{ latencyMs: number; dimension: number }> {
  const started = Date.now();
  const vector = await generateEmbedding('ping');
  return { latencyMs: Date.now() - started, dimension: vector.length };
}

export async function testConnection(providerId: string): Promise<{ latencyMs: number }> {
  const res = await invoke<ApiResponse<{ latencyMs: number }>>('ai_test_chat_connection', {
    provider: providerId,
  });
  if (!res.success || !res.data) throw new Error(res.error || '连接失败');
  return { latencyMs: res.data.latencyMs };
}

export async function generateSummary(text: string): Promise<string> {
  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: '你是一个技术内容摘要专家。请用2-3句话总结以下内容的核心价值和技术亮点。保持简洁。',
    },
    {
      role: 'user',
      content: text,
    },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-flash' });
  return res.content;
}

export async function generateTags(text: string): Promise<string[]> {
  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: '为以下内容生成3-5个分类标签。只返回标签列表，用逗号分隔。',
    },
    {
      role: 'user',
      content: text,
    },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-flash', temperature: 0.3 });
  return res.content.split(/[,，]/).map((t) => t.trim()).filter(Boolean);
}

// ==================== 证据化解读（P0-4） ====================

export interface EvidenceItem {
  id: string;      // E1 / E2 ...
  kind: string;    // readme | release | metadata | article | discussion | abstract | model_card
  url: string;
  text: string;
}

export interface EnrichInput {
  title: string;
  type: Item['type'];
  metadata: Record<string, unknown>;
  evidence: EvidenceItem[];
}

// 速览解读的结构化结果（P0-4：关键判断必须标注证据编号）
export interface EnrichResult {
  summary: string;
  whyImportant: string;
  keyPoints: { text: string; evidence: string[] }[];
  risks: string[];
  confidence: 'high' | 'medium' | 'low';
  tags: string[];
}

/** 结构化速览结果 → 纯文本 ai_summary（关键点/风险已内联，供列表与兜底展示） */
export function composeEnrichSummary(result: EnrichResult): string {
  const parts = [result.summary];
  if (result.keyPoints.length > 0) {
    parts.push(result.keyPoints.map((k) => `✨ ${k.text}`).join('\n'));
  }
  if (result.risks.length > 0) {
    parts.push(result.risks.map((r) => `⚠️ ${r}`).join('\n'));
  }
  const confLabel = { high: '高', medium: '中', low: '低' }[result.confidence] || '低';
  if (result.whyImportant) parts.push(`💡 ${result.whyImportant} · 可信度：${confLabel}`);
  return parts.filter(Boolean).join('\n');
}

function formatEvidence(evidence: EvidenceItem[], maxCharsPerItem = 4000): string {
  return evidence
    .map((e) => `[${e.id}]（${e.kind}）${e.url}\n${e.text.slice(0, maxCharsPerItem)}`)
    .join('\n\n');
}

// ==================== 发现页 Top5 打分筛选（pick@v1） ====================

export interface PickCandidateInput {
  id: string;
  title: string;
  description: string;
  stars: number | null;
  publishedAt: string;
  fetchedAt: string;
}

export interface PickScore {
  itemId: string;
  score: number;   // 0-10 综合热度价值分
  reason: string;  // 一句话推荐理由（≤30 字）
}

/**
 * LLM 打分筛选：从候选中选出今日热度最高、最值得关注的 topN 条。
 * 打分维度：热度信号（stars/upvotes）× 时效性 × 主题价值，与发现页「最火 Top5」定位对齐。
 * 只依据提供的候选字段判断，不臆测候选之外的信息。
 */
export async function pickTopItems(
  categoryLabel: string,
  candidates: PickCandidateInput[],
  topN = 5
): Promise<PickScore[]> {
  const candText = candidates
    .map(
      (c, idx) =>
        `${idx + 1}. [id:${c.id}] ${c.title}\n   热度:${c.stars ?? 0} 发布:${c.publishedAt || '未知'} 抓取:${c.fetchedAt || '未知'}\n   简介:${(c.description || '（无）').slice(0, 160)}`
    )
    .join('\n');

  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: `你是一位技术信息热度评审官。针对「${categoryLabel}」类别的一组候选内容，从中选出今日热度最高、最值得关注的 ${topN} 条并打分。

打分标准（0-10 综合分）：
- 热度信号（约 50%）：stars/upvotes/讨论量等数值越高越好
- 时效性（约 25%）：越接近今天越有优势，陈旧内容降分
- 主题价值（约 25%）：AI/开发者工具/重大发布等方向优先；营销水文、纯娱乐降分

输出严格的 JSON 数组（不要输出任何其他文字、不要用 markdown 代码块包裹），按分数从高到低排列，最多 ${topN} 条：
[{"itemId": "候选的 id 原值", "score": 8.5, "reason": "一句话推荐理由，30字内"}]

硬性约束：
- itemId 必须原样取自候选列表，禁止编造
- reason 只依据候选提供的标题/简介/热度，禁止推测未给出的信息
- 候选不足 ${topN} 条时返回全部合格候选，宁缺毋滥（明显无价值的可以不选）`,
    },
    { role: 'user', content: `候选列表（共 ${candidates.length} 条）：\n${candText}` },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-flash', temperature: 0.3 });
  const validIds = new Set(candidates.map((c) => c.id));
  try {
    const jsonMatch = res.content.match(/\[[\s\S]*\]/);
    const parsed = JSON.parse(jsonMatch ? jsonMatch[0] : res.content);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((p: { itemId?: unknown }) => typeof p?.itemId === 'string' && validIds.has(p.itemId))
      .slice(0, topN)
      .map((p: { itemId: string; score?: unknown; reason?: unknown }) => ({
        itemId: p.itemId,
        score: Math.max(0, Math.min(10, Number(p.score) || 0)),
        reason: String(p.reason || '').slice(0, 60),
      }));
  } catch {
    return [];
  }
}

function formatMetadata(metadata: Record<string, unknown>): string {
  return Object.entries(metadata)
    .filter(([, v]) => v !== undefined && v !== null && v !== '')
    .map(([k, v]) => `${k}: ${Array.isArray(v) ? v.join(', ') : v}`)
    .join('\n');
}

const GROUNDING_RULES = `硬性约束：
- 只能根据提供的 E1、E2 等证据回答，禁止根据项目名称、标题或常识推测安装方式、性能、License、竞品。
- 每个关键判断后必须标注证据编号，如 [E1]。
- 证据中没有的信息必须回答「未披露」。
- confidence 评定标准：high=正文+多源证据完整；medium=只有摘要或单一正文；low=只有元数据。`;

// 为单条内容生成「速览解读」（P0-4：输入 metadata+evidence，输出带证据标注）
export async function enrichContent(input: EnrichInput): Promise<EnrichResult> {
  const typeLabel =
    input.type === 'repo' ? '开源仓库' : input.type === 'paper' ? '学术论文' : input.type === 'model' ? 'AI 模型' : '技术内容';

  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: `你是一位资深技术分析师，帮助读者高效判断一条技术信息的价值。
针对给定的${typeLabel}及其证据材料，输出严格的 JSON（不要输出任何其他文字、不要用 markdown 代码块包裹）：
{
  "summary": "一句话定位：它是什么、解决什么问题（30字内）",
  "whyImportant": "为什么值得关注（25字内）",
  "keyPoints": [
    { "text": "核心能力/亮点1（20字内）[E1]", "evidence": ["E1"] },
    { "text": "核心能力/亮点2 [E2]", "evidence": ["E2"] },
    { "text": "核心能力/亮点3 [E1]", "evidence": ["E1"] }
  ],
  "risks": ["证据中提到的限制或风险（没有则空数组）"],
  "confidence": "high | medium | low",
  "tags": ["3-5个分类标签"]
}
${GROUNDING_RULES}`,
    },
    {
      role: 'user',
      content: `标题：${input.title}\n\n元数据：\n${formatMetadata(input.metadata)}\n\n证据材料：\n${formatEvidence(input.evidence)}`,
    },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-flash', temperature: 0.3 });
  try {
    const jsonMatch = res.content.match(/\{[\s\S]*\}/);
    const parsed = JSON.parse(jsonMatch ? jsonMatch[0] : res.content);
    const confidence = ['high', 'medium', 'low'].includes(parsed.confidence) ? parsed.confidence : 'low';
    return {
      summary: String(parsed.summary || ''),
      whyImportant: String(parsed.whyImportant || ''),
      keyPoints: Array.isArray(parsed.keyPoints)
        ? parsed.keyPoints.map((k: { text?: unknown; evidence?: unknown }) => ({
            text: String(k?.text || ''),
            evidence: Array.isArray(k?.evidence) ? k.evidence.map(String) : [],
          }))
        : [],
      risks: Array.isArray(parsed.risks) ? parsed.risks.map(String) : [],
      confidence,
      tags: Array.isArray(parsed.tags) ? parsed.tags.map(String) : [],
    };
  } catch {
    return { summary: res.content.slice(0, 200), whyImportant: '', keyPoints: [], risks: [], confidence: 'low', tags: [] };
  }
}

// 为单条内容生成「深度解读」（pro 模型，复用同一份 evidence，关键判断标注 [Ex]）
export async function deepDive(input: {
  title: string;
  type: Item['type'];
  url?: string;
  metadata: Record<string, unknown>;
  evidence: EvidenceItem[];
}): Promise<string> {
  const keyInfoByType: Record<string, string> = {
    repo: '语言 | 主要技术栈\nStars/热度 | 增长趋势\n核心功能 | 2-3 个关键能力\n安装方式 | 一行命令\n活跃度 | 维护状态\nLicense | 开源协议',
    paper: '研究问题 | 一句话\n方法 | 核心方法\n数据/实验 | 关键设置\n结论 | 最重要结果\n局限 | 适用边界',
    model: '任务类型 | pipeline\n基座模型 | base model\nLicense | 协议\n下载/点赞 | 热度\n评测表现 | benchmark（如有）',
    default: '定位 | 一句话\n目标用户 | 谁该用\n核心功能 | 2-3 个\n商业模式 | 如何收费/盈利\n竞品 | 同类代表',
  };
  const keyInfo = keyInfoByType[input.type] || keyInfoByType.default;

  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: `你是一位资深技术分析师。针对给定的一条技术信息及其证据材料，输出结构化的深度解读（中文，Markdown）。

结构要求（严格按此章节）：
## 核心定位
（一段话讲透它是什么、解决什么问题）

## 关键信息
（Markdown 表格，两列「维度 | 内容」，覆盖以下维度；证据未覆盖的填「未披露」：
${keyInfo}）

## 技术亮点
（实现原理、架构或方法上的关键决策。涉及架构、流程、数据流时，必须用 \`\`\`mermaid 代码块绘制 flowchart TD 或 graph TD 图辅助说明）

## 与同类对比
（Markdown 表格：维度 | 本项目 | 同类A | 同类B，3-5 个对比维度；仅当证据支持时输出，否则整节写「证据不足」）

## 为什么值得关注
（放在行业趋势里看：它代表什么方向，解决什么真问题）

## 对你的价值
（从效率与认知提升角度：可以怎么用、可以学什么、是否需要跟进）

硬性约束：
- 全文 600 字以内（不含图表）
- mermaid 节点文字用中文，节点 id 用英文，文字内不要出现引号、括号、冒号
- 表格单元格不超过 15 字
${GROUNDING_RULES}`,
    },
    {
      role: 'user',
      content: `标题：${input.title}\n链接：${input.url || '无'}\n\n元数据：\n${formatMetadata(input.metadata)}\n\n证据材料：\n${formatEvidence(input.evidence)}`,
    },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-pro', temperature: 0.5 });
  return res.content;
}

export async function generateDailyReport(items: { title: string; description: string; type: string }[]): Promise<string> {
  const content = items
    .map((i, idx) => `${idx + 1}. [${i.type}] ${i.title}\n${i.description}`)
    .join('\n\n');

  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: `你是一位资深技术分析师。请基于以下内容生成一份结构化的技术日报。

格式要求：
# SophoNote 技术日报
## 日期

### 今日概览
- 内容统计

### 热点仓库/论文/产品
（分条列出，每条包含名称、核心亮点）

### 趋势洞察
（总结今日内容的共同主题和趋势）

规则：标题带「（多源验证：N 个信源）」的条目已被多个独立信源交叉验证，优先列入热点并保留该标注。

语言：中文`,
    },
    {
      role: 'user',
      content: `以下是今日收集的技术内容，请生成日报：\n\n${content}`,
    },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-pro' });
  return res.content;
}

export async function generateWeeklyReport(items: { title: string; description: string; type: string }[]): Promise<string> {
  const content = items
    .map((i, idx) => `${idx + 1}. [${i.type}] ${i.title}\n${i.description}`)
    .join('\n\n');

  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: `你是一位资深技术分析师。请基于本周内容生成技术周报。

格式要求：
# SophoNote 技术周报
## 周期

### 本周趋势总结
### AI/开源/产品重点进展
### 值得关注的新方向
### 下周展望

语言：中文`,
    },
    {
      role: 'user',
      content: `以下是本周收集的技术内容，请生成周报：\n\n${content}`,
    },
  ];

  const res = await chatCompletion(messages, { model: 'deepseek-v4-pro' });
  return res.content;
}

// ==================== AG-03：AI 归属整理（AI 工作室项目模式试验田） ====================
// 设计基线：docs/architecture.md。
// 模型调用走统一 Gateway（ai_chat_completion）；建议 → 用户确认 → assign_document 写入，
// 是「直接写 + 简易确认」范式在智能体写入场景的首次落地。

export interface AssignProjectRef {
  id: string;
  name: string;
  description?: string | null;
}

export interface AssignDocumentRef {
  id: string;
  title: string;
  /** 可读类型标签（笔记/日记/深度解读…） */
  typeLabel: string;
  /** 正文摘录（≤300 字，作为归属判断证据） */
  excerpt: string;
}

export type AssignProposal =
  | { documentId: string; action: 'assign'; projectId: string; reason: string }
  | { documentId: string; action: 'newProject'; newProjectName: string; reason: string }
  | { documentId: string; action: 'skip'; reason: string };

/** AI 归属整理：为未归属文档逐个建议去向（归入现有项目 / 建议新项目 / 跳过）。
 *  返回的建议已做合法性过滤（projectId/documentId 均校验存在）。 */
export async function suggestProjectAssignments(
  projects: AssignProjectRef[],
  docs: AssignDocumentRef[]
): Promise<AssignProposal[]> {
  const projectLines = projects.length
    ? projects
        .map((p) => `- ${p.id} | ${p.name}${p.description ? ` | 目标：${p.description}` : ''}`)
        .join('\n')
    : '（当前没有项目。若文档值得归类，请建议创建新项目）';
  const docLines = docs
    .map((d) => `- ${d.id} | [${d.typeLabel}] ${d.title}\n  摘录：${d.excerpt || '（无正文）'}`)
    .join('\n');

  const messages: ChatMessage[] = [
    {
      role: 'system',
      content: `你是 SophoNote 的知识整理助手，负责把用户的文档归入项目。项目是扁平分组容器：一篇文档只能属于一个项目。

判断规则：
1. 文档主题与某个现有项目（名称与目标）明确契合：action=assign
2. 现有项目都不合适、但内容值得独立归类（或当前没有项目）：action=newProject，给出简洁的项目名（4-12 字，与现有项目名风格一致）
3. 内容过少无法判断、主题过杂、或碎片化不值得归类：action=skip
保守原则：拿不准就 skip，宁可暂不归类也不强行归属。

输出严格 JSON（不要输出任何其他文字、不要用 markdown 代码块包裹）：
{"assignments":[{"documentId":"...","action":"assign","projectId":"...","reason":"一句话理由（30字内）"},{"documentId":"...","action":"newProject","newProjectName":"...","reason":"一句话理由（30字内）"},{"documentId":"...","action":"skip","reason":"一句话理由（30字内）"}]}
每篇文档必须恰好出现一次。`,
    },
    {
      role: 'user',
      content: `## 现有项目（id | 名称 | 目标）\n${projectLines}\n\n## 待归属文档（id | 类型 | 标题，附正文摘录）\n${docLines}`,
    },
  ];

  const res = await chatCompletion(messages, {
    temperature: 0.2,
    promptVersion: PROMPT_VERSIONS.projectAssign,
  });

  let parsed: any;
  try {
    const jsonMatch = res.content.match(/\{[\s\S]*\}/);
    parsed = JSON.parse(jsonMatch ? jsonMatch[0] : res.content);
  } catch {
    throw new Error('AI 返回格式异常，请重试');
  }

  const validDocIds = new Set(docs.map((d) => d.id));
  const validProjectIds = new Set(projects.map((p) => p.id));
  const seen = new Set<string>();
  const out: AssignProposal[] = [];
  for (const a of Array.isArray(parsed?.assignments) ? parsed.assignments : []) {
    const documentId = String(a?.documentId || '');
    // 合法性过滤：未知文档 / 重复文档直接丢弃；未知 projectId 降级为 skip
    if (!validDocIds.has(documentId) || seen.has(documentId)) continue;
    seen.add(documentId);
    const reason = String(a?.reason || '').slice(0, 120);
    if (a?.action === 'assign' && validProjectIds.has(String(a.projectId || ''))) {
      out.push({ documentId, action: 'assign', projectId: String(a.projectId), reason });
    } else if (a?.action === 'newProject' && String(a?.newProjectName || '').trim()) {
      out.push({
        documentId,
        action: 'newProject',
        newProjectName: String(a.newProjectName).trim().slice(0, 20),
        reason,
      });
    } else {
      out.push({ documentId, action: 'skip', reason });
    }
  }
  return out;
}
