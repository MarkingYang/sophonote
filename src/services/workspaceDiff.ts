export type WorkspaceDiffLineKind = 'context' | 'addition' | 'deletion' | 'meta';

export interface WorkspaceDiffLine {
  kind: WorkspaceDiffLineKind;
  content: string;
  oldLine?: number;
  newLine?: number;
}

export interface WorkspaceDiffHunk {
  id: string;
  header: string;
  lines: WorkspaceDiffLine[];
  additions: number;
  deletions: number;
}

export type WorkspaceDiffChunk =
  | { kind: 'meta'; id: string; lines: string[] }
  | { kind: 'hunk'; id: string; hunk: WorkspaceDiffHunk };

export interface WorkspaceDiffDocument {
  raw: string;
  chunks: WorkspaceDiffChunk[];
  hunks: WorkspaceDiffHunk[];
}

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

export function parseWorkspaceDiff(raw: string): WorkspaceDiffDocument {
  const chunks: WorkspaceDiffChunk[] = [];
  const hunks: WorkspaceDiffHunk[] = [];
  let meta: string[] = [];
  let current: WorkspaceDiffHunk | null = null;
  let oldLine = 0;
  let newLine = 0;

  const flushMeta = () => {
    if (meta.length === 0) return;
    const id = `meta-${chunks.length}`;
    chunks.push({ kind: 'meta', id, lines: meta });
    meta = [];
  };

  const flushHunk = () => {
    if (!current) return;
    chunks.push({ kind: 'hunk', id: current.id, hunk: current });
    hunks.push(current);
    current = null;
  };

  raw.split('\n').forEach((line) => {
    const match = HUNK_HEADER.exec(line);
    if (match) {
      flushHunk();
      flushMeta();
      oldLine = Number(match[1]);
      newLine = Number(match[3]);
      current = {
        id: `hunk-${hunks.length}`,
        header: line,
        lines: [],
        additions: 0,
        deletions: 0,
      };
      return;
    }

    if (current && /^(?: |\+|-|\\)/.test(line)) {
      if (line.startsWith('+')) {
        current.lines.push({ kind: 'addition', content: line.slice(1), newLine });
        current.additions += 1;
        newLine += 1;
      } else if (line.startsWith('-')) {
        current.lines.push({ kind: 'deletion', content: line.slice(1), oldLine });
        current.deletions += 1;
        oldLine += 1;
      } else if (line.startsWith(' ')) {
        current.lines.push({ kind: 'context', content: line.slice(1), oldLine, newLine });
        oldLine += 1;
        newLine += 1;
      } else {
        current.lines.push({ kind: 'meta', content: line });
      }
      return;
    }

    flushHunk();
    meta.push(line);
  });

  flushHunk();
  flushMeta();
  return { raw, chunks, hunks };
}
