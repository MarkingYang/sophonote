import { createElement, lazy, Suspense, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Loader2 } from 'lucide-react';
import { useAppStore } from './stores/appStore';
import {
  hermesCronJobs,
  hermesCronRunResult,
  hermesCronRuns,
  sendNotification,
} from './services/tauri';
import Layout from './components/layout/Layout';
import GlobalSearch from './components/features/GlobalSearch';
import PerfFixturePanel from './components/features/PerfFixturePanel';
import { itemDetailLoader, pageLoaders, scheduleIdlePagePreloads } from './services/pagePreload';
import { startHostGateWatcher, isHostGateInFlight } from './services/hostGateAutorun';
import { mountedPageIds, rememberHeavyPage } from './services/pageKeepalive';
import KeptAlivePage from './components/layout/KeptAlivePage';

// 发布性能债治理：全部页面与条目详情面板改为 React.lazy 按需加载。
// 文档编辑/渲染重栈（Milkdown·ProseMirror·CodeMirror·KaTeX·highlight.js·react-markdown，
// 压缩前约 2.5MB）由此移出启动 chunk，仅在进入相应页面/打开详情时按 Tauri 本地磁盘毫秒级载入。
const Discover = lazy(pageLoaders.discover);
const Conversation = lazy(pageLoaders.conversation);
const ScheduledTasks = lazy(pageLoaders['scheduled-tasks']);
const Articles = lazy(pageLoaders.articles);
const Notes = lazy(pageLoaders.notes);
const AIStudio = lazy(pageLoaders['ai-studio']);
const Tasks = lazy(pageLoaders.tasks);
const Settings = lazy(pageLoaders.settings);
const ItemDetail = lazy(itemDetailLoader);

const pages: Record<string, React.ComponentType> = {
  discover: Discover,
  conversation: Conversation,
  'scheduled-tasks': ScheduledTasks,
  articles: Articles,
  notes: Notes,
  'ai-studio': AIStudio,
  tasks: Tasks,
  settings: Settings,
};

const DISCOVERY_SKILL = 'sophonote-ai-radar';
// 旧计划任务可能仍挂在被融合替代的 Skill 名上；Rust reconcile 会自动改指，
// 改指前的运行结果仍要被「发现」横幅识别。
const LEGACY_DISCOVERY_SKILL = 'sophonote-discovery-subscriptions';
const DISCOVERY_COMPLETED_PATTERN = /发现已更新[：:]\s*新增\s*(\d+)\s*条高质量内容[^。\n]*[。]?/;

function PageFallback() {
  return (
    <div className="flex h-full w-full items-center justify-center text-[var(--text-tertiary)]">
      <Loader2 size={18} className="animate-spin" />
    </div>
  );
}

/** 仅用于让 DEV 门禁 effect 在 HMR 后稳定重挂，避免半更新引用已删常量。 */
const HOST_GATE_WATCH_GEN = 12;

function App() {
  const activePage = useAppStore((state) => state.activePage);
  const initialize = useAppStore((state) => state.initialize);
  const initialized = useAppStore((state) => state.initialized);
  const theme = useAppStore((state) => state.settings.theme);
  const [keptHeavyPages, setKeptHeavyPages] = useState<string[]>([]);
  const mountedPages = mountedPageIds(activePage, keptHeavyPages);

  useEffect(() => {
    setKeptHeavyPages((prev) => rememberHeavyPage(prev, activePage));
  }, [activePage]);

  useEffect(() => {
    if (!initialized) {
      initialize();
    }
  }, [initialize, initialized]);

  // 首屏初始化完成后再逐页空闲预热；侧栏 hover/focus 还会提前触发目标页加载。
  useEffect(() => {
    if (!initialized) return;
    return scheduleIdlePagePreloads(activePage);
  }, [activePage, initialized]);

  // DEV 宿主门禁：sqlite 触发键 → 真实 WKWebView 跑 ISSUE-044 / NEXT-001，结果写回 settings。
  // 必须等 initialized：首 tick 若 invoke 未就绪会静默失败，后台 WKWebView 又会冻结
  // setInterval，触发键会一直停在 1。
  useEffect(() => {
    if (!initialized) return;
    return startHostGateWatcher();
  }, [HOST_GATE_WATCH_GEN, initialized]);

  // Theme watcher: applies/removes 'dark' class on <html> based on settings
  useEffect(() => {
    const root = document.documentElement;
    if (theme === 'dark') {
      root.classList.add('dark');
    } else if (theme === 'light') {
      root.classList.remove('dark');
    } else {
      // system: match OS preference
      const mql = window.matchMedia('(prefers-color-scheme: dark)');
      const applySystem = () => {
        if (mql.matches) {
          root.classList.add('dark');
        } else {
          root.classList.remove('dark');
        }
      };
      applySystem();
      mql.addEventListener('change', applySystem);
      return () => mql.removeEventListener('change', applySystem);
    }
  }, [theme]);

  // 监听 Rust 调度器事件：定时抓取完成 / 系统通知。
  useEffect(() => {
    const unlisteners: Array<Promise<() => void>> = [];

    unlisteners.push(
      listen<{ title: string; body: string }>('sophonote:notify', (event) => {
        sendNotification(event.payload.title, event.payload.body);
      })
    );

    unlisteners.push(
      listen<{ sourceId?: string }>('sophonote:fetch-completed', () => {
        // 原始抓取只是内部管线步骤，不打扰用户；最终高质量发现由 Hermes Cron 完成后通知。
        useAppStore.getState().refreshStats();
        // 刷新源健康状态（Sidebar 联通圆点：绿=成功 / 红=失败）
        useAppStore.getState().loadSources();
      })
    );

    unlisteners.push(
      listen<{ count: number }>('sophonote:articles-updated', () => {
        useAppStore.getState().loadArticles();
      })
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // Hermes Cron 是发现任务的唯一调度真相源。只在本轮从 running 进入 completed，
  // 且 Skill 明确报告产生高质量内容时发通知；抓取数、候选数与静默空结果均不通知。
  useEffect(() => {
    if (!initialized) return;

    const seenTerminalRuns = new Set<string>();
    let initialSnapshot = true;
    let polling = false;
    let cancelled = false;

    const pollDiscoveryRuns = async () => {
      if (polling || cancelled || isHostGateInFlight()) return;
      polling = true;
      let snapshotLoaded = false;
      try {
        const jobs = (await hermesCronJobs()).filter(
          (job) =>
            job.skills.includes(DISCOVERY_SKILL) ||
            job.skills.includes(LEGACY_DISCOVERY_SKILL),
        );

        for (const job of jobs) {
          try {
            const runs = (await hermesCronRuns(job)).slice(0, 5);
            for (const run of runs) {
              if (run.status !== 'completed' && run.status !== 'error') continue;
              if (seenTerminalRuns.has(run.sessionId)) continue;

              // 首次快照只建立基线，避免应用重启后重放历史通知。
              if (!initialSnapshot && run.status === 'completed') {
                const result = await hermesCronRunResult(run);
                const completed = result.markdown.match(DISCOVERY_COMPLETED_PATTERN);
                if (completed) {
                  const count = Number(completed[1]);
                  const store = useAppStore.getState();
                  if (store.settings.notificationEnabled) {
                    await sendNotification(
                      'SophoNote 发现已更新',
                      `新增 ${count} 条高质量内容，可前往「发现」查看。`,
                    );
                  }
                  await Promise.allSettled([
                    store.loadItems(),
                    store.loadArticles(),
                    store.refreshStats(),
                  ]);
                }
              }

              seenTerminalRuns.add(run.sessionId);
            }
          } catch (error) {
            console.warn(`Failed to inspect Hermes Cron job ${job.id}:`, error);
          }
        }
        snapshotLoaded = true;
      } catch (error) {
        console.warn('Failed to poll Hermes discovery runs:', error);
      } finally {
        // 首次查询失败时仍保留“基线”状态，避免下次恢复后把历史完成记录误通知。
        if (snapshotLoaded) initialSnapshot = false;
        polling = false;
      }
    };

    void pollDiscoveryRuns();
    const timer = window.setInterval(() => void pollDiscoveryRuns(), 15_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [initialized]);

  return (
    <Layout>
      <div className="relative flex min-h-0 flex-1 flex-col">
        {mountedPages.map((id) => {
          const PageComponent = pages[id] || Discover;
          return (
            <KeptAlivePage key={id} pageId={id} active={id === activePage}>
              <Suspense fallback={id === activePage ? <PageFallback /> : null}>
                {createElement(PageComponent)}
              </Suspense>
            </KeptAlivePage>
          );
        })}
      </div>
      <Suspense fallback={null}>
        <ItemDetail />
      </Suspense>
      <GlobalSearch />
      {/* NEXT-001 性能夹具：App 级浮层，跨页签常驻（内部按 store 开关显隐） */}
      <PerfFixturePanel />
    </Layout>
  );
}

export default App;
