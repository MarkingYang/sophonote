import { describe, expect, it } from 'vitest';
import {
  STREAM_VISUAL_BATCH_LIMIT,
  STREAM_VISUAL_MAX_INTERVAL_MS,
  STREAM_VISUAL_MIN_INTERVAL_MS,
  STREAM_VISUAL_TARGET_CHARS,
  streamFlushDelay,
} from '../agentStreamBatching';

describe('Hermes 流式视觉合帧', () => {
  it('少量字符最多等待 96ms，避免稀疏流逐字停顿', () => {
    expect(streamFlushDelay({ elapsedMs: 0, pendingChars: 2, pendingEvents: 1 }))
      .toBe(STREAM_VISUAL_MAX_INTERVAL_MS);
    expect(streamFlushDelay({ elapsedMs: 100, pendingChars: 2, pendingEvents: 1 })).toBe(0);
  });

  it('积累到短语长度后按最小视觉间隔刷新', () => {
    expect(streamFlushDelay({
      elapsedMs: 20,
      pendingChars: STREAM_VISUAL_TARGET_CHARS,
      pendingEvents: 12,
    })).toBe(STREAM_VISUAL_MIN_INTERVAL_MS - 20);
  });

  it('事件过多时立即刷新，限制内存与终端压力', () => {
    expect(streamFlushDelay({
      elapsedMs: 0,
      pendingChars: 1,
      pendingEvents: STREAM_VISUAL_BATCH_LIMIT,
    })).toBe(0);
  });

  it('句末和换行形成语义边界时立即刷新', () => {
    expect(streamFlushDelay({
      elapsedMs: 4,
      pendingChars: 8,
      pendingEvents: 2,
      pendingText: '这是结论。',
    })).toBe(0);
    expect(streamFlushDelay({
      elapsedMs: 4,
      pendingChars: 8,
      pendingEvents: 2,
      pendingText: '下一段\n',
    })).toBe(0);
  });
});
