import type { Item } from '../types';
import type { EnrichResult, EvidenceItem } from './ai';

/**
 * N1 一键沉淀：把信息流条目转为个人笔记的 Markdown 模板。
 * 携带来源反链（`sophonote:item/<id>` 内部协议，预览可点击跳回阅读视图）与关键证据引用，
 * 各区块按数据有无裁剪——没有速览也能沉淀（留白引导用户自己写）。
 */
export function buildSedimentMarkdown(
  item: Item,
  enrich: EnrichResult | null,
  evidence: EvidenceItem[]
): string {
  const lines: string[] = [];

  // 来源反链区：外部原文 + 应用内跳回原条目
  const sourceBits = [item.sourceId, new Date(item.publishedAt).toLocaleDateString('zh-CN')];
  const backlink = `[↩ 回到原条目](sophonote:item/${item.id})`;
  const sourceLine = item.url
    ? `> **来源** [${item.title}](${item.url}) · ${sourceBits.join(' · ')} · ${backlink}`
    : `> **来源** ${item.title} · ${sourceBits.join(' · ')} · ${backlink}`;
  lines.push(sourceLine, '');

  // 一句话定位
  const lede = enrich?.summary || item.aiSummary || item.description;
  lines.push('## 一句话定位', '');
  lines.push(lede || '（暂无摘要——计划任务或会话中的 AI 雷达 Skill 可补充速览，也可直接写下你的一句话理解）', '');

  // 关键点（带证据角标）
  if (enrich && enrich.keyPoints.length > 0) {
    lines.push('## 关键点', '');
    for (const kp of enrich.keyPoints) {
      const chips = kp.evidence.length > 0 ? ` ${kp.evidence.map((e) => `[${e}]`).join('')}` : '';
      lines.push(`- ${kp.text}${chips}`);
    }
    lines.push('');
  }

  if (enrich?.whyImportant) {
    lines.push('## 为何重要', '', enrich.whyImportant, '');
  }

  if (enrich && enrich.risks.length > 0) {
    lines.push('## 风险与限制', '');
    for (const r of enrich.risks) lines.push(`- ⚠ ${r}`);
    lines.push('');
  }

  // 原始证据清单（可溯源）
  if (evidence.length > 0) {
    lines.push('## 原始证据', '');
    for (const e of evidence) {
      const label = e.url ? `[${e.kind}](${e.url})` : e.kind;
      lines.push(`- **${e.id}** ${label}`);
    }
    lines.push('');
  }

  // 留白：沉淀的意义是写下自己的思考
  lines.push('## 我的想法', '', '');

  // AI 标签转 #标签（笔记本标签云可过滤；跳过含空格的标签避免破坏语法）
  const tags = (item.aiTags ?? []).filter((t) => /^\S+$/.test(t));
  if (tags.length > 0) {
    lines.push('---', '', tags.map((t) => `#${t}`).join(' '), '');
  }

  return lines.join('\n');
}
