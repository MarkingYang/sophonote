/** 页面 chunk 的唯一加载表：React.lazy、侧栏意图预取和空闲预取共用，避免路径漂移。 */
export const pageLoaders = {
  discover: () => import('../pages/Discover'),
  conversation: () => import('../pages/Conversation'),
  'scheduled-tasks': () => import('../pages/ScheduledTasks'),
  articles: () => import('../pages/Articles'),
  notes: () => import('../pages/Notes'),
  'ai-studio': () => import('../pages/AIStudio'),
  tasks: () => import('../pages/Tasks'),
  settings: () => import('../pages/Settings'),
} as const;

export type PageId = keyof typeof pageLoaders;

export const itemDetailLoader = () => import('../components/features/ItemDetail');

const loaded = new Map<PageId, Promise<unknown>>();

/** hover/focus 时复用同一个动态 import Promise，加载失败后允许下次重试。 */
export function preloadPage(page: string): Promise<unknown> | null {
  if (!(page in pageLoaders)) return null;
  const id = page as PageId;
  const existing = loaded.get(id);
  if (existing) return existing;
  const pending = pageLoaders[id]().catch((error) => {
    loaded.delete(id);
    throw error;
  });
  loaded.set(id, pending);
  return pending;
}

/**
 * 初始化完成后逐个利用浏览器空闲片段预热页面，避免点击时才解析大型 chunk。
 * 每次只调度一个页面，输入/动画优先；不支持 requestIdleCallback 的 WebView 退回短延时。
 */
export function scheduleIdlePagePreloads(activePage: string): () => void {
  const queue = (Object.keys(pageLoaders) as PageId[]).filter((page) => page !== activePage);
  let cancelled = false;
  let idleId: number | null = null;
  let timerId: number | null = null;

  const runNext = () => {
    if (cancelled) return;
    const page = queue.shift();
    if (!page) return;
    void preloadPage(page)?.catch(() => {});
    schedule();
  };

  const schedule = () => {
    if (cancelled || queue.length === 0) return;
    if ('requestIdleCallback' in window) {
      idleId = window.requestIdleCallback(runNext, { timeout: 1500 });
    } else {
      timerId = globalThis.setTimeout(runNext, 250);
    }
  };

  schedule();
  return () => {
    cancelled = true;
    if (idleId != null && 'cancelIdleCallback' in window) window.cancelIdleCallback(idleId);
    if (timerId != null) window.clearTimeout(timerId);
  };
}
