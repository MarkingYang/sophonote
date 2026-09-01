export interface StreamingMarkdownParts {
  stableBlocks: string[];
  tail: string;
}

/**
 * 把只追加的流式 Markdown 拆成已闭合块和仍在增长的尾部。
 * 空行只在代码围栏之外构成提交边界；因此 fenced code 内的空行不会被拆开。
 * 返回文本拼接后必须与输入逐字一致。
 */
export function splitStreamingMarkdown(content: string): StreamingMarkdownParts {
  if (!content) return { stableBlocks: [], tail: '' };

  const stableBlocks: string[] = [];
  let blockStart = 0;
  let cursor = 0;
  let fence: { marker: '`' | '~'; length: number } | null = null;

  while (cursor < content.length) {
    const newline = content.indexOf('\n', cursor);
    const lineEnd = newline === -1 ? content.length : newline + 1;
    const line = content.slice(cursor, newline === -1 ? content.length : newline);
    const trimmed = line.trimStart();
    const fenceMatch = trimmed.match(/^(`{3,}|~{3,})/);

    if (fenceMatch) {
      const marker = fenceMatch[1][0] as '`' | '~';
      if (!fence) {
        fence = { marker, length: fenceMatch[1].length };
      } else if (marker === fence.marker && fenceMatch[1].length >= fence.length) {
        fence = null;
      }
    }

    if (!fence && line.trim().length === 0 && newline !== -1) {
      stableBlocks.push(content.slice(blockStart, lineEnd));
      blockStart = lineEnd;
    }

    cursor = lineEnd;
  }

  return {
    stableBlocks: stableBlocks.filter(Boolean),
    tail: content.slice(blockStart),
  };
}
