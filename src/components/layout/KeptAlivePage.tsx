import { createContext, useCallback, useContext, useEffect, useRef, useSyncExternalStore, type ReactNode } from 'react';
import { useAgentStore } from '../../stores/agentStore';

const PageSurfaceContext = createContext(true);

/** 当前页签是否为交互面。保活但不可见的重型页为 false，须暂停快捷键与原生 WebView。 */
export function usePageSurfaceActive(): boolean {
  return useContext(PageSurfaceContext);
}

/** 隐藏保活页不订阅 AgentStore，避免后台 DOM 协调拖长当前页 MutationObserver settle。 */
export function useSurfaceAgentStore<T>(
  selector: (state: ReturnType<typeof useAgentStore.getState>) => T,
  equalityFn?: (a: T, b: T) => boolean,
): T {
  const active = usePageSurfaceActive();
  const selectorRef = useRef(selector);
  selectorRef.current = selector;
  const eqRef = useRef(equalityFn ?? Object.is);
  eqRef.current = equalityFn ?? Object.is;
  const snapshotRef = useRef<T>(selector(useAgentStore.getState()));

  const subscribe = useCallback((onChange: () => void) => {
    if (!active) return () => {};
    return useAgentStore.subscribe((state) => {
      const next = selectorRef.current(state);
      if (eqRef.current(snapshotRef.current, next)) return;
      snapshotRef.current = next;
      onChange();
    });
  }, [active]);

  const getSnapshot = () => {
    if (active) {
      const next = selectorRef.current(useAgentStore.getState());
      if (!eqRef.current(snapshotRef.current, next)) snapshotRef.current = next;
    }
    return snapshotRef.current;
  };

  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

interface KeptAlivePageProps {
  pageId: string;
  active: boolean;
  children: ReactNode;
}

/**
 * 受控保活容器：隐藏态不用裸 display:none 了事——同时 inert、aria-hidden、退出焦序，
 * 并对恢复态派发 resize，让编辑器/WebView 重算尺寸且不丢 EditorState。
 */
export default function KeptAlivePage({ pageId, active, children }: KeptAlivePageProps) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    if (!active) {
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && root.contains(focused)) focused.blur();
      return;
    }
    requestAnimationFrame(() => window.dispatchEvent(new Event('resize')));
  }, [active]);

  return (
    <PageSurfaceContext.Provider value={active}>
      <div
        ref={rootRef}
        data-page-id={pageId}
        data-page-active={active ? 'true' : 'false'}
        hidden={!active}
        aria-hidden={active ? undefined : true}
        inert={active ? undefined : true}
        className={active ? 'relative flex min-h-0 flex-1 flex-col' : undefined}
      >
        {children}
      </div>
    </PageSurfaceContext.Provider>
  );
}
