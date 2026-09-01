import { describe, expect, it, vi } from 'vitest';
import { copyMarkdownSource } from '../../../services/clipboard';

describe('Project Chat Markdown copy', () => {
  it('copies the exact source Markdown instead of rendered text', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    const markdown = '# 标题\n\n- **重点**\n\n```ts\nconst ready = true;\n```';
    await expect(copyMarkdownSource(markdown)).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith(markdown);
  });
});
