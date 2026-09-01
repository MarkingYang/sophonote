/**
 * NB-11 性能探针：轻量耗时记录器 + FPS 采样器（纯前端，常驻零感知，结果由 PerfOverlay 展示）。
 *
 * 定位 = 宿主走查「输入与滚动性能」的即插即用仪表：
 * - perfTime/perfMark 记录关键路径耗时（心跳序列化、分屏快照/更新、提及扫描），环形缓冲只留最近 N 条；
 * - startFpsSampler 以 rAF 计数 1s 窗口帧率，长文输入/滚动卡顿会直观反映在 FPS 掉档。
 * 记录本身成本为常数级（push + 越界 shift），不开面板也在跑，打开即看。
 */

export interface PerfSample {
  label: string;
  ms: number;
  at: number;
}

const RING_MAX = 60;

let ring: PerfSample[] = [];
const listeners = new Set<() => void>();

export function perfMark(label: string, ms: number): void {
  ring.push({ label, ms: Math.round(ms * 10) / 10, at: Date.now() });
  if (ring.length > RING_MAX) ring.shift();
  listeners.forEach((l) => l());
}

/** 同步耗时包装：fn 执行完打点（异步段不计，测的就是同步热路径） */
export function perfTime<T>(label: string, fn: () => T): T {
  const t0 = performance.now();
  const out = fn();
  perfMark(label, performance.now() - t0);
  return out;
}

export function perfSamples(): PerfSample[] {
  return ring;
}

export function perfClear(): void {
  ring = [];
  listeners.forEach((l) => l());
}

export function perfSubscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** FPS 采样：1s 窗口 rAF 计数；返回停止函数（组件卸载调用） */
export function startFpsSampler(onFps: (fps: number) => void): () => void {
  let raf = 0;
  let frames = 0;
  let windowStart = performance.now();
  let stopped = false;
  const loop = () => {
    if (stopped) return;
    frames++;
    const now = performance.now();
    if (now - windowStart >= 1000) {
      onFps(Math.round((frames * 1000) / (now - windowStart)));
      frames = 0;
      windowStart = now;
    }
    raf = requestAnimationFrame(loop);
  };
  raf = requestAnimationFrame(loop);
  return () => {
    stopped = true;
    cancelAnimationFrame(raf);
  };
}
