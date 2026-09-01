/**
 * NB-32：Inline Completion Spike —— 纯逻辑层单测。
 * 覆盖：锚点 hash、高风险结构抑制、mock provider（含取消）、
 * 控制器状态机（debounce/重触发/取消/旧结果丢弃/接受/拒绝）。
 * 全部走 fake timers，零真实模型调用。
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  anchorHash,
  isSuppressedContext,
  mockCompletionProvider,
  InlineCompletionController,
  type InlineCompletionProvider,
  type InlineCompletionRequest,
  type InlineCompletionResult,
} from '../inlineCompletion';

const DEBOUNCE = 300;
const MOCK_LATENCY = 250;

/** 可手动 flush 的 provider：忽略 abort，用于构造「旧结果晚到」场景 */
function manualProvider() {
  const pending: Array<(r: InlineCompletionResult) => void> = [];
  const provider: InlineCompletionProvider = {
    complete: (req: InlineCompletionRequest) =>
      new Promise<InlineCompletionResult>((resolve) => {
        pending.push(() =>
          resolve({
            requestId: req.requestId,
            articleId: req.articleId,
            documentVersion: req.documentVersion,
            anchorHash: req.caret.anchorHash,
            text: `结果@${req.requestId}`,
            finishReason: 'complete',
            model: 'manual',
            latencyMs: 0,
          })
        );
      }),
  };
  return {
    provider,
    flush: () => {
      while (pending.length > 0) pending.shift()!();
    },
    pendingCount: () => pending.length,
  };
}

function makeInput(overrides: Partial<Parameters<InlineCompletionController['scheduleTrigger']>[0]> = {}) {
  return {
    articleId: 'article-1',
    documentVersion: 1,
    prosePos: 10,
    prefix: '今天天气不错',
    suffix: '',
    ...overrides,
  };
}

describe('anchorHash', () => {
  it('确定性：相同上下文同 hash', () => {
    expect(anchorHash('abc', 'def')).toBe(anchorHash('abc', 'def'));
  });
  it('敏感：上下文不同则 hash 不同', () => {
    expect(anchorHash('abc', 'def')).not.toBe(anchorHash('abc', 'deg'));
    expect(anchorHash('abc', 'def')).not.toBe(anchorHash('abd', 'def'));
  });
});

describe('isSuppressedContext（高风险结构抑制）', () => {
  it('未闭合代码围栏内 → 抑制', () => {
    expect(isSuppressedContext('```js\nconsole.log(1)\n// 写代码')).toBe(true);
  });
  it('已闭合代码围栏后 → 不抑制', () => {
    expect(isSuppressedContext('```\ncode\n```\n回到正文')).toBe(false);
  });
  it('frontmatter 未闭合 → 抑制；闭合后 → 不抑制', () => {
    expect(isSuppressedContext('---\ntitle: x\n')).toBe(true);
    expect(isSuppressedContext('---\ntitle: x\n---\n正文')).toBe(false);
  });
  it('表格行内 → 抑制', () => {
    expect(isSuppressedContext('| 列A | 列B')).toBe(true);
  });
  it('行内代码未闭合 → 抑制', () => {
    expect(isSuppressedContext('这里是 `code')).toBe(true);
    expect(isSuppressedContext('这里是 `code` 之后')).toBe(false);
  });
  it('链接地址内 → 抑制', () => {
    expect(isSuppressedContext('见 [说明](https://ex')).toBe(true);
    expect(isSuppressedContext('见 [说明](https://x.com) 之后')).toBe(false);
  });
  it('普通段落 → 不抑制；空前缀 → 不抑制', () => {
    expect(isSuppressedContext('今天天气不错')).toBe(false);
    expect(isSuppressedContext('')).toBe(false);
  });
});

describe('mockCompletionProvider', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const req: InlineCompletionRequest = {
    requestId: 'ic-1',
    articleId: 'article-1',
    documentVersion: 3,
    caret: { prosePos: 5, anchorHash: 'h1' },
    language: 'auto',
    prefix: '今天天气不错',
    suffix: '',
    title: '',
    outline: [],
    trigger: 'typing',
  };

  it('确定性回显绑定四元组，finishReason=complete', async () => {
    const p = mockCompletionProvider().complete(req, new AbortController().signal);
    await vi.advanceTimersByTimeAsync(MOCK_LATENCY);
    const r = await p;
    expect(r.requestId).toBe('ic-1');
    expect(r.articleId).toBe('article-1');
    expect(r.documentVersion).toBe(3);
    expect(r.anchorHash).toBe('h1');
    expect(r.finishReason).toBe('complete');
    expect(r.text).toContain('mock 补全');
  });

  it('abort → 拒绝且为 AbortError', async () => {
    const ac = new AbortController();
    const p = mockCompletionProvider().complete(req, ac.signal);
    // 先挂断言再 abort：保证拒绝发生时 handler 已就位，避免 unhandled rejection
    const expectation = expect(p).rejects.toMatchObject({ name: 'AbortError' });
    ac.abort();
    await vi.advanceTimersByTimeAsync(MOCK_LATENCY);
    await expectation;
  });
});

describe('InlineCompletionController 状态机', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function setup(provider?: InlineCompletionProvider) {
    const onVisible = vi.fn();
    const onDismiss = vi.fn();
    const controller = new InlineCompletionController(
      provider ?? mockCompletionProvider(),
      { onVisible, onDismiss },
      DEBOUNCE
    );
    return { controller, onVisible, onDismiss };
  }

  it('debounce：300ms 未到不请求，到点请求并可见', async () => {
    const { controller, onVisible } = setup();
    controller.scheduleTrigger(makeInput());
    expect(controller.getPhase()).toBe('debouncing');
    await vi.advanceTimersByTimeAsync(DEBOUNCE - 1);
    expect(controller.getPhase()).toBe('debouncing');
    await vi.advanceTimersByTimeAsync(1);
    expect(controller.getPhase()).toBe('requesting');
    await vi.advanceTimersByTimeAsync(MOCK_LATENCY);
    expect(controller.getPhase()).toBe('visible');
    expect(onVisible).toHaveBeenCalledTimes(1);
    expect(controller.getVisibleText()).toContain('mock 补全');
  });

  it('连续输入重置 debounce：只发一次请求', async () => {
    const { controller, onVisible } = setup();
    controller.scheduleTrigger(makeInput({ prefix: '第一句' }));
    await vi.advanceTimersByTimeAsync(DEBOUNCE - 100);
    controller.scheduleTrigger(makeInput({ prefix: '第一句，第二句' }));
    await vi.advanceTimersByTimeAsync(DEBOUNCE + MOCK_LATENCY);
    expect(onVisible).toHaveBeenCalledTimes(1);
    expect(controller.getPhase()).toBe('visible');
  });

  it('debounce 期间 cancel：不请求、回 idle', async () => {
    const provider = { complete: vi.fn() };
    const { controller } = setup(provider as unknown as InlineCompletionProvider);
    controller.scheduleTrigger(makeInput());
    controller.cancel();
    await vi.advanceTimersByTimeAsync(DEBOUNCE + MOCK_LATENCY);
    expect(provider.complete).not.toHaveBeenCalled();
    expect(controller.getPhase()).toBe('idle');
  });

  it('旧结果晚到直接丢弃（requestId/版本绑定校验）', async () => {
    const { provider, flush } = manualProvider();
    const { controller, onVisible } = setup(provider);

    controller.scheduleTrigger(makeInput({ documentVersion: 1 }));
    await vi.advanceTimersByTimeAsync(DEBOUNCE); // 请求 ic-1 在途
    controller.scheduleTrigger(makeInput({ documentVersion: 2 })); // 触发 abort + 新 debounce
    await vi.advanceTimersByTimeAsync(DEBOUNCE); // 请求 ic-2 在途

    flush(); // 两个结果都到达：ic-1 必须被丢弃
    await vi.advanceTimersByTimeAsync(0);
    expect(onVisible).toHaveBeenCalledTimes(1);
    expect(controller.getVisibleText()).toBe('结果@ic-2');
  });

  it('抑制上下文与空 prefix 不触发请求', async () => {
    const provider = { complete: vi.fn() };
    const { controller } = setup(provider as unknown as InlineCompletionProvider);
    controller.scheduleTrigger(makeInput({ prefix: '```js\n在代码里' }));
    controller.scheduleTrigger(makeInput({ prefix: '   ' }));
    await vi.advanceTimersByTimeAsync(DEBOUNCE + MOCK_LATENCY);
    expect(provider.complete).not.toHaveBeenCalled();
    expect(controller.getPhase()).toBe('idle');
  });

  it('accept：返回文本并回 idle；无可见建议时返回 null', async () => {
    const { controller } = setup();
    expect(controller.accept()).toBeNull();
    controller.scheduleTrigger(makeInput());
    await vi.advanceTimersByTimeAsync(DEBOUNCE + MOCK_LATENCY);
    const text = controller.accept();
    expect(text).toContain('mock 补全');
    expect(controller.getPhase()).toBe('idle');
    expect(controller.getVisibleText()).toBeNull();
  });

  it('dismiss：触发 onDismiss 并回 idle', async () => {
    const { controller, onDismiss } = setup();
    controller.scheduleTrigger(makeInput());
    await vi.advanceTimersByTimeAsync(DEBOUNCE + MOCK_LATENCY);
    controller.dismiss();
    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(controller.getPhase()).toBe('idle');
  });

  it('visible 状态下再次 scheduleTrigger：先 dismiss 旧建议', async () => {
    const { controller, onDismiss } = setup();
    controller.scheduleTrigger(makeInput());
    await vi.advanceTimersByTimeAsync(DEBOUNCE + MOCK_LATENCY);
    expect(controller.getPhase()).toBe('visible');
    controller.scheduleTrigger(makeInput({ prefix: '继续写点别的' }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(controller.getPhase()).toBe('debouncing');
  });
});
