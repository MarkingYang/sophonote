import type { LocalFilePreview, LocalGitStatus } from './tauri';

export function localGitStatusEqual(left: LocalGitStatus, right: LocalGitStatus): boolean {
  return left.isRepo === right.isRepo
    && left.branch === right.branch
    && left.ahead === right.ahead
    && left.behind === right.behind
    && left.changes.length === right.changes.length
    && left.changes.every((change, index) => {
      const other = right.changes[index];
      return change.path === other.path
        && change.status === other.status
        && change.staged === other.staged;
    });
}

export function reuseLocalGitStatus(
  current: LocalGitStatus,
  next: LocalGitStatus,
): LocalGitStatus {
  return localGitStatusEqual(current, next) ? current : next;
}

export function reuseLocalFilePreview(
  current: LocalFilePreview | null,
  next: LocalFilePreview,
): LocalFilePreview {
  return current?.path === next.path
    && current.fingerprint === next.fingerprint
    && current.truncated === next.truncated
    ? current
    : next;
}
