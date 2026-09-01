/**
 * NEXT-001 性能夹具场景运行器：在真实宿主内脚本化执行 PRD §3.1/§16 的
 * 固定场景，产出 P50/P95 报告（设备/构建/采样方法一并记录，直接可贴台账）。
 *
 * 场景口径（与台账 NEXT-001 一致）：
 * - nav_switch    冷/热页签：一级导航逐页切换，冷=本会话首次挂载，热=二次挂载；
 * - doc_ab_switch 快速 A/B：5KB↔50KB 文档连续切换 20 次（ISSUE-001 的 20 次口径）；
 * - typing_50k    输入延迟：50KB 文档编辑态注入文本块，测派发→上屏（keydown-to-paint 代理）；
 * - list_200      200 篇列表：笔记本列表挂载稳定耗时 + 脚本化滚动 FPS。
 *
 * 「可交互」判据统一为 settle：document.body MutationObserver 静默 150ms（2.5s 兜底），
 * 与首帧（双 rAF）分开记录。方法论随报告输出，保证改前/改后可复算。
 */

import { useAppStore } from '../stores/appStore';
import { perfSamples } from './notePerf';
import { FIXTURE_50K_ID, FIXTURE_5K_ID, isFixtureArticle, summarizeSamples, type SampleStats } from './perfFixture';
import { getPerfProbeTargets } from './perfProbeRegistry';
import { getAppVersion } from './tauri';

// ==================== 报告类型 ====================

export interface PerfEnv {
  appVersion: string;
  ua: string;
  platform: string;
  cores: number;
  deviceMemoryGB: number | null;
}

export interface ScenarioResult {
  id: string;
  label: string;
  samples: number[];
  stats: SampleStats | null;
  meta: Record<string, number | string | null>;
  error: string | null;
}

export interface PerfReport {
  schema: 1;
  at: string;
  env: PerfEnv;
  methodology: string[];
  scenarios: ScenarioResult[];
}

export const PERF_METHODOLOGY = [
  '可交互 = document.body MutationObserver 静默 150ms（2.5s 超时兜底）；首帧 = 状态变更后双 rAF',
  '冷页签 = 本会话该页首次挂载（chunk 可能已被空闲预取预热）；热页签 = 二次及以上挂载',
  'A/B 切换经 requestOpenArticle（现网 ⌘K 同路径），20 次交替 5KB↔50KB',
  '输入延迟 = ProseMirror insertText 派发→双 rAF 上屏，30 轮 × 16 字符，含 Long Task 与 FPS 采样',
  '语料 = perfFixture 确定性 200 篇（含固定 5KB/50KB），改前/改后必须同一份语料',
];

// ==================== 基础工具 ====================

function yieldMacrotask(): Promise<void> {
  return new Promise((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      resolve();
    };
    try {
      const ch = new MessageChannel();
      ch.port1.onmessage = finish;
      ch.port2.postMessage(null);
    } catch {
      /* 无 MessageChannel 时走 timeout/rAF/微任务 */
    }
    setTimeout(finish, 0);
    requestAnimationFrame(finish);
    queueMicrotask(finish);
  });
}

/** 墙钟等待：MessageChannel + rAF + timeout + 微任务，避免 WKWebView 冻住单一调度源。 */
export async function waitMs(ms: number): Promise<void> {
  const t0 = performance.now();
  while (performance.now() - t0 < ms) {
    await yieldMacrotask();
  }
}

const sleep = (ms: number) => waitMs(ms);

export function nextPaint(): Promise<void> {
  return Promise.race([
    new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    }),
    waitMs(32),
  ]).then(() => undefined);
}

/** DOM 静默探测：quietMs 内无 mutation 视为稳定；返回等待耗时（timeoutMs 兜底） */
export async function settle(quietMs = 150, timeoutMs = 2500): Promise<number> {
  const t0 = performance.now();
  let lastMutation = t0;
  let observer: MutationObserver | null = null;
  try {
    observer = new MutationObserver(() => {
      lastMutation = performance.now();
    });
    observer.observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
  } catch {
    await waitMs(quietMs);
    return Math.round(performance.now() - t0);
  }
  try {
    while (performance.now() - t0 < timeoutMs) {
      if (performance.now() - lastMutation >= quietMs) break;
      await yieldMacrotask();
    }
  } finally {
    observer.disconnect();
  }
  return Math.round(performance.now() - t0);
}

export interface LongTaskSummary {
  supported: boolean;
  count: number;
  totalMs: number;
  maxMs: number;
}

export function startLongTaskCollector(): { stop: () => void; summary: () => LongTaskSummary } {
  let count = 0;
  let totalMs = 0;
  let maxMs = 0;
  let supported = true;
  let observer: PerformanceObserver | null = null;
  try {
    observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        count++;
        totalMs += entry.duration;
        if (entry.duration > maxMs) maxMs = entry.duration;
      }
    });
    observer.observe({ entryTypes: ['longtask'] });
  } catch {
    supported = false;
  }
  return {
    stop: () => observer?.disconnect(),
    summary: () => ({
      supported,
      count,
      totalMs: Math.round(totalMs),
      maxMs: Math.round(maxMs),
    }),
  };
}

/** FPS 采样（复用 notePerf 的 1s 窗口 rAF 计数）：返回 min/avg */
export function startFpsCollector(): { stop: () => void; summary: () => { min: number | null; avg: number | null } } {
  const readings: number[] = [];
  let raf = 0;
  let frames = 0;
  let windowStart = performance.now();
  let stopped = false;
  const loop = () => {
    if (stopped) return;
    frames++;
    const now = performance.now();
    if (now - windowStart >= 500) {
      readings.push(Math.round((frames * 1000) / (now - windowStart)));
      frames = 0;
      windowStart = now;
    }
    raf = requestAnimationFrame(loop);
  };
  raf = requestAnimationFrame(loop);
  return {
    stop: () => {
      stopped = true;
      cancelAnimationFrame(raf);
    },
    summary: () => {
      if (readings.length === 0) return { min: null, avg: null };
      const sum = readings.reduce((a, b) => a + b, 0);
      return { min: Math.min(...readings), avg: Math.round(sum / readings.length) };
    },
  };
}

/** 单场景墙钟上限；超时后记录失败并继续，避免 typing 等场景拖死整份门禁。 */
export async function raceTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  let settled = false;
  const timeout = waitMs(ms).then(() => {
    if (!settled) throw new Error(`${label} 超时 ${ms}ms`);
    return undefined as never;
  });
  try {
    const value = await Promise.race([promise, timeout]);
    settled = true;
    return value as T;
  } catch (error) {
    settled = true;
    throw error;
  }
}

// ==================== 环境采集 ====================

export interface NavigatorLike {
  userAgent: string;
  platform?: string;
  hardwareConcurrency?: number;
  deviceMemory?: number;
}

export function collectEnv(nav: NavigatorLike, appVersion: string): PerfEnv {
  return {
    appVersion,
    ua: nav.userAgent,
    platform: nav.platform ?? 'unknown',
    cores: nav.hardwareConcurrency ?? 0,
    deviceMemoryGB: typeof nav.deviceMemory === 'number' ? nav.deviceMemory : null,
  };
}

// ==================== 场景 ====================

const NAV_PAGES = ['notes', 'discover', 'ai-studio', 'scheduled-tasks', 'tasks', 'conversation'];

/** 逐页冷/热 P95，用来判断保活后剩余瓶颈在哪一页，而不是只看混合 P95。 */
export function navPageP95Meta(
  pages: readonly string[],
  coldByPage: Record<string, number[]>,
  hotByPage: Record<string, number[]>,
): Record<string, number | null> {
  const meta: Record<string, number | null> = {};
  for (const page of pages) {
    meta[`coldP95_${page}`] = summarizeSamples(coldByPage[page] ?? [])?.p95 ?? null;
    meta[`hotP95_${page}`] = summarizeSamples(hotByPage[page] ?? [])?.p95 ?? null;
  }
  return meta;
}

async function runNavSwitchScenario(): Promise<ScenarioResult> {
  const visited = new Set<string>();
  const samples: number[] = [];
  const cold: number[] = [];
  const hot: number[] = [];
  const firstPaints: number[] = [];
  const coldByPage: Record<string, number[]> = Object.fromEntries(NAV_PAGES.map((id) => [id, []]));
  const hotByPage: Record<string, number[]> = Object.fromEntries(NAV_PAGES.map((id) => [id, []]));
  for (let round = 0; round < 2; round++) {
    for (const page of NAV_PAGES) {
      const t0 = performance.now();
      useAppStore.getState().setActivePage(page);
      await nextPaint();
      firstPaints.push(Math.round(performance.now() - t0));
      await settle();
      const ms = Math.round(performance.now() - t0);
      samples.push(ms);
      if (visited.has(page)) {
        hot.push(ms);
        hotByPage[page].push(ms);
      } else {
        cold.push(ms);
        coldByPage[page].push(ms);
      }
      visited.add(page);
    }
  }
  const coldStats = summarizeSamples(cold);
  const hotStats = summarizeSamples(hot);
  return {
    id: 'nav_switch',
    label: '页签冷/热切换（6 页 × 2 轮）',
    samples,
    stats: summarizeSamples(samples),
    meta: {
      coldP50: coldStats?.p50 ?? null,
      coldP95: coldStats?.p95 ?? null,
      hotP50: hotStats?.p50 ?? null,
      hotP95: hotStats?.p95 ?? null,
      firstPaintP50: summarizeSamples(firstPaints)?.p50 ?? null,
      ...navPageP95Meta(NAV_PAGES, coldByPage, hotByPage),
    },
    error: null,
  };
}

async function runDocABScenario(switches = 20): Promise<ScenarioResult> {
  const articles = useAppStore.getState().articles;
  const has5k = articles.some((a) => a.id === FIXTURE_5K_ID);
  const has50k = articles.some((a) => a.id === FIXTURE_50K_ID);
  if (!has5k || !has50k) {
    return {
      id: 'doc_ab_switch',
      label: '文档快速 A/B 切换（5KB↔50KB × 20）',
      samples: [],
      stats: null,
      meta: {},
      error: '缺少夹具文档，请先「播种语料」',
    };
  }
  const store = useAppStore.getState();
  store.setActivePage('notes');
  store.requestOpenArticle(FIXTURE_5K_ID);
  await settle();
  await sleep(200);

  const samples: number[] = [];
  for (let i = 1; i <= switches; i++) {
    const target = i % 2 === 1 ? FIXTURE_50K_ID : FIXTURE_5K_ID;
    const t0 = performance.now();
    useAppStore.getState().requestOpenArticle(target);
    await settle();
    samples.push(Math.round(performance.now() - t0));
  }
  return {
    id: 'doc_ab_switch',
    label: '文档快速 A/B 切换（5KB↔50KB × 20）',
    samples,
    stats: summarizeSamples(samples),
    meta: { switches },
    error: null,
  };
}

function typingChunk(round: number): string {
  const base = '性能夹具输入样本 perf-fixture typing probe。';
  return `${base}${round}\n`;
}

async function runTypingScenario(rounds = 30): Promise<ScenarioResult> {
  const articles = useAppStore.getState().articles;
  if (!articles.some((a) => a.id === FIXTURE_50K_ID)) {
    return {
      id: 'typing_50k',
      label: '50KB 长文输入延迟（30 轮注入）',
      samples: [],
      stats: null,
      meta: {},
      error: '缺少 50KB 夹具文档，请先「播种语料」',
    };
  }
  const store = useAppStore.getState();
  store.setActivePage('notes');
  store.requestOpenArticle(FIXTURE_50K_ID);
  await settle();

  // 进入编辑态并等待 Crepe 就绪（宿主挂载 + create 为异步）
  const workbench = getPerfProbeTargets().workbench;
  workbench?.enterEdit();
  let editor = getPerfProbeTargets().editor;
  const deadline = performance.now() + 4000;
  while ((!editor || !editor.isReady()) && performance.now() < deadline) {
    await sleep(50);
    editor = getPerfProbeTargets().editor;
  }
  if (!editor || !editor.isReady()) {
    return {
      id: 'typing_50k',
      label: '50KB 长文输入延迟（30 轮注入）',
      samples: [],
      stats: null,
      meta: {},
      error: '编辑器未在 4s 内就绪（需宿主在笔记本内打开 50KB 文档后重试）',
    };
  }

  const longTasks = startLongTaskCollector();
  const fps = startFpsCollector();
  const serializeBefore = perfSamples().filter((s) => s.label.includes('序列化')).length;
  const samples: number[] = [];
  let ok = true;
  for (let i = 0; i < rounds; i++) {
    const t0 = performance.now();
    const inserted = await editor.insertTextAtCursor(typingChunk(i));
    if (!inserted) {
      ok = false;
      break;
    }
    await nextPaint();
    samples.push(Math.round((performance.now() - t0) * 10) / 10);
  }
  longTasks.stop();
  fps.stop();
  const lt = longTasks.summary();
  const fpsSummary = fps.summary();
  const serializeDuring =
    perfSamples().filter((s) => s.label.includes('序列化')).length - serializeBefore;
  return {
    id: 'typing_50k',
    label: '50KB 长文输入延迟（30 轮注入）',
    samples,
    stats: summarizeSamples(samples),
    meta: {
      rounds: ok ? rounds : samples.length,
      longTaskCount: lt.count,
      longTaskTotalMs: lt.totalMs,
      longTaskMaxMs: lt.maxMs,
      longTaskSupported: lt.supported ? 1 : 0,
      fpsMin: fpsSummary.min,
      fpsAvg: fpsSummary.avg,
      serializeDuringTyping: serializeDuring,
    },
    error: ok ? null : 'insertTextAtCursor 失败（编辑器被销毁或只读）',
  };
}

async function runListScenario(): Promise<ScenarioResult> {
  const fixtureCount = useAppStore.getState().articles.filter(isFixtureArticle).length;
  const store = useAppStore.getState();
  const t0 = performance.now();
  store.setActivePage('notes');
  await nextPaint();
  await settle();
  const mountMs = Math.round(performance.now() - t0);

  const container = document.querySelector<HTMLElement>('[data-perf-scroll="doc-list"]');
  const fps = startFpsCollector();
  let scrolled = 0;
  if (container) {
    for (let i = 0; i < 24; i++) {
      container.scrollTop += 400;
      scrolled++;
      await sleep(80);
    }
  }
  fps.stop();
  const fpsSummary = fps.summary();
  return {
    id: 'list_200',
    label: '200 篇列表挂载 + 滚动',
    samples: [mountMs],
    stats: null,
    meta: {
      listMountMs: mountMs,
      fixtureDocs: fixtureCount,
      scrollSteps: scrolled,
      fpsMin: fpsSummary.min,
      fpsAvg: fpsSummary.avg,
    },
    error: container ? null : '未找到列表滚动容器（data-perf-scroll="doc-list"）',
  };
}

// ==================== 编排 ====================

export type ScenarioProgress = (message: string) => void;

export async function runAllScenarios(
  onProgress?: ScenarioProgress,
  onScenario?: (report: PerfReport) => void,
): Promise<PerfReport> {
  const nav = typeof navigator !== 'undefined' ? navigator : ({ userAgent: 'node' } as NavigatorLike);
  let appVersion = 'dev';
  try {
    appVersion = await getAppVersion();
  } catch {
    /* 非宿主环境保持 dev */
  }
  const report: PerfReport = {
    schema: 1,
    at: new Date().toISOString(),
    env: collectEnv(nav as NavigatorLike, appVersion),
    methodology: PERF_METHODOLOGY,
    scenarios: [],
  };
  const runners: Array<{ id: string; run: () => Promise<ScenarioResult> }> = [
    { id: 'nav_switch', run: runNavSwitchScenario },
    { id: 'doc_ab_switch', run: () => runDocABScenario() },
    { id: 'typing_50k', run: () => runTypingScenario() },
    { id: 'list_200', run: runListScenario },
  ];
  for (const { id, run } of runners) {
    onProgress?.(`运行 ${id} …`);
    try {
      report.scenarios.push(await raceTimeout(run(), 90_000, id));
    } catch (e) {
      report.scenarios.push({
        id,
        label: id,
        samples: [],
        stats: null,
        meta: {},
        error: e instanceof Error ? e.message : String(e),
      });
    }
    onProgress?.(`完成 ${id}`);
    onScenario?.(report);
    await sleep(300); // 场景间冷却，避免尾流相互污染
  }
  onProgress?.('完成');
  return report;
}

// ==================== 报告格式化（贴台账用） ====================

export function formatReportMarkdown(report: PerfReport): string {
  const lines: string[] = [];
  lines.push(`性能夹具基线（${report.at}）`);
  lines.push('');
  lines.push(`- 设备：${report.env.platform} / ${report.env.cores} 核 / ${report.env.deviceMemoryGB ?? '?'} GB`);
  lines.push(`- UA：${report.env.ua}`);
  lines.push(`- 构建：SophoNote ${report.env.appVersion}`);
  lines.push('');
  lines.push('| 场景 | n | P50 (ms) | P95 (ms) | max (ms) | 备注 |');
  lines.push('|---|---|---|---|---|---|');
  for (const s of report.scenarios) {
    const st = s.stats;
    const note = s.error
      ? `失败：${s.error}`
      : Object.entries(s.meta)
          .filter(([, v]) => v !== null && v !== undefined && v !== '')
          .map(([k, v]) => `${k}=${v}`)
          .join(' ');
    lines.push(
      `| ${s.label} | ${st?.n ?? 0} | ${st?.p50 ?? '—'} | ${st?.p95 ?? '—'} | ${st?.max ?? '—'} | ${note || '—'} |`,
    );
  }
  lines.push('');
  lines.push('采样方法：');
  for (const m of report.methodology) lines.push(`- ${m}`);
  return lines.join('\n');
}
