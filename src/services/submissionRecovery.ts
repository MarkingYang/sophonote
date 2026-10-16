import { cancelSubmission, findSubmission, getSubmission, submissionTerminal, type PendingSubmission, type SubmissionSnapshot } from './agentSubmission';
/** Bound polling. Timeout retains identity and lock; it never means execution failed. */
export async function observeSubmission(
  read: () => PendingSubmission | undefined,
  write: (pending: PendingSubmission) => void,
): Promise<SubmissionSnapshot | null> {
  for (let attempt = 0; attempt < 130; attempt += 1) {
    const pending = read();
    if (!pending) return null;
    try {
      let snapshot = pending.snapshot?.submissionId
        ? await getSubmission(pending.snapshot.submissionId)
        : await findSubmission(pending.clientRequestId);
      if (read()?.cancelRequested && !snapshot.cancelRequested && !submissionTerminal(snapshot.state)) {
        snapshot = await cancelSubmission(snapshot.submissionId);
      }
      const latest = read();
      if (!latest || latest.clientRequestId !== pending.clientRequestId) return null;
      write({ ...latest, snapshot, error: undefined });
      if (snapshot.runId || submissionTerminal(snapshot.state)) return snapshot;
    } catch {
      const latest = read();
      if (latest) write({ ...latest, error: '提交状态尚未确认，正在查询；不会重复发送' });
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  return null;
}
