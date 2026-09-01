import { describe, expect, it } from 'vitest';
import { splitStreamingMarkdown } from '../streamingMarkdown';

describe('流式 Markdown 稳定块', () => {
  it('按围栏外空行提交稳定块并逐字保持正文', () => {
    const source = '# 标题\n\n第一段。\n\n仍在生成';
    const parts = splitStreamingMarkdown(source);
    expect(parts.stableBlocks).toEqual(['# 标题\n\n', '第一段。\n\n']);
    expect(parts.tail).toBe('仍在生成');
    expect(parts.stableBlocks.join('') + parts.tail).toBe(source);
  });

  it('代码围栏内部的空行不形成稳定边界', () => {
    const source = '```ts\nconst value = 1;\n\nreturn value;\n```\n\n结尾';
    const parts = splitStreamingMarkdown(source);
    expect(parts.stableBlocks).toEqual(['```ts\nconst value = 1;\n\nreturn value;\n```\n\n']);
    expect(parts.tail).toBe('结尾');
  });

  it('未闭合围栏全部保留为活跃尾部', () => {
    const source = '```md\n# 尚未完成\n\n正文';
    expect(splitStreamingMarkdown(source)).toEqual({ stableBlocks: [], tail: source });
  });
});
