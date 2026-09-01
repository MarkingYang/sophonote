import outlineRaw from '../../scripts/walkthrough-samples/sample-outline-long.md?raw';
import tasksRaw from '../../scripts/walkthrough-samples/sample-tasks.md?raw';
import linksFromRaw from '../../scripts/walkthrough-samples/sample-links-from.md?raw';
import linksTargetRaw from '../../scripts/walkthrough-samples/sample-links-target.md?raw';
import templateMeetingRaw from '../../scripts/walkthrough-samples/sample-template-meeting.md?raw';
import searchRaw from '../../scripts/walkthrough-samples/sample-search-extra.md?raw';
import type { Article } from '../types';

export interface NoteExample {
  id: string;
  title: string;
  summary: string;
  features: string[];
  content: string;
}

export function stripExampleFrontmatter(markdown: string): string {
  return markdown.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n?/, '').trimStart();
}

export const noteExamples: NoteExample[] = [
  {
    id: 'sample-outline-long',
    title: '样例·深度学习研读',
    summary: '用一篇结构化长文体验 Markdown、大纲导航、段落嵌入与跨笔记阅读。',
    features: ['Markdown', '大纲', '嵌入'],
    content: stripExampleFrontmatter(outlineRaw),
  },
  {
    id: 'sample-tasks',
    title: '样例·任务清单',
    summary: '把笔记中的待办汇总到任务页，勾选后再写回原始 Markdown。',
    features: ['Tasks', '回链'],
    content: stripExampleFrontmatter(tasksRaw),
  },
  {
    id: 'sample-links-from',
    title: '样例·双链引用页',
    summary: '体验标准双链、别名、标题链接、悬停预览和改名同步。',
    features: ['双链', '反链', '悬停预览'],
    content: stripExampleFrontmatter(linksFromRaw),
  },
  {
    id: 'sample-links-target',
    title: '样例·被引用页',
    summary: '作为双链和段落嵌入的目标，展示反链与源文档写回。',
    features: ['反链', '嵌入写回'],
    content: stripExampleFrontmatter(linksTargetRaw),
  },
  {
    id: 'sample-template-meeting',
    title: '样例模板·会议纪要',
    summary: '普通笔记加上 #template 即成为模板，并支持日期、时间和标题变量。',
    features: ['模板', '变量'],
    content: stripExampleFrontmatter(templateMeetingRaw),
  },
  {
    id: 'sample-search-extra',
    title: '样例·搜索陪练',
    summary: '用重复关键词体验全文搜索、命中上下文和高亮。',
    features: ['搜索', '高亮'],
    content: stripExampleFrontmatter(searchRaw),
  },
];

export function missingNoteExamples(existingTitles: Iterable<string>): NoteExample[] {
  const titles = new Set(existingTitles);
  return noteExamples.filter((example) => !titles.has(example.title));
}

export function noteExampleArticles(
  existingTitles: Iterable<string>,
  now = new Date(),
  idFactory: () => string = () => crypto.randomUUID(),
): Article[] {
  return missingNoteExamples(existingTitles).map((example) => ({
    id: idFactory(),
    title: example.title,
    content: example.content,
    articleType: 'manual',
    edited: false,
    createdAt: now.toISOString(),
    blocksJson: null,
  }));
}
