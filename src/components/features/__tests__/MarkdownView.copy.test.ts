import { describe, expect, it } from 'vitest';
import { mermaidVisualConfig, specialMarkdownBlockLabel } from '../MarkdownView';

describe('MarkdownView special block copy labels', () => {
  it('distinguishes Mermaid fences from regular code fences', () => {
    expect(specialMarkdownBlockLabel('```mermaid\nflowchart LR\nA --> B\n```')).toBe('复制 Mermaid');
    expect(specialMarkdownBlockLabel('~~~MERMAID\nsequenceDiagram\n~~~')).toBe('复制 Mermaid');
    expect(specialMarkdownBlockLabel('```text\nflowchart TB\nA --> B\n```')).toBe('复制 Mermaid');
    expect(specialMarkdownBlockLabel('```text\nC4Context\nPerson(user, "User")\n```')).toBe('复制 Mermaid');
    expect(specialMarkdownBlockLabel('```text\n普通说明\nflowchart TB\n```')).toBe('复制代码');
    expect(specialMarkdownBlockLabel('```ts\nconst ready = true\n```')).toBe('复制代码');
  });

  it('uses the same high-contrast neo visual system in light and dark mode', () => {
    const light = mermaidVisualConfig(false);
    const dark = mermaidVisualConfig(true);

    expect(light).toMatchObject({
      theme: 'base',
      look: 'neo',
      flowchart: { nodeSpacing: 48, rankSpacing: 58, wrappingWidth: 220 },
      sequence: { actorMargin: 64, mirrorActors: false },
    });
    // 失败不得在 body 残留 mermaid 错误 SVG（残留会撑高文档致整窗滚动，2026-08-09 线上症状）
    expect(light.suppressErrorRendering).toBe(true);
    expect(dark.suppressErrorRendering).toBe(true);
    expect(light.themeVariables.primaryTextColor).toBe('#172033');
    expect(dark.themeVariables.primaryTextColor).toBe('#eef4ff');
    expect(dark.themeVariables.lineColor).toBe('#8da4cf');
  });
});
