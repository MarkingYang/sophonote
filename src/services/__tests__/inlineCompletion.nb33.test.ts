/**
 * NB-33：AG-30 真实补全前端接入单测。
 * 覆盖：tauriCompletionProvider 契约映射 / 取消传播 / 竞态兜底、
 * 控制器对空文本（filtered）静默丢弃、真实标题/大纲上下文进请求、
 * 上下文求值器异常降级、传输层错误降级。
 * 全部 mock `services/tauri`，零真实模型调用、零 Tauri 依赖。
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('../tauri', () => ({
  completionSuggest: vi.fn(),
  completionCancel: vi.fn(),
  completionReportFeedback: vi.fn(),
}));

import { completionSuggest, completionCancel } from '../tauri';
import {
  InlineCompletionController,
  tauriCompletionProvider,
  type InlineCompletionCallbacks,
  type InlineCompletionRequest,
} from '../inlineCompletion';

const DEBOUNCE = 300;

function makeReq(overrides: Partial<InlineCompletionRequest> = {}): InlineCompletionRequest {
  return {
    requestId: 'ic-1',
    articleId: 'a1',
    documentVersion: 3,
    caret: { prosePos: 5, anchorHash: 'h1' },
    language: 'auto',
    prefix: '云端远程访问（非必需）。',
    suffix: '',
    title: '',
    outline: [],
    trigger: 'typing',
    ...overrides,
  };
}

/** 回显请求绑定的响应（绑定四元组一致 + 有文本 = 控制器可呈现） */
function echoResult(req: InlineCompletionRequest, text = '续写内容') {
  return {
    requestId: req.requestId,
    articleId: req.articleId,
    documentVersion: req.documentVersion,
    anchorHash: req.caret.anchorHash,
    text,
    finishReason: 'complete' as const,
    model: 'test-model',
    latencyMs: 10,
  };
}

function makeInput(overrides: Partial<Parameters<InlineCompletionController['scheduleTrigger']>[0]> = {}) {
  return {
    articleId: 'a1',
    documentVersion: 3,
    prosePos: 5,
    prefix: '云端远程访问（非必需）。',
    suffix: '',
    ...overrides,
  };
}

describe('NB-33 tauriCompletionProvider（AG-30 前端适配）', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('响应字段 1:1 映射（§4.3 camelCase 契约）', async () => {
    const req = makeReq();
    vi.mocked(completionSuggest).mockResolvedValueOnce(echoResult(req, '真实续写'));
    const result = await tauriCompletionProvider().complete(req, new AbortController().signal);
    expect(completionSuggest).toHaveBeenCalledWith(req);
    expect(result).toEqual({
      requestId: 'ic-1',
      articleId: 'a1',
      documentVersion: 3,
      anchorHash: 'h1',
      text: '真实续写',
      finishReason: 'complete',
      model: 'test-model',
      latencyMs: 10,
    });
  });

  it('已取消的 signal 直接 AbortError，不发请求', async () => {
    const ac = new AbortController();
    ac.abort();
    await expect(
      tauriCompletionProvider().complete(makeReq(), ac.signal)
    ).rejects.toMatchObject({ name: 'AbortError' });
    expect(completionSuggest).not.toHaveBeenCalled();
    expect(completionCancel).not.toHaveBeenCalled();
  });

  it('中途 abort：传播 completion_cancel，响应到达后按 AbortError 拒绝', async () => {
    let resolveSuggest!: (v: ReturnType<typeof echoResult>) => void;
    vi.mocked(completionSuggest).mockImplementationOnce(
      () => new Promise((resolve) => { resolveSuggest = resolve; })
    );
    const ac = new AbortController();
    const p = tauriCompletionProvider().complete(makeReq(), ac.signal);

    ac.abort();
    expect(completionCancel).toHaveBeenCalledWith('ic-1');

    // Rust 侧取消后仍会返回响应（timeout/filtered）——到达时按已取消丢弃
    resolveSuggest(echoResult(makeReq()));
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('completionCancel 失败不影响主流程（fire-and-forget）', async () => {
    vi.mocked(completionSuggest).mockResolvedValueOnce(echoResult(makeReq()));
    vi.mocked(completionCancel).mockRejectedValueOnce(new Error('invoke failed'));
    const ac = new AbortController();
    const p = tauriCompletionProvider().complete(makeReq(), ac.signal);
    ac.abort();
    // 未等到响应就 abort，但完成路径已被 abort 检查拦截；不应向外抛取消之外的错误
    await expect(p).rejects.toMatchObject({ name: 'AbortError' });
  });
});

describe('NB-33 控制器 × 真实 provider 集成（fake timers，零模型调用）', () => {
  let onVisible: ReturnType<typeof vi.fn>;
  let onDismiss: ReturnType<typeof vi.fn>;
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    onVisible = vi.fn();
    onDismiss = vi.fn();
    warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.useRealTimers();
    warnSpy.mockRestore();
  });

  function makeController() {
    return new InlineCompletionController(
      tauriCompletionProvider(),
      { onVisible, onDismiss } satisfies InlineCompletionCallbacks,
      DEBOUNCE
    );
  }

  it('正常路径：debounce → 请求 → 绑定一致 → visible', async () => {
    vi.mocked(completionSuggest).mockImplementation(async (req) => echoResult(req));
    const c = makeController();
    c.scheduleTrigger(makeInput({ getTitle: () => '笔记标题', getOutline: () => ['第一节', '第二节'] }));
    await vi.advanceTimersByTimeAsync(DEBOUNCE);
    expect(onVisible).toHaveBeenCalledTimes(1);
    expect(c.getPhase()).toBe('visible');
    expect(c.getVisibleText()).toBe('续写内容');

    // 真实标题/大纲随请求发出（§4.4 上下文）
    const sent = vi.mocked(completionSuggest).mock.calls[0][0];
    expect(sent.title).toBe('笔记标题');
    expect(sent.outline).toEqual(['第一节', '第二节']);
  });

  it('Rust 返回空文本（关闭/超时/过滤）时静默不展示', async () => {
    vi.mocked(completionSuggest).mockImplementation(async (req) => ({
      ...echoResult(req),
      text: '',
      finishReason: 'filtered' as const,
      model: 'disabled',
    }));
    const c = makeController();
    c.scheduleTrigger(makeInput());
    await vi.advanceTimersByTimeAsync(DEBOUNCE);
    expect(onVisible).not.toHaveBeenCalled();
    expect(c.getVisibleText()).toBeNull();
    // 静默丢弃语义（NB-32 既有）：不打扰用户，等待下一次触发
    c.scheduleTrigger(makeInput({ prefix: '' }));
    expect(c.getPhase()).toBe('idle');
  });

  it('上下文求值器抛错：降级为空上下文，补全本身不受影响', async () => {
    vi.mocked(completionSuggest).mockImplementation(async (req) => echoResult(req));
    const c = makeController();
    c.scheduleTrigger(
      makeInput({
        getTitle: () => {
          throw new Error('store unavailable');
        },
        getOutline: () => {
          throw new Error('doc unavailable');
        },
      })
    );
    await vi.advanceTimersByTimeAsync(DEBOUNCE);
    const sent = vi.mocked(completionSuggest).mock.calls[0][0];
    expect(sent.title).toBe('');
    expect(sent.outline).toEqual([]);
    expect(onVisible).toHaveBeenCalledTimes(1);
  });

  it('传输层错误（invoke reject）：降级 idle，不 visible，仅 console.warn', async () => {
    vi.mocked(completionSuggest).mockRejectedValueOnce(new Error('invoke failed'));
    const c = makeController();
    c.scheduleTrigger(makeInput());
    await vi.advanceTimersByTimeAsync(DEBOUNCE);
    expect(onVisible).not.toHaveBeenCalled();
    expect(c.getPhase()).toBe('idle');
    expect(warnSpy).toHaveBeenCalled();
  });

  it('绑定校验仍生效：Rust 回显旧 documentVersion 的结果被丢弃', async () => {
    vi.mocked(completionSuggest).mockImplementation(async (req) => ({
      ...echoResult(req),
      documentVersion: req.documentVersion - 1, // 模拟错位响应
    }));
    const c = makeController();
    c.scheduleTrigger(makeInput());
    await vi.advanceTimersByTimeAsync(DEBOUNCE);
    expect(onVisible).not.toHaveBeenCalled();
  });
});
