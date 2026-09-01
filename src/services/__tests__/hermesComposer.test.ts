import { describe, expect, it } from 'vitest';
import {
  capabilityMatches,
  composerHistoryStep,
  composerItems,
  detectComposerTrigger,
  droppedPathKind,
  droppedPathName,
  findTimelineMatches,
  isSessionControlCommand,
  parseSkillInvocation,
  physicalPointInCssRect,
  rememberComposerHistory,
  replaceComposerTrigger,
} from '../hermesComposer';

describe('Hermes composer protocol helpers', () => {
  it('detects a leading slash and an inline reference token', () => {
    expect(detectComposerTrigger('/mark')).toMatchObject({ kind: 'slash', query: 'mark' });
    expect(detectComposerTrigger('阅读 @fi')).toMatchObject({ kind: 'reference', query: 'fi' });
    expect(detectComposerTrigger('正文 /mark')).toBeNull();
  });

  it('groups Runtime commands, skills and references without inventing tools', () => {
    const slash = composerItems(
      detectComposerTrigger('/so'),
      [{ name: '/sort', description: 'command', category: 'Session' }],
      [{ name: 'sophonote-writing', description: 'Markdown', origin: null }],
      [],
    );
    expect(slash.map((item) => item.kind)).toEqual(['command', 'skill']);
    const refs = composerItems(
      detectComposerTrigger('@file'),
      [],
      [],
      [{ text: '@file:', display: '@file:', description: 'attach file' }],
    );
    expect(refs[0]?.name).toBe('@file:');
  });

  it('parses direct skill invocation and preserves its argument', () => {
    expect(parseSkillInvocation('/proofread  校对这一段', [
      { name: 'proofread', description: '', origin: null },
    ])).toEqual({ skill: 'proofread', arg: '校对这一段' });
    const trigger = detectComposerTrigger('读取 @fi')!;
    expect(replaceComposerTrigger('读取 @fi', trigger, '@file: ')).toBe('读取 @file: ');
  });

  it('searches capability names, descriptions and nested labels case-insensitively', () => {
    expect(capabilityMatches('browser', 'Browser tools', 'navigate')).toBe(true);
    expect(capabilityMatches('github', 'filesystem', 'read_file')).toBe(false);
  });

  it('routes Desktop session controls without treating them as ordinary prompts', () => {
    expect(isSessionControlCommand('/undo 2')).toBe('undo');
    expect(isSessionControlCommand('/YOLO')).toBe('yolo');
    expect(isSessionControlCommand('/model')).toBeNull();
  });

  it('walks composer history newest-first and restores the live draft', () => {
    const history = rememberComposerHistory(rememberComposerHistory([], '第一问'), '第二问');
    expect(history).toEqual(['第二问', '第一问']);
    const up = composerHistoryStep('up', history, -1, '草稿');
    expect(up).toMatchObject({ index: 0, text: '第二问', draft: '草稿' });
    const down = composerHistoryStep('down', history, 0, up.draft);
    expect(down).toMatchObject({ index: -1, text: '草稿' });
  });

  it('finds timeline matches without inventing empty-query hits', () => {
    expect(findTimelineMatches([
      { key: 'a', text: '请撤回上一轮' },
      { key: 'b', text: '无关' },
    ], '撤回')).toEqual([0]);
    expect(findTimelineMatches([{ key: 'a', text: 'hello' }], '  ')).toEqual([]);
  });

  it('classifies native drop paths by extension without inventing folder kinds', () => {
    expect(droppedPathKind('/tmp/example/index.html')).toBe('file');
    expect(droppedPathKind('/tmp/photo.PNG')).toBe('image');
    expect(droppedPathName('/tmp/example/index.html')).toBe('index.html');
  });

  it('maps physical drop coordinates onto the chat panel CSS box', () => {
    const rect = { left: 400, top: 0, right: 800, bottom: 600 };
    expect(physicalPointInCssRect({ x: 500, y: 100 }, rect, 1)).toBe(true);
    expect(physicalPointInCssRect({ x: 100, y: 100 }, rect, 1)).toBe(false);
    expect(physicalPointInCssRect({ x: 1000, y: 200 }, rect, 2)).toBe(true);
  });
});
