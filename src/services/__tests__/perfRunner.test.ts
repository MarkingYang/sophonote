import { describe, expect, it } from 'vitest';
import { collectEnv, formatReportMarkdown, navPageP95Meta, PERF_METHODOLOGY, raceTimeout, settle, type PerfReport } from '../perfRunner';

describe('NEXT-001 环境采集', () => {
  it('缺失字段降级为安全默认值', () => {
    const env = collectEnv({ userAgent: 'vitest' }, '0.1.0');
    expect(env.appVersion).toBe('0.1.0');
    expect(env.ua).toBe('vitest');
    expect(env.platform).toBe('unknown');
    expect(env.cores).toBe(0);
    expect(env.deviceMemoryGB).toBeNull();
  });

  it('完整字段透传', () => {
    const env = collectEnv(
      { userAgent: 'ua', platform: 'MacIntel', hardwareConcurrency: 10, deviceMemory: 16 },
      '1.2.3',
    );
    expect(env.platform).toBe('MacIntel');
    expect(env.cores).toBe(10);
    expect(env.deviceMemoryGB).toBe(16);
  });
});

describe('NEXT-001 台账表格格式化', () => {
  const report: PerfReport = {
    schema: 1,
    at: '2026-08-16T10:00:00.000Z',
    env: {
      appVersion: '0.5.0',
      ua: 'WebKit',
      platform: 'MacIntel',
      cores: 10,
      deviceMemoryGB: 16,
    },
    methodology: PERF_METHODOLOGY,
    scenarios: [
      {
        id: 'doc_ab_switch',
        label: '文档快速 A/B 切换（5KB↔50KB × 20）',
        samples: [120, 130, 180],
        stats: { n: 3, min: 120, max: 180, mean: 143.3, p50: 130, p95: 180 },
        meta: { switches: 20 },
        error: null,
      },
      {
        id: 'typing_50k',
        label: '50KB 长文输入延迟（30 轮注入）',
        samples: [],
        stats: null,
        meta: {},
        error: '编辑器未在 4s 内就绪',
      },
    ],
  };

  it('包含设备/构建/采样方法三要素（台账 NEXT-001 验收口径）', () => {
    const md = formatReportMarkdown(report);
    expect(md).toContain('MacIntel');
    expect(md).toContain('10 核');
    expect(md).toContain('SophoNote 0.5.0');
    expect(md).toContain('采样方法：');
    expect(md).toContain('静默 150ms');
  });

  it('场景行输出 P50/P95/max；失败场景带原因', () => {
    const md = formatReportMarkdown(report);
    expect(md).toContain('| 文档快速 A/B 切换');
    expect(md).toContain('130');
    expect(md).toContain('180');
    expect(md).toContain('失败：编辑器未在 4s 内就绪');
  });
});

describe('NEXT-001 场景超时', () => {
  it('超时后拒绝，不永久挂起', async () => {
    await expect(raceTimeout(new Promise(() => undefined), 20, 'typing_50k')).rejects.toThrow(
      'typing_50k 超时 20ms',
    );
  });

  it('准时完成则返回原值', async () => {
    await expect(raceTimeout(Promise.resolve(7), 50, 'ok')).resolves.toBe(7);
  });
});

describe('NEXT-001 settle 墙钟收口', () => {
  it('持续 mutation 时在 timeoutMs 内结束', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const id = window.setInterval(() => {
      el.dataset.n = String(Date.now());
    }, 20);
    try {
      const ms = await settle(150, 280);
      expect(ms).toBeGreaterThanOrEqual(140);
      expect(ms).toBeLessThan(1200);
    } finally {
      window.clearInterval(id);
      el.remove();
    }
  });
});

describe('NEXT-004 逐页热切换 P95', () => {
  it('按页输出冷/热 P95，缺样本为 null', () => {
    const meta = navPageP95Meta(
      ['notes', 'discover'],
      { notes: [200], discover: [80] },
      { notes: [120, 140], discover: [] },
    );
    expect(meta.coldP95_notes).toBe(200);
    expect(meta.hotP95_notes).toBe(140);
    expect(meta.coldP95_discover).toBe(80);
    expect(meta.hotP95_discover).toBeNull();
  });
});
