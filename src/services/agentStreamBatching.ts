/**
 * Hermes 通常以 1～3 个字符发送 delta。SophoNote 保留真实流式，但把这些 wire token
 * 合成稳定的视觉帧，避免 Markdown 每个字都重建 AST、用户看到逐字“蹦出”。
 */
export const STREAM_VISUAL_MIN_INTERVAL_MS = 48;
export const STREAM_VISUAL_MAX_INTERVAL_MS = 96;
export const STREAM_VISUAL_TARGET_CHARS = 12;
export const STREAM_VISUAL_BATCH_LIMIT = 96;

interface StreamFlushInput {
  elapsedMs: number;
  pendingChars: number;
  pendingEvents: number;
  pendingText?: string;
}

function endsAtSemanticBoundary(text: string | undefined): boolean {
  if (!text || text.length < 3) return false;
  return /(?:\n|[。！？!?])$/.test(text);
}

/** 返回距下一次视觉归约还需等待的毫秒数；0 表示应立即 flush。 */
export function streamFlushDelay({
  elapsedMs,
  pendingChars,
  pendingEvents,
  pendingText,
}: StreamFlushInput): number {
  if (pendingEvents >= STREAM_VISUAL_BATCH_LIMIT) return 0;
  if (endsAtSemanticBoundary(pendingText)) return 0;
  const targetInterval = pendingChars >= STREAM_VISUAL_TARGET_CHARS
    ? STREAM_VISUAL_MIN_INTERVAL_MS
    : STREAM_VISUAL_MAX_INTERVAL_MS;
  return Math.max(0, targetInterval - Math.max(0, elapsedMs));
}
