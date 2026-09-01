/**
 * 统一变更会话纯函数单测。旧 DiffApprovalCard 已删除；状态映射与 hunk
 * 决策收敛到无 UI 副作用的共享服务。
 * 零 Tauri 依赖、零真实模型调用、不渲染组件。
 */
import { describe, it, expect } from 'vitest';
import { acceptedHunkIndexes, changePhaseFromStatus, initialHunkDecisions, resolveHunkReview } from '../../../services/changeSession';

describe('changePhaseFromStatus：持久化状态 → 变更会话阶段', () => {
  it('committed/rejected/failed/rolled_back → 对应展示态', () => {
    expect(changePhaseFromStatus('committed')).toBe('applied');
    expect(changePhaseFromStatus('rejected')).toBe('rejected');
    expect(changePhaseFromStatus('failed')).toBe('conflict');
    expect(changePhaseFromStatus('rolled_back')).toBe('conflict');
    expect(changePhaseFromStatus('undone')).toBe('undone');
  });

  it('pending_approval / proposed / prepared / 未知态 → 可交互 pending', () => {
    expect(changePhaseFromStatus('pending_approval')).toBe('proposed');
    expect(changePhaseFromStatus('proposed')).toBe('proposed');
    expect(changePhaseFromStatus('prepared')).toBe('applying');
    expect(changePhaseFromStatus('something-else')).toBe('proposed');
  });
});

describe('hunk decisions', () => {
  it('从持久化部分批准结果恢复每处决策', () => {
    expect(initialHunkDecisions(3, 'applied', [0, 2])).toEqual(['accepted', 'rejected', 'accepted']);
    expect(initialHunkDecisions(2, 'proposed')).toEqual(['pending', 'pending']);
  });

  it('只提交用户逐处接受的下标', () => {
    expect(acceptedHunkIndexes(['accepted', 'rejected', 'accepted'])).toEqual([0, 2]);
    expect(acceptedHunkIndexes(['rejected', 'rejected'])).toEqual([]);
  });

  it('批量决定只填充未确认项，不覆盖已有决定', () => {
    expect(resolveHunkReview(['rejected', 'pending', 'accepted'], 'all', 'accepted')).toMatchObject({
      decisions: ['rejected', 'accepted', 'accepted'],
      complete: true,
      acceptedHunks: [1, 2],
      requiresDraftFlush: true,
    });
  });
});

describe('resolveHunkReview：AG-32R1 草稿 flush 门禁', () => {
  it('全 × 完成审阅但不要求草稿保存（纯拒绝零写入）', () => {
    const review = resolveHunkReview(['rejected', 'pending'], 1, 'rejected');
    expect(review.complete).toBe(true);
    expect(review.acceptedHunks).toEqual([]);
    expect(review.requiresDraftFlush).toBe(false);
  });

  it('单 hunk 拒绝同样不要求草稿保存', () => {
    const review = resolveHunkReview(['pending'], 0, 'rejected');
    expect(review.complete).toBe(true);
    expect(review.requiresDraftFlush).toBe(false);
  });

  it('接受子集非空时必须先等草稿保存成功', () => {
    const review = resolveHunkReview(['pending', 'pending'], 0, 'accepted');
    expect(review.complete).toBe(false);
    expect(review.requiresDraftFlush).toBe(false);

    const completed = resolveHunkReview(['accepted', 'pending'], 1, 'rejected');
    expect(completed.complete).toBe(true);
    expect(completed.acceptedHunks).toEqual([0]);
    expect(completed.requiresDraftFlush).toBe(true);
  });

  it('审阅未完成时不进入任何提交门禁', () => {
    const review = resolveHunkReview(['pending', 'pending', 'pending'], 2, 'rejected');
    expect(review.complete).toBe(false);
    expect(review.requiresDraftFlush).toBe(false);
    expect(review.decisions).toEqual(['pending', 'pending', 'rejected']);
  });
});
