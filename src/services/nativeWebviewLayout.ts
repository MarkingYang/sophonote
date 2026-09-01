/**
 * 原生子 WebView 与 HTML 层不同步：父页 `hidden` 后宿主矩形变为 0，
 * 若跳过 setPosition 会把上一帧矩形盖在发现/会话等当前页上。
 * 隐藏或尺寸无效时停泊到屏外 1×1，恢复后再贴回宿主。
 */

export const PARKED_WEBVIEW_POSITION = { x: -10000, y: -10000 } as const;
export const PARKED_WEBVIEW_SIZE = { width: 1, height: 1 } as const;

export interface NativeWebviewRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface NativeWebviewLayout {
  x: number;
  y: number;
  width: number;
  height: number;
  parked: boolean;
}

export function nativeWebviewLayout(
  pageActive: boolean,
  rect: NativeWebviewRect,
): NativeWebviewLayout {
  const parked = !pageActive || rect.width < 1 || rect.height < 1;
  if (parked) {
    return {
      x: PARKED_WEBVIEW_POSITION.x,
      y: PARKED_WEBVIEW_POSITION.y,
      width: PARKED_WEBVIEW_SIZE.width,
      height: PARKED_WEBVIEW_SIZE.height,
      parked: true,
    };
  }
  return {
    x: Math.round(rect.left),
    y: Math.round(rect.top),
    width: Math.max(1, Math.round(rect.width)),
    height: Math.max(1, Math.round(rect.height)),
    parked: false,
  };
}
