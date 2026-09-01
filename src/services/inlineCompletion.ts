/**
 * 行内补全纯逻辑层（零 ProseMirror 依赖，可单测）。NB-32 Spike 落地，NB-33 产品化。
 *
 * Provider 双轨：mockCompletionProvider（NB-32 单测/走查基线，保留不删）+
 * tauriCompletionProvider（NB-33：对接 AG-30 Rust CompletionService，装配层默认）。
 *
 * 设计基线：docs/architecture.md（请求契约 §4.3 逐字段对齐）。
 * 硬约束（§十一）：ghost text 不进 Markdown/dirty/历史；不创建 Thread/Run；
 * 旧 request/version/anchor 结果直接丢弃，绝不在新位置插入旧建议。
 */
import { completionCancel, completionSuggest } from './tauri';

// ---------- 请求契约（设计 §4.3） ----------

export interface InlineCompletionRequest {
  requestId: string;
  articleId: string;
  documentVersion: number;
  caret: { prosePos: number; anchorHash: string };
  language: 'zh-CN' | 'en' | 'auto';
  prefix: string; // 截断后的光标前上下文
  suffix: string; // 截断后的光标后上下文
  title: string;
  outline: string[];
  projectId?: string;
  trigger: 'typing' | 'manual';
}

export interface InlineCompletionResult {
  requestId: string;
  articleId: string;
  documentVersion: number;
  anchorHash: string;
  text: string;
  finishReason: 'complete' | 'timeout' | 'filtered';
  model: string;
  latencyMs: number;
}

export interface InlineCompletionProvider {
  complete(
    req: InlineCompletionRequest,
    signal: AbortSignal
  ): Promise<InlineCompletionResult>;
}

// ---------- 锚点 hash（djb2，确定性、零依赖） ----------

export function anchorHash(before: string, after: string): string {
  const s = `${before.slice(-32)} ${after.slice(0, 32)}`;
  let h = 5381;
  for (let i = 0; i < s.length; i++) {
    h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  }
  return (h >>> 0).toString(36);
}

// ---------- 高风险结构抑制（设计 §4.1 第 7 条，Spike 子集） ----------

/**
 * 光标前上下文是否落在默认抑制结构中：
 * frontmatter / 代码围栏 / 表格行 / 行内代码 / 链接地址。
 * 判定只基于 prefix（光标前文本），纯函数可单测。
 */
export function isSuppressedContext(prefix: string): boolean {
  if (prefix.length === 0) return false;

  // frontmatter：文档以 --- 开头且尚未闭合（闭合需出现第二个独占行的 ---）
  if (/^---\r?\n/.test(prefix)) {
    const rest = prefix.replace(/^---\r?\n/, '');
    if (!/(\r?\n)---\s*(\r?\n|$)/.test(rest)) return true;
  }

  const lines = prefix.split(/\r?\n/);
  const currentLine = lines[lines.length - 1];

  // 代码围栏：prefix 中 ``` 出现奇数次 = 仍在围栏内
  const fenceCount = (prefix.match(/```/g) ?? []).length;
  if (fenceCount % 2 === 1) return true;

  // 表格行：当前行以 | 开头
  if (/^\s*\|/.test(currentLine)) return true;

  // 行内代码：当前行单反引号奇数个
  const inlineTicks = (currentLine.match(/`/g) ?? []).length;
  if (inlineTicks % 2 === 1) return true;

  // 链接地址：当前行存在未闭合的 `](`
  const lastOpen = currentLine.lastIndexOf('](');
  if (lastOpen >= 0 && currentLine.indexOf(')', lastOpen) < 0) return true;

  return false;
}

// ---------- Mock Provider（Spike 专用，确定性 + 可取消） ----------

const MOCK_LATENCY_MS = 250;
const MOCK_MODEL = 'mock-provider';

/**
 * 确定性 mock：延迟 250ms 后回一条可辨识的续写文本；abort 时拒绝。
 * 文案带「mock 补全」标记，走查时肉眼可辨，防止误当真实内容。
 * NB-33 起仅用于单测/降级演示，产品装配默认走 tauriCompletionProvider。
 */
export function mockCompletionProvider(): InlineCompletionProvider {
  return {
    complete(req, signal) {
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          if (signal.aborted) return; // 由 abort 监听负责 reject
          const tail = req.prefix.replace(/\s+$/, '').slice(-12);
          resolve({
            requestId: req.requestId,
            articleId: req.articleId,
            documentVersion: req.documentVersion,
            anchorHash: req.caret.anchorHash,
            text: `（mock 补全：顺着「${tail || '空文档'}」续写的一句。）`,
            finishReason: 'complete',
            model: MOCK_MODEL,
            latencyMs: MOCK_LATENCY_MS,
          });
        }, MOCK_LATENCY_MS);
        signal.addEventListener('abort', () => {
          clearTimeout(timer);
          reject(new DOMException('Aborted', 'AbortError'));
        });
      });
    },
  };
}

// ---------- Tauri Provider（NB-33：AG-30 真实 CompletionService 前端适配） ----------

/**
 * NB-33：把请求经 Tauri 送到 Rust `completion_suggest`（AG-30 轻量路径，不建 Thread/Run）。
 *
 * 契约要点（设计 §4.3）：
 * - 请求/响应字段均 camelCase，与 Rust serde rename_all 对齐，响应字段 1:1 映射回 InlineCompletionResult；
 * - Rust 侧对 关闭/未配置/超时/过滤 一律返回 ok + 空 text（finishReason 区分），
 *   前端控制器对空 text 本就静默丢弃——失败永不弹窗、不打扰输入；
 * - AbortSignal 触发时向 Rust 传播 completion_cancel（CancellationToken），
 *   并以 AbortError 拒绝本地 Promise，与 mock provider 的取消语义一致；
 * - invoke 传输层错误（非 AbortError）向上抛，由控制器 catch 降级为 idle（仅 console.warn）。
 */
export function tauriCompletionProvider(): InlineCompletionProvider {
  return {
    async complete(req, signal) {
      if (signal.aborted) {
        throw new DOMException('Aborted', 'AbortError');
      }
      const onAbort = () => {
        // 取消传播 fire-and-forget：命令本身永不报错（找不到请求 = false），
        // 即使 Rust 侧已完成，结果也会被控制器的绑定四元组校验丢弃
        void completionCancel(req.requestId).catch(() => {});
      };
      signal.addEventListener('abort', onAbort, { once: true });
      try {
        const r = await completionSuggest(req);
        // 竞态兜底：建议到达前已被取消（新输入/切文档）→ 按 AbortError 走既有丢弃路径
        if (signal.aborted) {
          throw new DOMException('Aborted', 'AbortError');
        }
        return {
          requestId: r.requestId,
          articleId: r.articleId,
          documentVersion: r.documentVersion,
          anchorHash: r.anchorHash,
          text: r.text,
          finishReason: r.finishReason,
          model: r.model,
          latencyMs: r.latencyMs,
        };
      } finally {
        signal.removeEventListener('abort', onAbort);
      }
    },
  };
}

// ---------- 控制器状态机（设计 §4.2） ----------

export type InlineCompletionPhase = 'idle' | 'debouncing' | 'requesting' | 'visible';

/** 一次建议的绑定四元组：任一不一致结果直接丢弃（设计 §4.2 末段） */
export interface CompletionBinding {
  articleId: string;
  documentVersion: number;
  anchorHash: string;
  prosePos: number;
}

export interface InlineCompletionCallbacks {
  /** 建议可呈现（已通过绑定校验） */
  onVisible: (result: InlineCompletionResult, binding: CompletionBinding) => void;
  /** 建议被取消/拒绝/失效 */
  onDismiss: () => void;
}

/**
 * InlineCompletionController：debounce、取消、状态机、绑定校验、接受/拒绝。
 * 禁止事项（设计 §3.1）：不创建 Thread/Run，不直接保存文档——本类对编辑器零感知，
 * 编辑器侧（inlineCompletionPlugin）负责 Decoration 呈现与接受后的普通文本插入。
 */
export class InlineCompletionController {
  private phase: InlineCompletionPhase = 'idle';
  private timer: ReturnType<typeof setTimeout> | null = null;
  private abortController: AbortController | null = null;
  private binding: CompletionBinding | null = null;
  private inflightRequestId: string | null = null;
  private visibleResult: InlineCompletionResult | null = null;
  private requestSeq = 0;

  constructor(
    private readonly provider: InlineCompletionProvider,
    private readonly callbacks: InlineCompletionCallbacks,
    private readonly debounceMs = 300
  ) {}

  getPhase(): InlineCompletionPhase {
    return this.phase;
  }

  getVisibleText(): string | null {
    return this.phase === 'visible' && this.visibleResult ? this.visibleResult.text : null;
  }

  /**
   * 编辑器每次有效更新（输入/光标落定）调用：重置 debounce。
   * suppressed 上下文与空 prefix 直接不触发（设计 §4.1）。
   *
   * NB-33：getTitle/getOutline 为惰性上下文求值器——debounce 落定真正发请求时才调用，
   * 拿到的是最新标题/大纲（300ms 内改名/改标题不会发出旧上下文）；缺省 = 空上下文。
   */
  scheduleTrigger(input: {
    articleId: string;
    documentVersion: number;
    prosePos: number;
    prefix: string;
    suffix: string;
    getTitle?: () => string;
    getOutline?: () => string[];
  }): void {
    this.clearTimer();

    // 任何新触发都意味着旧建议失效
    if (this.phase === 'visible') {
      this.dismiss();
    }
    this.abortInflight();

    if (input.prefix.trim().length === 0 || isSuppressedContext(input.prefix)) {
      this.phase = 'idle';
      this.binding = null;
      return;
    }

    this.binding = {
      articleId: input.articleId,
      documentVersion: input.documentVersion,
      anchorHash: anchorHash(input.prefix, input.suffix),
      prosePos: input.prosePos,
    };
    this.phase = 'debouncing';

    const snapshot = { ...input, prefix: input.prefix, suffix: input.suffix };
    this.timer = setTimeout(() => this.fire(snapshot), this.debounceMs);
  }

  /** 外部主动取消（切文档/销毁），等价于状态机 stale/cancelled 分支 */
  cancel(): void {
    this.clearTimer();
    this.abortInflight();
    const wasVisible = this.phase === 'visible';
    this.phase = 'idle';
    this.binding = null;
    this.visibleResult = null;
    if (wasVisible) this.callbacks.onDismiss();
  }

  /** Tab 接受：返回建议文本（无可见建议返回 null），状态回 idle */
  accept(): string | null {
    if (this.phase !== 'visible' || !this.visibleResult) return null;
    const text = this.visibleResult.text;
    this.phase = 'idle';
    this.visibleResult = null;
    this.binding = null;
    return text;
  }

  /** Esc 拒绝 */
  dismiss(): void {
    if (this.phase !== 'visible') return;
    this.phase = 'idle';
    this.visibleResult = null;
    this.binding = null;
    this.callbacks.onDismiss();
  }

  dispose(): void {
    this.clearTimer();
    this.abortInflight();
    this.phase = 'idle';
    this.binding = null;
    this.visibleResult = null;
  }

  // ---------- 内部 ----------

  private clearTimer(): void {
    if (this.timer != null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  private abortInflight(): void {
    if (this.abortController) {
      this.abortController.abort();
      this.abortController = null;
    }
    this.inflightRequestId = null;
  }

  private fire(input: {
    articleId: string;
    documentVersion: number;
    prosePos: number;
    prefix: string;
    suffix: string;
    getTitle?: () => string;
    getOutline?: () => string[];
  }): void {
    this.timer = null;
    const binding = this.binding;
    if (!binding) {
      this.phase = 'idle';
      return;
    }

    this.requestSeq += 1;
    const requestId = `ic-${this.requestSeq}`;
    this.inflightRequestId = requestId;
    this.phase = 'requesting';
    this.abortController = new AbortController();
    const signal = this.abortController.signal;

    // NB-33：真实标题/大纲上下文（设计 §4.4 上下文预算；求值器异常不阻塞补全，降级为空上下文）
    let title = '';
    let outline: string[] = [];
    try {
      title = input.getTitle?.() ?? '';
      outline = input.getOutline?.() ?? [];
    } catch (e) {
      console.warn('[nb33] completion context resolve failed:', e);
      title = '';
      outline = [];
    }

    const req: InlineCompletionRequest = {
      requestId,
      articleId: input.articleId,
      documentVersion: input.documentVersion,
      caret: { prosePos: input.prosePos, anchorHash: binding.anchorHash },
      language: 'auto',
      prefix: input.prefix,
      suffix: input.suffix,
      title,
      outline,
      trigger: 'typing',
    };

    this.provider
      .complete(req, signal)
      .then((result) => {
        // 绑定四元组校验：任一不一致直接丢弃（设计 §4.2）
        if (
          this.phase !== 'requesting' ||
          this.inflightRequestId !== result.requestId ||
          !this.binding ||
          this.binding.articleId !== result.articleId ||
          this.binding.documentVersion !== result.documentVersion ||
          this.binding.anchorHash !== result.anchorHash ||
          result.text.length === 0
        ) {
          return; // stale/cancelled：静默丢弃，不通知 UI
        }
        this.phase = 'visible';
        this.visibleResult = result;
        this.abortController = null;
        this.callbacks.onVisible(result, this.binding);
      })
      .catch((err: unknown) => {
        // AbortError 是预期路径；其它错误 Spike 期仅降级不打扰用户
        if (!(err instanceof DOMException && err.name === 'AbortError')) {
          console.warn('[nb32] completion provider error:', err);
        }
        if (this.phase === 'requesting' && this.inflightRequestId === requestId) {
          this.phase = 'idle';
          this.binding = null;
        }
      });
  }
}
