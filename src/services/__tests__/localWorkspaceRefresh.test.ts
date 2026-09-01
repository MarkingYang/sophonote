import { describe, expect, it } from 'vitest';
import type { LocalFilePreview, LocalGitStatus } from '../tauri';
import { reuseLocalFilePreview, reuseLocalGitStatus } from '../localWorkspaceRefresh';

const cleanGitStatus: LocalGitStatus = {
  isRepo: true,
  branch: 'main',
  ahead: 0,
  behind: 0,
  changes: [],
};

const preview: LocalFilePreview = {
  path: 'README.md',
  content: '# SophoNote',
  size: 9,
  truncated: false,
  fingerprint: 'same-content',
};

describe('local workspace background refresh', () => {
  it('reuses the current Git state when a poll returns the same snapshot', () => {
    const next = { ...cleanGitStatus, changes: [] };
    expect(reuseLocalGitStatus(cleanGitStatus, next)).toBe(cleanGitStatus);
  });

  it('publishes a changed Git state', () => {
    const next = {
      ...cleanGitStatus,
      changes: [{ path: 'README.md', status: ' M', staged: false }],
    };
    expect(reuseLocalGitStatus(cleanGitStatus, next)).toBe(next);
  });

  it('keeps the rendered file instance when its fingerprint is unchanged', () => {
    const next = { ...preview };
    expect(reuseLocalFilePreview(preview, next)).toBe(preview);
  });

  it('publishes an externally changed file', () => {
    const next = { ...preview, content: '# Updated', fingerprint: 'new-content' };
    expect(reuseLocalFilePreview(preview, next)).toBe(next);
  });
});
