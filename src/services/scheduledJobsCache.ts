import type { HermesCronJobInfo } from './tauri';

/**
 * 计划任务列表的会话级缓存。页签会卸载，热切换时用上次列表画首帧，
 * 避免每次都先进入 loading spinner 再整表替换（拖长 MutationObserver settle）。
 */
let jobs: HermesCronJobInfo[] = [];
let hydrated = false;

export function scheduledJobsCacheHydrated(): boolean {
  return hydrated;
}

export function cachedScheduledJobs(): HermesCronJobInfo[] {
  return jobs;
}

export function setCachedScheduledJobs(next: HermesCronJobInfo[]): void {
  jobs = next;
  hydrated = true;
}

/** 已有快照时热切换不再请求；15s 轮询仍会校准。 */
export function shouldFetchScheduledJobsOnMount(): boolean {
  return !hydrated;
}

export function resetScheduledJobsCacheForTests(): void {
  jobs = [];
  hydrated = false;
}
