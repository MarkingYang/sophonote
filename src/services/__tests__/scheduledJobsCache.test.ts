import { beforeEach, describe, expect, it } from 'vitest';
import {
  cachedScheduledJobs,
  resetScheduledJobsCacheForTests,
  scheduledJobsCacheHydrated,
  setCachedScheduledJobs,
  shouldFetchScheduledJobsOnMount,
} from '../scheduledJobsCache';
import type { HermesCronJobInfo } from '../tauri';

const job = (id: string): HermesCronJobInfo =>
  ({
    id,
    name: id,
    profile: 'default',
  }) as HermesCronJobInfo;

describe('NEXT-004 计划任务列表热切换缓存', () => {
  beforeEach(() => {
    resetScheduledJobsCacheForTests();
  });

  it('写入后标记已 hydrate，空列表也可作为首帧', () => {
    setCachedScheduledJobs([]);
    expect(scheduledJobsCacheHydrated()).toBe(true);
    expect(shouldFetchScheduledJobsOnMount()).toBe(false);
    expect(cachedScheduledJobs()).toEqual([]);
    setCachedScheduledJobs([job('cron-1')]);
    expect(cachedScheduledJobs().map((item) => item.id)).toEqual(['cron-1']);
  });

  it('未 hydrate 时首屏需要拉取，hydrate 后热切换不再拉', () => {
    expect(shouldFetchScheduledJobsOnMount()).toBe(true);
    setCachedScheduledJobs([job('cron-1')]);
    expect(shouldFetchScheduledJobsOnMount()).toBe(false);
  });
});
