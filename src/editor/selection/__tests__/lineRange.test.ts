import { describe, expect, it } from 'vitest';
import { blockIndexAtLine, selectionLineRange, topLevelBlockStartLines } from '../lineRange';

// AG-31：chip 行号来源（best-effort 映射；未命中回落摘录展示）
describe('selectionLineRange', () => {
  const src = '# 标题\n\n引言段落。\n\n数字 3.14 与 42 要保留。\n\n结尾。';

  it('精确命中单段 → 单行区间', () => {
    expect(selectionLineRange(src, '数字 3.14 与 42 要保留。')).toEqual([5, 5]);
  });

  it('多段选区 → 跨行区间（含中间空行）', () => {
    expect(selectionLineRange(src, '引言段落。\n\n数字 3.14 与 42 要保留。')).toEqual([3, 5]);
  });

  it('选区首尾空行容忍', () => {
    expect(selectionLineRange(src, '\n引言段落。\n')).toEqual([3, 3]);
  });

  it('未命中 → null（UI 回落摘录）', () => {
    expect(selectionLineRange(src, '不存在的文本')).toBeNull();
  });

  it('同首行不同尾不误落', () => {
    const dup = '重复行\n甲\n重复行\n乙';
    expect(selectionLineRange(dup, '重复行\n甲')).toEqual([1, 2]);
    expect(selectionLineRange(dup, '重复行\n不配')).toBeNull();
  });
});

// AG-32：内联建议块锚点（源码行号 → ProseMirror 顶层块下标）
describe('blockIndexAtLine / topLevelBlockStartLines', () => {
  const src = '# T\n\n段落一\n\n段落二\n\n- 列表';

  it('行号到块下标', () => {
    expect(blockIndexAtLine(src, 1)).toBe(0);
    expect(blockIndexAtLine(src, 3)).toBe(1);
    expect(blockIndexAtLine(src, 5)).toBe(2);
    expect(blockIndexAtLine(src, 7)).toBe(3);
  });

  it('块间空行归入前一块', () => {
    expect(blockIndexAtLine(src, 4)).toBe(1);
  });

  it('围栏内空行不切块', () => {
    const fenced = '前言\n\n```\na\n\nb\n```\n\n后记';
    expect(topLevelBlockStartLines(fenced)).toEqual([0, 2, 8]);
    expect(blockIndexAtLine(fenced, 5)).toBe(1);
  });

  it('空文档 -1', () => {
    expect(blockIndexAtLine('', 3)).toBe(-1);
  });
});
