import { describe, expect, it } from 'vitest';
import { parseWorkspaceDiff } from '../workspaceDiff';

describe('parseWorkspaceDiff', () => {
  it('parses hunk line numbers and change counts', () => {
    const parsed = parseWorkspaceDiff([
      'diff --git a/app.ts b/app.ts',
      '--- a/app.ts',
      '+++ b/app.ts',
      '@@ -10,3 +10,4 @@',
      ' keep',
      '-old',
      '+new',
      '+extra',
      ' tail',
    ].join('\n'));

    expect(parsed.hunks).toHaveLength(1);
    expect(parsed.hunks[0]).toMatchObject({ additions: 2, deletions: 1 });
    expect(parsed.hunks[0].lines).toMatchObject([
      { kind: 'context', oldLine: 10, newLine: 10 },
      { kind: 'deletion', oldLine: 11 },
      { kind: 'addition', newLine: 11 },
      { kind: 'addition', newLine: 12 },
      { kind: 'context', oldLine: 12, newLine: 13 },
    ]);
  });

  it('keeps staged and working-tree metadata between hunk groups', () => {
    const parsed = parseWorkspaceDiff([
      '# Staged changes',
      '@@ -1 +1 @@',
      '-a',
      '+b',
      '# Working tree changes',
      '@@ -3 +3 @@',
      '-c',
      '+d',
    ].join('\n'));

    expect(parsed.hunks).toHaveLength(2);
    expect(parsed.chunks.map((chunk) => chunk.kind)).toEqual(['meta', 'hunk', 'meta', 'hunk']);
  });

  it('falls back to metadata for untracked file content', () => {
    const parsed = parseWorkspaceDiff('# Untracked file\n\nplain text');
    expect(parsed.hunks).toHaveLength(0);
    expect(parsed.chunks).toHaveLength(1);
  });
});
