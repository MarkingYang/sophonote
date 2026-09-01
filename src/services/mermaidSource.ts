const MERMAID_DIRECTIVE = /^(?:flowchart|graph|sequenceDiagram|classDiagram(?:-v2)?|stateDiagram(?:-v2)?|erDiagram|journey|gantt|pie|quadrantChart|requirementDiagram|gitGraph|C4Context|C4Container|C4Component|C4Dynamic|C4Deployment|mindmap|timeline|sankey-beta|xychart-beta|block-beta|packet-beta|kanban|architecture-beta|radar-beta|treemap-beta)\b/i;

const TEXT_LIKE_LANGUAGES = new Set(['', 'text', 'txt', 'plain', 'plaintext']);

/**
 * Return the first Mermaid statement, ignoring BOM/blank lines, Mermaid comments,
 * init directives and optional YAML frontmatter. Keeping this check anchored to the
 * first real statement avoids treating an ordinary text/code block that merely
 * mentions `flowchart` as a diagram.
 */
function firstStatement(source: string): string {
  const lines = source.replace(/\uFEFF/g, '').replace(/\r\n?/g, '\n').split('\n');
  let index = 0;
  while (index < lines.length && !lines[index].trim()) index += 1;

  if (lines[index]?.trim() === '---') {
    index += 1;
    while (index < lines.length && lines[index].trim() !== '---') index += 1;
    if (index < lines.length) index += 1;
  }

  while (index < lines.length) {
    const line = lines[index].trim();
    if (line && !line.startsWith('%%')) return line;
    index += 1;
  }
  return '';
}

/** Conservative content detection for legacy Mermaid blocks fenced as text. */
export function isMermaidSource(source: string): boolean {
  return MERMAID_DIRECTIVE.test(firstStatement(source));
}

export function shouldRenderMermaid(language: string | undefined, source: string): boolean {
  const normalized = (language ?? '').trim().toLowerCase();
  if (normalized === 'mermaid' || normalized === 'mmd') return true;
  return TEXT_LIKE_LANGUAGES.has(normalized) && isMermaidSource(source);
}

export interface FencedCodeInfo {
  language: string;
  body: string;
}

/** Parse the source range returned by react-markdown for one fenced code block. */
export function parseFencedCode(source: string): FencedCodeInfo | null {
  const lines = source.replace(/\r\n?/g, '\n').split('\n');
  const opening = lines[0]?.match(/^\s*(`{3,}|~{3,})[ \t]*([^\s`~]*)?.*$/);
  if (!opening) return null;

  const marker = opening[1];
  const closing = new RegExp(`^\\s*${marker[0]}{${marker.length},}\\s*$`);
  let end = lines.length;
  if (end > 1 && closing.test(lines[end - 1])) end -= 1;

  return {
    language: (opening[2] ?? '').toLowerCase(),
    body: lines.slice(1, end).join('\n'),
  };
}

export function isMermaidFence(source: string): boolean {
  const fence = parseFencedCode(source);
  return fence != null && shouldRenderMermaid(fence.language, fence.body);
}
