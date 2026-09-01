import type {
  HermesCommandInfo,
  HermesReferenceInfo,
  HermesSkillInfo,
} from './tauri';

export type HermesComposerItem =
  | { kind: 'command'; name: string; description: string; group: string }
  | { kind: 'skill'; name: string; description: string; group: 'Skills' }
  | { kind: 'reference'; name: string; description: string; group: '引用' };

export interface ComposerTrigger {
  kind: 'slash' | 'reference';
  query: string;
  start: number;
  end: number;
}

export function detectComposerTrigger(text: string, caret = text.length): ComposerTrigger | null {
  const prefix = text.slice(0, caret);
  const match = prefix.match(/(^|\s)([/@][^\s]*)$/);
  if (!match) return null;
  const token = match[2];
  if (token.startsWith('/') && prefix.trimStart() !== token) return null;
  const start = caret - token.length;
  return {
    kind: token[0] === '/' ? 'slash' : 'reference',
    query: token.slice(1).toLocaleLowerCase(),
    start,
    end: caret,
  };
}

function matches(query: string, ...values: Array<string | undefined>): boolean {
  if (!query) return true;
  return values.some((value) => value?.toLocaleLowerCase().includes(query));
}

export function composerItems(
  trigger: ComposerTrigger | null,
  commands: HermesCommandInfo[],
  skills: HermesSkillInfo[],
  references: HermesReferenceInfo[],
): HermesComposerItem[] {
  if (!trigger) return [];
  if (trigger.kind === 'reference') {
    return references
      .filter((item) => matches(trigger.query, item.text.slice(1), item.display, item.description))
      .map((item) => ({
        kind: 'reference' as const,
        name: item.text,
        description: item.description,
        group: '引用' as const,
      }));
  }
  const commandItems = commands
    .filter((item) => matches(trigger.query, item.name.replace(/^\//, ''), item.description))
    .map((item) => ({
      kind: 'command' as const,
      name: item.name,
      description: item.description,
      group: item.category || 'Commands',
    }));
  const skillItems = skills
    .filter((item) => matches(trigger.query, item.name, item.description))
    .map((item) => ({
      kind: 'skill' as const,
      name: `/${item.name}`,
      description: item.description,
      group: 'Skills' as const,
    }));
  return [...commandItems, ...skillItems].slice(0, 80);
}

export function replaceComposerTrigger(
  text: string,
  trigger: ComposerTrigger,
  replacement: string,
): string {
  return `${text.slice(0, trigger.start)}${replacement}${text.slice(trigger.end)}`;
}

export function parseSkillInvocation(text: string, skills: HermesSkillInfo[]) {
  const match = text.trim().match(/^\/([^\s]+)(?:\s+([\s\S]*))?$/);
  if (!match) return null;
  const skill = skills.find((item) => item.name.toLocaleLowerCase() === match[1].toLocaleLowerCase());
  if (!skill) return null;
  return { skill: skill.name, arg: match[2]?.trim() ?? '' };
}

export function capabilityMatches(query: string, ...values: Array<string | null | undefined>): boolean {
  return matches(query.trim().toLocaleLowerCase(), ...values.filter((value): value is string => value != null));
}

export function isSessionControlCommand(command: string): 'undo' | 'yolo' | null {
  const name = command.trim().split(/\s+/, 1)[0]?.replace(/^\//, '').toLocaleLowerCase();
  if (name === 'undo') return 'undo';
  if (name === 'yolo') return 'yolo';
  return null;
}

export function composerHistoryStep(
  direction: 'up' | 'down',
  history: string[],
  index: number,
  draft: string,
): { index: number; text: string; draft: string } {
  if (history.length === 0) return { index: -1, text: draft, draft };
  if (direction === 'up') {
    const nextIndex = index < 0 ? 0 : Math.min(index + 1, history.length - 1);
    return {
      index: nextIndex,
      text: history[nextIndex] ?? draft,
      draft: index < 0 ? draft : draft,
    };
  }
  if (index < 0) return { index: -1, text: draft, draft };
  const nextIndex = index - 1;
  if (nextIndex < 0) return { index: -1, text: draft, draft };
  return { index: nextIndex, text: history[nextIndex] ?? draft, draft };
}

export function rememberComposerHistory(history: string[], text: string, limit = 50): string[] {
  const value = text.trim();
  if (!value) return history;
  const next = [value, ...history.filter((item) => item !== value)];
  return next.slice(0, limit);
}

export function canUseComposerHistory(
  text: string,
  caret: number,
  hasPalette: boolean,
): boolean {
  if (hasPalette) return false;
  if (text.length === 0) return true;
  return caret === 0 || caret === text.length;
}

export function findTimelineMatches(
  items: Array<{ key: string; text: string }>,
  query: string,
): number[] {
  const needle = query.trim().toLocaleLowerCase();
  if (!needle) return [];
  return items.flatMap((item, index) => (
    item.text.toLocaleLowerCase().includes(needle) ? [index] : []
  ));
}

const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp']);

export function droppedPathName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

/** Finder / Tauri 原生拖放只有路径。目录仍走附件选择器的 folder 入口。 */
export function droppedPathKind(path: string): 'image' | 'file' {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  return IMAGE_EXTS.has(ext) ? 'image' : 'file';
}

export function physicalPointInCssRect(
  position: { x: number; y: number },
  rect: { left: number; top: number; right: number; bottom: number },
  devicePixelRatio = 1,
): boolean {
  const scale = devicePixelRatio > 0 ? devicePixelRatio : 1;
  const x = position.x / scale;
  const y = position.y / scale;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}
