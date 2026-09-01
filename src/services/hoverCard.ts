/**
 * NB-09 悬停预览卡定位纯函数（供 WikiHoverCard.tsx 使用，node 可独立断言）。
 * 规则：
 * - 链接下方放不下卡片（rectBottom + CARD_MAX_H > 视口高）且上方放得下（rectTop > CARD_MAX_H）
 *   → 向上弹出（above=true，y 为卡片底边锚点），否则向下（y 为顶边锚点）；
 * - 左边距 clamp 到 [8, 视口宽 - CARD_W - 8]，防卡片溢出右缘。
 */

export const CARD_W = 360;
export const CARD_MAX_H = 300;

export interface CardPos {
  left: number;
  /** above=true 时以此为卡片底边锚点（向上弹出），否则为顶边锚点 */
  y: number;
  above: boolean;
}

export function calcCardPos(opts: {
  rectTop: number;
  rectBottom: number;
  rectLeft: number;
  viewW: number;
  viewH: number;
}): CardPos {
  const { rectTop, rectBottom, rectLeft, viewW, viewH } = opts;
  const above = rectBottom + CARD_MAX_H > viewH && rectTop > CARD_MAX_H;
  const left = Math.max(8, Math.min(rectLeft, viewW - CARD_W - 8));
  return { left, y: above ? rectTop - 6 : rectBottom + 6, above };
}
