import { describe, expect, it } from 'vitest';
import { nativeWebviewLayout, PARKED_WEBVIEW_POSITION, PARKED_WEBVIEW_SIZE } from '../nativeWebviewLayout';
import { freezeWhenInactive, mountedPageIds, rememberHeavyPage } from '../pageKeepalive';

describe('NEXT-004 重型页 LRU 保活', () => {
  it('非重型页不进入保活列表', () => {
    expect(rememberHeavyPage([], 'discover')).toEqual([]);
    expect(rememberHeavyPage(['notes'], 'discover')).toEqual(['notes']);
    expect(rememberHeavyPage(['notes', 'ai-studio'], 'conversation')).toEqual(['notes', 'ai-studio']);
  });

  it('访问笔记本/工作室后保留，最近者在前，最多两个', () => {
    let kept = rememberHeavyPage([], 'notes');
    expect(kept).toEqual(['notes']);
    kept = rememberHeavyPage(kept, 'ai-studio');
    expect(kept).toEqual(['ai-studio', 'notes']);
    kept = rememberHeavyPage(kept, 'notes');
    expect(kept).toEqual(['notes', 'ai-studio']);
  });

  it('挂载集合 = 当前页 + 保活重型页，当前页在前且去重', () => {
    expect(mountedPageIds('discover', ['notes', 'ai-studio'])).toEqual([
      'discover',
      'notes',
      'ai-studio',
    ]);
    expect(mountedPageIds('notes', ['notes', 'ai-studio'])).toEqual(['notes', 'ai-studio']);
  });

  it('隐藏态订阅视为相等，避免后台页因 store 更新重渲染', () => {
    const eq = freezeWhenInactive(false);
    expect(eq({ n: 1 }, { n: 2 })).toBe(true);
    const live = freezeWhenInactive(true);
    expect(live(1, 1)).toBe(true);
    expect(live(1, 2)).toBe(false);
  });
});

describe('NEXT-004 原生子 WebView 停泊', () => {
  const visible = { left: 120.4, top: 80.6, width: 640.2, height: 480.8 };

  it('可见且页 active 时贴宿主矩形', () => {
    expect(nativeWebviewLayout(true, visible)).toEqual({
      x: 120,
      y: 81,
      width: 640,
      height: 481,
      parked: false,
    });
  });

  it('页隐藏或宿主尺寸为 0 时停泊屏外，不沿用上一帧', () => {
    expect(nativeWebviewLayout(false, visible)).toEqual({
      x: PARKED_WEBVIEW_POSITION.x,
      y: PARKED_WEBVIEW_POSITION.y,
      width: PARKED_WEBVIEW_SIZE.width,
      height: PARKED_WEBVIEW_SIZE.height,
      parked: true,
    });
    expect(nativeWebviewLayout(true, { ...visible, width: 0, height: 0 }).parked).toBe(true);
  });
});
