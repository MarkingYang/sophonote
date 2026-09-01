/**
 * NB-31：保存失败契约 —— 把 flush 保存的核心决策抽成纯函数，可独立单测。
 *
 * 背景（P0-1 假保存）：此前 appStore 写动作吞掉后端异常，flushSave 的 catch 永远
 * 不触发，`lastSavedMdRef` 照常推进、dirty 照常清除、状态栏照常显示"已保存"，
 * 后端写失败时重启即丢内容。
 *
 * 本模块只负责决策，不持有任何 React/Zustand/Tauri 依赖：
 * - 正文写失败 → 不推进正文基线，且不再尝试写标题（避免"半成功"只落标题），
 *   由调用方保留 dirty、展示错误，下次 flush 自然重试。
 * - 正文成功、标题失败 → 正文基线推进，标题基线不推进，整体仍报 error。
 * - 两者都无变化 → unchanged，调用方可安全清 dirty。
 */

export interface FlushInput {
  /** 编辑器当前正文（或外部来源的最新正文） */
  md: string;
  /** 当前标题 */
  title: string;
  /** 上一次确认落盘成功的正文基线 */
  lastSavedMd: string;
  /** 上一次确认落盘成功的标题基线 */
  savedTitle: string;
  /** 正文写入（失败须 reject） */
  writeContent: (md: string) => Promise<void>;
  /** 标题写入（失败须 reject） */
  writeTitle: (title: string) => Promise<void>;
}

export type FlushStatus = 'unchanged' | 'saved' | 'error';

export interface FlushOutcome {
  status: FlushStatus;
  /** flush 后的正文基线：仅在写入成功后推进（失败保留 dirty，防假保存） */
  lastSavedMd: string;
  /** flush 后的标题基线：仅在写入成功后推进 */
  savedTitle: string;
  /** status === 'error' 时的失败原因（已归一为字符串） */
  error?: string;
}

/** 把未知异常归一为可展示的错误文案 */
export const messageOf = (e: unknown): string =>
  e instanceof Error ? e.message : String(e);

/**
 * 执行一次"正文优先、标题次之"的 flush。本函数不抛异常，所有失败都折叠进
 * outcome.error，由调用方映射到 dirty / savedAt / 错误提示等 UI 状态。
 */
export async function flushDocument(input: FlushInput): Promise<FlushOutcome> {
  const { md, title, lastSavedMd, savedTitle, writeContent, writeTitle } = input;
  let nextMd = lastSavedMd;
  let nextTitle = savedTitle;
  const contentChanged = md !== lastSavedMd;
  const titleChanged = title !== savedTitle;

  if (!contentChanged && !titleChanged) {
    return { status: 'unchanged', lastSavedMd, savedTitle };
  }

  if (contentChanged) {
    try {
      await writeContent(md);
      nextMd = md;
    } catch (e) {
      return { status: 'error', lastSavedMd: nextMd, savedTitle: nextTitle, error: messageOf(e) };
    }
  }

  if (titleChanged) {
    try {
      await writeTitle(title);
      nextTitle = title;
    } catch (e) {
      return { status: 'error', lastSavedMd: nextMd, savedTitle: nextTitle, error: messageOf(e) };
    }
  }

  return { status: 'saved', lastSavedMd: nextMd, savedTitle: nextTitle };
}
