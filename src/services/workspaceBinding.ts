import {
  getLocalWorkspaceGitStatus,
  getSetting,
  scanLocalWorkspace,
  updateSetting,
} from './tauri';

export type WorkspacePermissionMode = 'ask' | 'autoEdit' | 'plan';

export interface WorkspaceBinding {
  version: 1;
  root: string;
  name: string;
  kind: 'folder' | 'git';
  permissionMode: WorkspacePermissionMode;
  authorizedAt: string;
}

export interface WorkspaceScopeSnapshot {
  root: string;
  name: string;
  kind: WorkspaceBinding['kind'];
  branch: string | null;
  permissionMode: WorkspacePermissionMode;
  capturedAt: string;
}

export const THREAD_WORKSPACE_KEY_PREFIX = 'ui:thread-workspace:';
export const PROJECT_WORKSPACE_KEY_PREFIX = 'ui:project-workspace:';

function defaultBinding(root: string): WorkspaceBinding {
  return {
    version: 1,
    root,
    name: root.split(/[\\/]/).filter(Boolean).pop() || root,
    kind: 'folder',
    permissionMode: 'ask',
    authorizedAt: new Date().toISOString(),
  };
}

export function parseWorkspaceBinding(raw: string): WorkspaceBinding | null {
  const value = raw.trim();
  if (!value) return null;
  if (!value.startsWith('{')) return defaultBinding(value);
  try {
    const parsed = JSON.parse(value) as Partial<WorkspaceBinding>;
    if (typeof parsed.root !== 'string' || !parsed.root.trim()) return null;
    const fallback = defaultBinding(parsed.root);
    return {
      ...fallback,
      ...parsed,
      version: 1,
      permissionMode: parsed.permissionMode === 'autoEdit' || parsed.permissionMode === 'plan'
        ? parsed.permissionMode
        : 'ask',
      kind: parsed.kind === 'git' ? 'git' : 'folder',
    };
  } catch {
    return defaultBinding(value);
  }
}

const bindingMemo = new Map<string, WorkspaceBinding | null>();

/** 未读过为 undefined；已读无绑定为 null。热切换用快照画首帧，避免先清空再异步填。 */
export function peekWorkspaceBinding(key: string): WorkspaceBinding | null | undefined {
  return bindingMemo.has(key) ? bindingMemo.get(key) : undefined;
}

export function rememberWorkspaceBinding(key: string, binding: WorkspaceBinding | null): void {
  bindingMemo.set(key, binding);
}

export async function loadWorkspaceBinding(key: string): Promise<WorkspaceBinding | null> {
  try {
    const parsed = parseWorkspaceBinding(await getSetting(key));
    bindingMemo.set(key, parsed);
    return parsed;
  } catch {
    bindingMemo.set(key, null);
    return null;
  }
}

export async function authorizeWorkspace(root: string): Promise<WorkspaceBinding> {
  const [workspace, git] = await Promise.all([
    scanLocalWorkspace(root),
    getLocalWorkspaceGitStatus(root),
  ]);
  return {
    version: 1,
    root: workspace.root,
    name: workspace.name,
    kind: git.isRepo ? 'git' : 'folder',
    permissionMode: 'ask',
    authorizedAt: new Date().toISOString(),
  };
}

export async function saveWorkspaceBinding(key: string, binding: WorkspaceBinding | null): Promise<void> {
  bindingMemo.set(key, binding);
  await updateSetting(key, binding ? JSON.stringify(binding) : '');
}

export function withWorkspacePermission(
  binding: WorkspaceBinding,
  permissionMode: WorkspacePermissionMode,
): WorkspaceBinding {
  return { ...binding, permissionMode };
}

export async function captureScopeSnapshot(binding: WorkspaceBinding): Promise<WorkspaceScopeSnapshot> {
  const git = await getLocalWorkspaceGitStatus(binding.root);
  return {
    root: binding.root,
    name: binding.name,
    kind: binding.kind,
    branch: git.branch ?? null,
    permissionMode: binding.permissionMode,
    capturedAt: new Date().toISOString(),
  };
}
