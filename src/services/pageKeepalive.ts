/**
 * NEXT-004 / DEC-047：重型工作区受控保活。
 * 宿主基线暖页签 P95=519ms，超过 PRD ≤150ms；只对笔记本/工作室做最多 2 个 LRU 保活。
 * 轻量页仍卸载。曾试过单独保活会话页，三页同挂会使 MutationObserver settle 被隐藏 DOM 拉到数秒，已撤回。
 * 保活页必须同时 inert、退出焦序、暂停快捷键，并停泊原生 WebView。
 */

export const HEAVY_PAGES = ['notes', 'ai-studio'] as const;
export const MAX_KEPT_HEAVY_PAGES = 2;

export type HeavyPageId = (typeof HEAVY_PAGES)[number];

export function isHeavyPage(page: string): page is HeavyPageId {
  return (HEAVY_PAGES as readonly string[]).includes(page);
}

/** 最近访问的重型页在前；非重型页不进入列表。 */
export function rememberHeavyPage(
  kept: string[],
  activePage: string,
  max = MAX_KEPT_HEAVY_PAGES,
): string[] {
  if (!isHeavyPage(activePage)) return kept.filter((id) => isHeavyPage(id)).slice(0, max);
  return [activePage, ...kept.filter((id) => id !== activePage && isHeavyPage(id))].slice(0, max);
}

/** 当前页 + 仍保活的重型页，当前页始终在前。 */
export function mountedPageIds(activePage: string, keptHeavy: string[]): string[] {
  const ids = [activePage];
  for (const id of keptHeavy) {
    if (!ids.includes(id)) ids.push(id);
  }
  return ids;
}

/** 保活隐藏页订阅 store 时用：隐藏态视为相等，避免后台 DOM 协调拖长当前页 settle。 */
export function freezeWhenInactive<T>(
  active: boolean,
  equalityFn?: (a: T, b: T) => boolean,
): (a: T, b: T) => boolean {
  return (a, b) => !active || (equalityFn ? equalityFn(a, b) : Object.is(a, b));
}
