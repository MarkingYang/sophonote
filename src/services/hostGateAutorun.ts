/**
 * DEV 宿主门禁自动跑（osascript 无辅助访问时的替代路径）。
 * 触发键 `dev.host_gate.autorun=1` 由本机 sqlite 写入；跑完清零并把 JSON 报告写入
 * `dev.host_gate.report`。生产构建 `import.meta.env.DEV === false` 时整段空操作。
 *
 * 覆盖：ISSUE-044 发现打开路径（feed 注入 + getDeepDiveByItem，与 Discover 点击同路径）；
 * NEXT-001 夹具播种 / 四场景 / 清空。ISSUE-042 真实 Chat 改标题仍需窗口点击，本模块不冒充。
 */

import { useAppStore } from '../stores/appStore';
import type { Item } from '../types';
import {
  clearFixtureCorpus,
  isFixtureArticle,
  seedFixtureCorpus,
} from './perfFixture';
import { formatReportMarkdown, runAllScenarios, type PerfReport } from './perfRunner';
import {
  getDeepDiveByItem,
  getDiscoveryFeed,
  getSetting,
  updateSetting,
  type DiscoveryFeedRow,
} from './tauri';

export const HOST_GATE_TRIGGER_KEY = 'dev.host_gate.autorun';
export const HOST_GATE_REPORT_KEY = 'dev.host_gate.report';
export const ISSUE044_NEEDLES = ['WeatherNext', 'introduces M6', 'FID'] as const;

export interface Issue044Probe {
  needle: string;
  id?: string;
  title?: string;
  found: boolean;
  opened: boolean;
  contentChars: number;
  error: string | null;
}

export interface HostGateReport {
  status: 'ok' | 'partial' | 'error' | 'running';
  at: string;
  issue044: Issue044Probe[];
  next001: {
    markdown: string;
    report: PerfReport;
    created: number;
    updated: number;
    cleared: number;
    leftover: number;
  } | null;
  errors: string[];
}

/** 与 Discover.rowToPick 同一套 Item 投影，保证注入后 ItemDetail 能找到条目。 */
export function feedRowToItem(row: DiscoveryFeedRow): Item {
  return {
    id: row.id,
    sourceId: row.sourceId,
    type: (row.type as Item['type']) || 'article',
    title: row.title,
    url: row.url ?? '',
    description: row.description ?? '',
    author: row.author ?? undefined,
    language: row.language ?? undefined,
    stars: row.stars ?? undefined,
    forks: row.forks ?? undefined,
    publishedAt: row.publishedAt ?? row.aiScoredAt,
    fetchedAt: row.fetchedAt ?? row.aiScoredAt,
    status: (row.status as Item['status']) || 'unread',
    aiSummary: row.aiSummary ?? undefined,
    aiTags: row.aiTags ? row.aiTags.split(',').map((t) => t.trim()).filter(Boolean) : undefined,
    contentStatus: (row.contentStatus as Item['contentStatus']) ?? undefined,
    qualityLevel: row.qualityLevel ?? undefined,
  };
}

export function pickFeedRowsByNeedles<T extends { title: string }>(
  rows: T[],
  needles: readonly string[],
): Array<{ needle: string; row: T | null }> {
  return needles.map((needle) => ({
    needle,
    row: rows.find((r) => r.title.includes(needle)) ?? null,
  }));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error(`${label} 超时 ${ms}ms`)), ms);
    promise.then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timer);
        reject(error);
      },
    );
  });
}

async function collectDeepFeed(): Promise<DiscoveryFeedRow[]> {
  const page = await withTimeout(
    getDiscoveryFeed({ minScore: 7, requireDeep: true, limit: 100 }),
    8000,
    'db_discovery_feed',
  );
  return page.rows;
}

export async function probeIssue044(): Promise<Issue044Probe[]> {
  const rows = await collectDeepFeed();
  const picks = pickFeedRowsByNeedles(rows, ISSUE044_NEEDLES);
  const store = useAppStore.getState();
  const results: Issue044Probe[] = [];
  for (const { needle, row } of picks) {
    if (!row) {
      results.push({
        needle,
        found: false,
        opened: false,
        contentChars: 0,
        error: 'feed 未命中该标题',
      });
      continue;
    }
    store.addItems([feedRowToItem(row)]);
    store.setSelectedItemId(row.id);
    let article = null;
    let error: string | null = null;
    try {
      article = await withTimeout(getDeepDiveByItem(row.id), 8000, `getDeepDiveByItem ${row.id}`);
      if (article) store.upsertArticle(article);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
    await sleep(200);
    const cached = useAppStore.getState().articles.find(
      (a) => a.itemId === row.id && a.articleType === 'deep-dive',
    );
    const content = (article?.content || cached?.content || '').trim();
    const opened = useAppStore.getState().items.some((i) => i.id === row.id);
    results.push({
      needle,
      id: row.id,
      title: row.title,
      found: true,
      opened,
      contentChars: content.length,
      error: error ?? (content.length === 0 ? '深度解读正文为空' : null),
    });
  }
  useAppStore.getState().setSelectedItemId(null);
  return results;
}

async function heartbeat(message: string): Promise<void> {
  try {
    await updateSetting('dev.host_gate.heartbeat', `${new Date().toISOString()} ${message}`);
  } catch {
    /* invoke 尚未就绪 */
  }
}

async function runPerfBaseline(
  onPartial?: (next001: NonNullable<HostGateReport['next001']>) => void,
): Promise<NonNullable<HostGateReport['next001']>> {
  const store = useAppStore.getState();
  await heartbeat('seed start');
  const seed = await seedFixtureCorpus(store.articles);
  await heartbeat(`seed done created=${seed.created} updated=${seed.updated}`);
  await store.loadArticles();
  await heartbeat('loadArticles done, runAllScenarios');
  const snapshot = (report: PerfReport) => {
    onPartial?.({
      markdown: formatReportMarkdown(report),
      report,
      created: seed.created,
      updated: seed.updated,
      cleared: 0,
      leftover: 0,
    });
  };
  const report = await runAllScenarios((message) => {
    void heartbeat(message);
  }, snapshot);
  await heartbeat('scenarios done, clearing');
  const remaining = useAppStore.getState().articles;
  let cleared = await clearFixtureCorpus(remaining);
  await store.loadArticles();
  let leftover = useAppStore.getState().articles.filter(isFixtureArticle).length;
  if (leftover > 0) {
    await heartbeat(`retry clear leftover=${leftover}`);
    cleared += await clearFixtureCorpus(useAppStore.getState().articles);
    await store.loadArticles();
    leftover = useAppStore.getState().articles.filter(isFixtureArticle).length;
  }
  return {
    markdown: formatReportMarkdown(report),
    report,
    created: seed.created,
    updated: seed.updated,
    cleared,
    leftover,
  };
}

let inFlight = false;
let inFlightSince = 0;
const IN_FLIGHT_STALE_MS = 200_000;

function isInFlight(): boolean {
  if (!inFlight) return false;
  if (Date.now() - inFlightSince > IN_FLIGHT_STALE_MS) {
    inFlight = false;
    return false;
  }
  return true;
}

export function isHostGateInFlight(): boolean {
  return isInFlight();
}

function isDevHost(): boolean {
  return import.meta.env.DEV === true || import.meta.env.MODE === 'development';
}

async function writeReport(payload: HostGateReport): Promise<void> {
  await updateSetting(HOST_GATE_REPORT_KEY, JSON.stringify(payload));
}

export async function maybeAutorunHostGate(): Promise<void> {
  if (!isDevHost()) return;
  if (isInFlight()) return;
  inFlight = true;
  inFlightSince = Date.now();
  try {
    const initialized = useAppStore.getState().initialized;
    if (!initialized) return;
    let trigger = '';
    try {
      trigger = await getSetting(HOST_GATE_TRIGGER_KEY);
    } catch {
      return;
    }
    if (trigger !== '1') return;
    await heartbeat('trigger accepted');
    await updateSetting(HOST_GATE_TRIGGER_KEY, '0');
    const payload: HostGateReport = {
      status: 'running',
      at: new Date().toISOString(),
      issue044: [],
      next001: null,
      errors: [],
    };
    await writeReport(payload);
    try {
      payload.issue044 = await probeIssue044();
      if (payload.issue044.some((p) => p.error)) payload.status = 'partial';
      await writeReport({ ...payload, status: 'running' });
    } catch (e) {
      payload.status = 'partial';
      payload.errors.push(`ISSUE-044: ${e instanceof Error ? e.message : String(e)}`);
      await writeReport(payload);
    }
    try {
      payload.next001 = await withTimeout(
        runPerfBaseline((partial) => {
          payload.next001 = partial;
          void writeReport({ ...payload, status: 'running' });
        }),
        180_000,
        'NEXT-001 夹具',
      );
      if (payload.next001.leftover > 0) {
        payload.status = 'partial';
        payload.errors.push(`夹具清空后仍剩 ${payload.next001.leftover} 篇（报告已保留）`);
      }
    } catch (e) {
      payload.status = 'partial';
      payload.errors.push(`NEXT-001: ${e instanceof Error ? e.message : String(e)}`);
      try {
        await clearFixtureCorpus(useAppStore.getState().articles);
        await useAppStore.getState().loadArticles();
      } catch {
        /* 清语料失败记入 errors 即可 */
      }
    }
    if (payload.status === 'running') {
      payload.status = payload.errors.length ? 'partial' : 'ok';
    } else if (payload.errors.length && payload.status === 'ok') {
      payload.status = 'partial';
    }
    await writeReport(payload);
  } catch (e) {
    try {
      await writeReport({
        status: 'error',
        at: new Date().toISOString(),
        issue044: [],
        next001: null,
        errors: [e instanceof Error ? e.message : String(e)],
      });
    } catch {
      /* 报告也写失败时只能看前端控制台 */
    }
  } finally {
    inFlight = false;
  }
}

/** DEV 轮询。不在重挂时清 inFlight：否则空闲 tick 会冲掉夹具心跳，挂死的一轮也丢进度。超过 200s 视为过期。 */
export function startHostGateWatcher(): () => void {
  if (!isDevHost()) return () => {};
  let stopped = false;
  const tick = () => {
    if (stopped) return;
    void maybeAutorunHostGate();
  };
  tick();
  const id = window.setInterval(tick, 4000);
  return () => {
    stopped = true;
    window.clearInterval(id);
  };
}
