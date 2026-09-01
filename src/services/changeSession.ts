export type ChangeSessionPhase =
  | 'generating'
  | 'proposed'
  | 'applying'
  | 'applied'
  | 'rejected'
  | 'conflict'
  | 'undone';

export type HunkDecision = 'pending' | 'accepted' | 'rejected';
export type HunkDecisionTarget = number | 'all';

/** 持久化 operation 状态到统一前端状态机的唯一映射。 */
export function changePhaseFromStatus(status: string): ChangeSessionPhase {
  switch (status) {
    case 'committed':
      return 'applied';
    case 'rejected':
      return 'rejected';
    case 'failed':
    case 'rolled_back':
      return 'conflict';
    case 'undone':
      return 'undone';
    case 'generating':
      return 'generating';
    case 'prepared':
      return 'applying';
    default:
      return 'proposed';
  }
}

export function initialHunkDecisions(
  count: number,
  phase: ChangeSessionPhase,
  appliedHunks?: number[] | null
): HunkDecision[] {
  if (phase === 'rejected') return Array.from({ length: count }, () => 'rejected');
  if (phase !== 'applied' && phase !== 'undone') {
    return Array.from({ length: count }, () => 'pending');
  }
  const accepted = new Set(appliedHunks ?? Array.from({ length: count }, (_, index) => index));
  return Array.from({ length: count }, (_, index) => accepted.has(index) ? 'accepted' : 'rejected');
}

export function acceptedHunkIndexes(decisions: HunkDecision[]): number[] {
  const indexes: number[] = [];
  decisions.forEach((decision, index) => {
    if (decision === 'accepted') indexes.push(index);
  });
  return indexes;
}

export interface HunkReviewResolution {
  /** 落下本次决策后的全量决策数组 */
  decisions: HunkDecision[];
  /** 全部区域均已决定（✓ 或 ×） */
  complete: boolean;
  /** 接受子集（升序下标） */
  acceptedHunks: number[];
  /**
   * AG-32R1：是否需要先等草稿保存成功。
   * 只有接受子集非空（将进入 apply 写盘）时才依赖草稿 flush；
   * 纯拒绝路径正文零写入，不得依赖草稿保存（docs/architecture.md）。
   */
  requiresDraftFlush: boolean;
}

/**
 * 计算落下一处或全部剩余项 ✓/× 后的审阅状态。
 * `all` 只填充 pending，不覆盖用户此前逐处作出的决定。
 */
export function resolveHunkReview(
  decisions: HunkDecision[],
  target: HunkDecisionTarget,
  decision: Exclude<HunkDecision, 'pending'>
): HunkReviewResolution {
  const next = decisions.map((value, index) => (
    target === 'all'
      ? value === 'pending' ? decision : value
      : index === target ? decision : value
  ));
  const acceptedHunks = acceptedHunkIndexes(next);
  const complete = next.every((value) => value !== 'pending');
  return {
    decisions: next,
    complete,
    acceptedHunks,
    requiresDraftFlush: complete && acceptedHunks.length > 0,
  };
}
