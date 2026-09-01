import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  AlertTriangle,
  Bell,
  Check,
  ChevronDown,
  ChevronRight,
  Clipboard,
  Code2,
  Columns2,
  ExternalLink,
  Eye,
  File,
  FileCode2,
  FileImage,
  FileText,
  Files,
  Folder,
  FolderGit2,
  FolderOpen,
  GitBranch,
  Globe2,
  ChevronsDown,
  ChevronsUp,
  Loader2,
  MoreHorizontal,
  RefreshCw,
  Save,
  Search,
  SquareTerminal,
  Trash2,
  Undo2,
  X,
} from 'lucide-react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { openPath, openUrl } from '@tauri-apps/plugin-opener';
import {
  authorizeLocalWorkspacePreview,
  discardLocalWorkspaceFile,
  getLocalWorkspaceGitDiff,
  getLocalWorkspaceGitStatus,
  listLocalWorkspaceDirectory,
  readLocalWorkspaceFile,
  scanLocalWorkspace,
  stageLocalWorkspaceFile,
  unstageLocalWorkspaceFile,
  writeLocalWorkspaceFile,
  type LocalFilePreview,
  type LocalGitChange,
  type LocalGitStatus,
  type LocalWorkspaceEntry,
  type LocalWorkspaceSnapshot,
} from '../../services/tauri';
import type { WorkspacePermissionMode } from '../../services/workspaceBinding';
import { parseWorkspaceDiff, type WorkspaceDiffDocument, type WorkspaceDiffLine } from '../../services/workspaceDiff';
import {
  localGitStatusEqual,
  reuseLocalFilePreview,
  reuseLocalGitStatus,
} from '../../services/localWorkspaceRefresh';
import {
  browserFileKind,
  isBrowserPreviewFile,
  normalizeBrowserUrl,
  shouldOpenProjectFileInBrowser,
} from '../../services/browserNavigation';
import NativeBrowserSurface from './NativeBrowserSurface';
import { usePageSurfaceActive } from '../layout/KeptAlivePage';
import LocalTerminal from './LocalTerminal';
import CodeEditor, { codeLanguageLabel } from './CodeEditor';
import MarkdownView from '../features/MarkdownView';
import HorizontalResizeHandle from '../ui/HorizontalResizeHandle';
import ResizableSplitPane from '../ui/ResizableSplitPane';

type WorkspaceView = 'files' | 'changes' | 'browser';
type BottomPanel = 'terminal' | 'problems' | 'output';

interface LocalWorkspacePanelProps {
  root: string | null;
  onChooseRoot?: () => void;
  onClearRoot?: () => void;
  permissionMode?: WorkspacePermissionMode;
  className?: string;
}

const EMPTY_GIT: LocalGitStatus = {
  isRepo: false,
  ahead: 0,
  behind: 0,
  changes: [],
};

function ancestorPaths(path: string): string[] {
  const parts = path.split('/');
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join('/'));
}

function insertDirectoryChildren(
  entries: LocalWorkspaceEntry[],
  parentPath: string,
  children: LocalWorkspaceEntry[],
): LocalWorkspaceEntry[] {
  const parentIndex = entries.findIndex((entry) => entry.path === parentPath && entry.kind === 'directory');
  if (parentIndex < 0) return entries;
  const parentDepth = entries[parentIndex].depth;
  let insertionIndex = parentIndex + 1;
  while (insertionIndex < entries.length && entries[insertionIndex].depth > parentDepth) insertionIndex += 1;
  return [...entries.slice(0, insertionIndex), ...children, ...entries.slice(insertionIndex)];
}

function formatBytes(bytes?: number): string {
  if (bytes == null) return '';
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(bytes < 10_240 ? 1 : 0)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

function isMarkdownWorkspaceFile(path: string | null | undefined): boolean {
  return Boolean(path && /\.(?:md|markdown|mdown|mkd|mmd)$/i.test(path));
}

function markdownWorkspaceContent(path: string, content: string): string {
  return /\.mmd$/i.test(path) ? `\`\`\`mermaid\n${content}\n\`\`\`` : content;
}

function statusTone(status: string): string {
  if (status.includes('?') || status.includes('A')) return 'text-[var(--success)] bg-[var(--success-subtle)]';
  if (status.includes('D')) return 'text-[var(--danger)] bg-[var(--danger-subtle)]';
  return 'text-[var(--warning)] bg-[var(--warning-subtle)]';
}

function statusLabel(status: string): string {
  if (status === '??') return 'U';
  if (status.includes('A')) return 'A';
  if (status.includes('D')) return 'D';
  if (status.includes('R')) return 'R';
  return 'M';
}

export default function LocalWorkspacePanel({
  root,
  onChooseRoot,
  onClearRoot,
  permissionMode = 'ask',
  className = '',
}: LocalWorkspacePanelProps) {
  const [tab, setTab] = useState<WorkspaceView>('files');
  const [bottomPanel, setBottomPanel] = useState<BottomPanel>('terminal');
  const [panelOpen, setPanelOpen] = useState(false);
  const [snapshot, setSnapshot] = useState<LocalWorkspaceSnapshot | null>(null);
  const [git, setGit] = useState<LocalGitStatus>(EMPTY_GIT);
  const [preview, setPreview] = useState<LocalFilePreview | null>(null);
  const [diff, setDiff] = useState<{ path?: string; content: string; truncated: boolean } | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [selectedChangePath, setSelectedChangePath] = useState<string | null>(null);
  const [openPaths, setOpenPaths] = useState<string[]>([]);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [loadedDirectories, setLoadedDirectories] = useState<Set<string>>(new Set());
  const [loadingDirectories, setLoadingDirectories] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState('');
  const [searchVisible, setSearchVisible] = useState(false);
  const [loading, setLoading] = useState(false);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [dirtyPaths, setDirtyPaths] = useState<Set<string>>(new Set());
  const [saving, setSaving] = useState(false);
  const [panelHeight, setPanelHeight] = useState(260);
  const [terminalStarted, setTerminalStarted] = useState(false);
  const [terminalSplit, setTerminalSplit] = useState(false);
  const [secondaryTerminalStarted, setSecondaryTerminalStarted] = useState(false);
  const [terminalClearToken, setTerminalClearToken] = useState(0);
  const [editorSplit, setEditorSplit] = useState(false);
  const [activeEditorGroup, setActiveEditorGroup] = useState<'primary' | 'secondary'>('primary');
  const [secondaryPath, setSecondaryPath] = useState<string | null>(null);
  const [secondaryPreview, setSecondaryPreview] = useState<LocalFilePreview | null>(null);
  const [secondaryPreviewLoading, setSecondaryPreviewLoading] = useState(false);
  const [cursor, setCursor] = useState({ line: 1, column: 1 });
  const pageActive = usePageSurfaceActive();
  const pageActiveRef = useRef(pageActive);
  pageActiveRef.current = pageActive;
  const gitRef = useRef(git);
  gitRef.current = git;
  const previewRef = useRef(preview);
  previewRef.current = preview;
  const secondaryPreviewRef = useRef(secondaryPreview);
  secondaryPreviewRef.current = secondaryPreview;
  const [previewUrl, setPreviewUrl] = useState('http://localhost:3000');
  const [previewDraft, setPreviewDraft] = useState('http://localhost:3000');
  const [previewLocalPath, setPreviewLocalPath] = useState<string | null>(null);
  const [previewKey, setPreviewKey] = useState(0);
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  const [moreOpen, setMoreOpen] = useState(false);
  const [markdownPreview, setMarkdownPreview] = useState(false);
  const [collapsedDiffHunks, setCollapsedDiffHunks] = useState<Set<string>>(new Set());
  const fullPreviewPathsRef = useRef<Set<string>>(new Set());
  const diffDocument = useMemo(() => parseWorkspaceDiff(diff?.content ?? ''), [diff?.content]);

  const loadWorkspace = useCallback(async () => {
    if (!root) return;
    setLoading(true);
    setError(null);
    try {
      const [nextSnapshot, nextGit] = await Promise.all([
        scanLocalWorkspace(root),
        getLocalWorkspaceGitStatus(root),
      ]);
      setSnapshot(nextSnapshot);
      gitRef.current = nextGit;
      setGit(nextGit);
      setCollapsed(new Set(
        nextSnapshot.entries
          .filter((entry) => entry.kind === 'directory')
          .map((entry) => entry.path)
      ));
      setLoadedDirectories(new Set());
      setLoadingDirectories(new Set());
      const firstFile = nextSnapshot.entries.find((entry) => entry.kind === 'file');
      setSelectedPath((current) => {
        const nextPath = current && nextSnapshot.entries.some((entry) => entry.path === current) ? current : firstFile?.path ?? null;
        if (nextPath) setOpenPaths((open) => open.includes(nextPath) ? open : [...open, nextPath]);
        return nextPath;
      });
      setSelectedChangePath((current) => current && nextGit.changes.some((entry) => entry.path === current) ? current : nextGit.changes[0]?.path ?? null);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : String(loadError));
      setSnapshot(null);
      setGit(EMPTY_GIT);
    } finally {
      setLoading(false);
    }
  }, [root]);

  useEffect(() => {
    setSnapshot(null);
    setPreview(null);
    setDiff(null);
    setSelectedPath(null);
    setSelectedChangePath(null);
    setOpenPaths([]);
    setEditorSplit(false);
    setActiveEditorGroup('primary');
    setSecondaryPath(null);
    setSecondaryPreview(null);
    setLoadedDirectories(new Set());
    setLoadingDirectories(new Set());
    setPanelOpen(false);
    setTerminalStarted(false);
    setTerminalSplit(false);
    setSecondaryTerminalStarted(false);
    setDrafts({});
    setDirtyPaths(new Set());
    fullPreviewPathsRef.current.clear();
    setQuery('');
    if (root) void loadWorkspace();
  }, [root, loadWorkspace]);

  useEffect(() => {
    setCursor({ line: 1, column: 1 });
    setMarkdownPreview(isMarkdownWorkspaceFile(selectedPath));
  }, [selectedPath]);

  useEffect(() => {
    setCollapsedDiffHunks(new Set());
  }, [selectedChangePath]);

  useEffect(() => {
    if (!root || !selectedPath || tab !== 'files') {
      setPreview(null);
      return;
    }
    if (dirtyPaths.has(selectedPath)) return;
    let active = true;
    const initialLoad = previewRef.current?.path !== selectedPath;
    if (initialLoad) {
      setPreview(null);
      setPreviewLoading(true);
    }
    setError(null);
    readLocalWorkspaceFile(root, selectedPath, fullPreviewPathsRef.current.has(selectedPath))
      .then((value) => {
        if (!active) return;
        setPreview((current) => reuseLocalFilePreview(current, value));
        setDrafts((current) => current[value.path] === value.content
          ? current
          : { ...current, [value.path]: value.content });
      })
      .catch((readError) => { if (active) { setPreview(null); setError(readError instanceof Error ? readError.message : String(readError)); } })
      .finally(() => { if (active) setPreviewLoading(false); });
    return () => { active = false; };
  }, [dirtyPaths, refreshEpoch, root, selectedPath, tab]);

  useEffect(() => {
    if (!editorSplit || !root || !secondaryPath || tab !== 'files') {
      setSecondaryPreview(null);
      return;
    }
    if (secondaryPath === selectedPath && preview) {
      setSecondaryPreview(preview);
      return;
    }
    if (dirtyPaths.has(secondaryPath)) return;
    let active = true;
    const initialLoad = secondaryPreviewRef.current?.path !== secondaryPath;
    if (initialLoad) setSecondaryPreviewLoading(true);
    readLocalWorkspaceFile(root, secondaryPath, fullPreviewPathsRef.current.has(secondaryPath))
      .then((value) => {
        if (!active) return;
        setSecondaryPreview((current) => reuseLocalFilePreview(current, value));
        setDrafts((current) => current[value.path] === value.content
          ? current
          : { ...current, [value.path]: value.content });
      })
      .catch((readError) => { if (active) setError(readError instanceof Error ? readError.message : String(readError)); })
      .finally(() => { if (active) setSecondaryPreviewLoading(false); });
    return () => { active = false; };
  }, [dirtyPaths, editorSplit, preview, refreshEpoch, root, secondaryPath, selectedPath, tab]);

  useEffect(() => {
    if (!root || !selectedChangePath || tab !== 'changes') {
      setDiff(null);
      return;
    }
    let active = true;
    setPreviewLoading(true);
    setError(null);
    const change = git.changes.find((item) => item.path === selectedChangePath);
    const request = change?.status === '??'
      ? readLocalWorkspaceFile(root, selectedChangePath).then((file) => ({
          path: file.path,
          content: `# Untracked file\n\n${file.content}`,
          truncated: file.truncated,
        }))
      : getLocalWorkspaceGitDiff(root, selectedChangePath);
    request
      .then((value) => { if (active) setDiff(value); })
      .catch((diffError) => { if (active) { setDiff(null); setError(diffError instanceof Error ? diffError.message : String(diffError)); } })
      .finally(() => { if (active) setPreviewLoading(false); });
    return () => { active = false; };
  }, [git.changes, root, selectedChangePath, tab]);

  useEffect(() => {
    if (!pageActive || !root || (tab !== 'files' && tab !== 'changes')) return;
    let active = true;
    const refresh = async () => {
      try {
        const nextGit = await getLocalWorkspaceGitStatus(root);
        if (!active) return;
        if (localGitStatusEqual(gitRef.current, nextGit)) return;
        gitRef.current = nextGit;
        setGit((current) => reuseLocalGitStatus(current, nextGit));
        setRefreshEpoch((current) => current + 1);
      } catch {
        // 自动刷新失败不覆盖用户当前正在查看的结果；手动刷新会显示具体错误。
      }
    };
    const interval = window.setInterval(() => void refresh(), 3_000);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, [pageActive, root, tab]);

  const visibleEntries = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (needle) {
      return snapshot?.entries.filter((entry) => entry.path.toLocaleLowerCase().includes(needle)) ?? [];
    }
    return snapshot?.entries.filter((entry) => !ancestorPaths(entry.path).some((parent) => collapsed.has(parent))) ?? [];
  }, [collapsed, query, snapshot]);

  const toggleDirectory = useCallback(async (entry: LocalWorkspaceEntry) => {
    if (!root || entry.kind !== 'directory') return;
    if (loadingDirectories.has(entry.path)) return;
    if (!collapsed.has(entry.path)) {
      setCollapsed((current) => new Set(current).add(entry.path));
      return;
    }
    if (loadedDirectories.has(entry.path)) {
      setCollapsed((current) => {
        const next = new Set(current);
        next.delete(entry.path);
        return next;
      });
      return;
    }
    setLoadingDirectories((current) => new Set(current).add(entry.path));
    setError(null);
    try {
      const children = await listLocalWorkspaceDirectory(root, entry.path);
      setSnapshot((current) => current
        ? { ...current, entries: insertDirectoryChildren(current.entries, entry.path, children) }
        : current);
      setLoadedDirectories((current) => new Set(current).add(entry.path));
      setCollapsed((current) => {
        const next = new Set(current);
        next.delete(entry.path);
        for (const child of children) if (child.kind === 'directory') next.add(child.path);
        return next;
      });
    } catch (directoryError) {
      setError(directoryError instanceof Error ? directoryError.message : String(directoryError));
    } finally {
      setLoadingDirectories((current) => {
        const next = new Set(current);
        next.delete(entry.path);
        return next;
      });
    }
  }, [collapsed, loadedDirectories, loadingDirectories, root]);

  const copyCurrent = useCallback(async () => {
    const file = activeEditorGroup === 'secondary' && editorSplit ? secondaryPreview : preview;
    const content = tab === 'files' && file ? drafts[file.path] ?? file.content : diff?.content;
    if (!content) return;
    await navigator.clipboard.writeText(content);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1_500);
  }, [activeEditorGroup, diff?.content, drafts, editorSplit, preview, secondaryPreview, tab]);

  const saveWorkspaceFile = async (file: LocalFilePreview, group: 'primary' | 'secondary') => {
    if (!root || permissionMode === 'plan') return;
    const draft = drafts[file.path] ?? file.content;
    setSaving(true);
    setError(null);
    try {
      const saved = await writeLocalWorkspaceFile(root, file.path, draft, file.fingerprint);
      if (group === 'secondary') setSecondaryPreview(saved);
      else setPreview(saved);
      setDrafts((current) => ({ ...current, [saved.path]: saved.content }));
      setDirtyPaths((current) => {
        const next = new Set(current);
        next.delete(saved.path);
        return next;
      });
      setGit(await getLocalWorkspaceGitStatus(root));
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : String(saveError));
    } finally {
      setSaving(false);
    }
  };

  const saveCurrentFile = async () => {
    const file = activeEditorGroup === 'secondary' ? secondaryPreview : preview;
    if (file) await saveWorkspaceFile(file, activeEditorGroup);
  };

  const loadFullCurrentFile = async () => {
    if (!root || !preview || !preview.truncated) return;
    setPreviewLoading(true);
    setError(null);
    try {
      const fullPreview = await readLocalWorkspaceFile(root, preview.path, true);
      fullPreviewPathsRef.current.add(preview.path);
      setPreview(fullPreview);
      setDrafts((current) => ({ ...current, [fullPreview.path]: fullPreview.content }));
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setPreviewLoading(false);
    }
  };

  const openFile = useCallback((path: string, group = activeEditorGroup) => {
    setTab('files');
    setOpenPaths((current) => current.includes(path) ? current : [...current, path]);
    if (editorSplit && group === 'secondary') setSecondaryPath(path);
    else setSelectedPath(path);
    setActiveEditorGroup(group);
  }, [activeEditorGroup, editorSplit]);

  const showLocalBrowserFile = useCallback((path: string) => {
    setTab('browser');
    setPreviewUrl(convertFileSrc(path));
    setPreviewDraft(path);
    setPreviewLocalPath(path);
    setPreviewKey((current) => current + 1);
    setError(null);
  }, []);

  const openDroppedFiles = useCallback((paths: string[]) => {
    const path = paths.find(isBrowserPreviewFile);
    if (!path) {
      setError('该文件不能在浏览器中打开');
      return;
    }
    showLocalBrowserFile(path);
  }, [showLocalBrowserFile]);

  const openProjectFile = useCallback(async (path: string) => {
    if (!root || !shouldOpenProjectFileInBrowser(path)) {
      openFile(path);
      return;
    }
    setPreviewLoading(true);
    setError(null);
    try {
      const absolutePath = await authorizeLocalWorkspacePreview(root, path);
      showLocalBrowserFile(absolutePath);
    } catch (previewError) {
      setError(previewError instanceof Error ? previewError.message : String(previewError));
    } finally {
      setPreviewLoading(false);
    }
  }, [openFile, root, showLocalBrowserFile]);

  const closeEditor = async (path: string) => {
    if (dirtyPaths.has(path)) {
      const accepted = await confirmDialog(`${path} 还有未保存的修改，仍然关闭？`, {
        title: '关闭编辑器',
        kind: 'warning',
      });
      if (!accepted) return;
    }
    setOpenPaths((current) => {
      const index = current.indexOf(path);
      const next = current.filter((item) => item !== path);
      if (selectedPath === path) setSelectedPath(next[Math.max(0, index - 1)] ?? next[0] ?? null);
      return next;
    });
    if (secondaryPath === path) {
      setSecondaryPath((current) => current === path
        ? openPaths.find((item) => item !== path && item !== selectedPath) ?? selectedPath
        : current);
    }
    setDrafts((current) => {
      const next = { ...current };
      delete next[path];
      return next;
    });
    setDirtyPaths((current) => {
      const next = new Set(current);
      next.delete(path);
      return next;
    });
  };

  const updateDraft = (file: LocalFilePreview, value: string) => {
    setDrafts((current) => ({ ...current, [file.path]: value }));
    setDirtyPaths((current) => {
      const next = new Set(current);
      if (value === file.content) next.delete(file.path);
      else next.add(file.path);
      return next;
    });
  };

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!pageActiveRef.current) return;
      const activeFile = activeEditorGroup === 'secondary' ? secondaryPreview : preview;
      const keyTarget = event.target instanceof HTMLElement ? event.target : null;
      const inCodeMirror = Boolean(keyTarget?.closest('.cm-code-editor'));
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's' && !(event.target instanceof HTMLTextAreaElement) && !inCodeMirror && tab === 'files' && activeFile && dirtyPaths.has(activeFile.path)) {
        event.preventDefault();
        void saveCurrentFile();
      }
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'p') {
        event.preventDefault();
        setTab('files');
        setSearchVisible(true);
      }
      if (event.ctrlKey && event.key === '`') {
        event.preventDefault();
        setTerminalStarted(true);
        setBottomPanel('terminal');
        setPanelOpen((open) => !open);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  });

  const changeGitState = async (action: 'stage' | 'unstage' | 'discard') => {
    if (!root || !selectedChangePath || permissionMode === 'plan') return;
    if (action === 'discard') {
      const accepted = await confirmDialog(`撤销 ${selectedChangePath} 的未提交修改？此操作不能从 SophoNote 恢复。`, {
        title: '撤销代码修改',
        kind: 'warning',
      });
      if (!accepted) return;
    }
    setLoading(true);
    setError(null);
    try {
      if (action === 'stage') await stageLocalWorkspaceFile(root, selectedChangePath);
      if (action === 'unstage') await unstageLocalWorkspaceFile(root, selectedChangePath);
      if (action === 'discard') await discardLocalWorkspaceFile(root, selectedChangePath);
      setGit(await getLocalWorkspaceGitStatus(root));
      setRefreshEpoch((current) => current + 1);
    } catch (gitError) {
      setError(gitError instanceof Error ? gitError.message : String(gitError));
    } finally {
      setLoading(false);
    }
  };

  const navigatePreview = () => {
    const value = normalizeBrowserUrl(previewDraft);
    if (!value) {
      setError('请输入有效的网址或 localhost 地址');
      return;
    }
    setPreviewUrl(value);
    setPreviewDraft(value);
    setPreviewLocalPath(null);
    setPreviewKey((current) => current + 1);
    setError(null);
  };

  if (!root) {
    return (
      <section className={`min-h-0 flex flex-col items-center justify-center bg-[var(--bg-canvas)] px-8 ${className}`}>
        <div className="max-w-sm text-center">
          <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]">
            <FolderGit2 size={21} className="text-[var(--accent)]" />
          </div>
          <h3 className="text-sm font-semibold text-[var(--text-primary)]">连接本地项目</h3>
          <p className="mt-2 text-xs leading-5 text-[var(--text-tertiary)]">
            选择目录后可编辑代码与 Markdown、检查 Git 变更，并让会话持续在该项目范围内工作。
          </p>
          {onChooseRoot && (
            <button type="button" onClick={onChooseRoot} className="btn-primary mt-4 px-3 py-1.5 text-xs">
              选择项目目录
            </button>
          )}
        </div>
      </section>
    );
  }

  const activeFilePreview = activeEditorGroup === 'secondary' && editorSplit ? secondaryPreview : preview;
  const activeContent = tab === 'files'
    ? (activeFilePreview ? drafts[activeFilePreview.path] ?? activeFilePreview.content : undefined)
    : diff?.content;
  const activePath = tab === 'files' ? activeFilePreview?.path ?? null : selectedChangePath;
  const language = codeLanguageLabel(activePath);
  const markdownFile = tab === 'files' && activeFilePreview && isMarkdownWorkspaceFile(activeFilePreview.path);
  const allDiffHunksCollapsed = diffDocument.hunks.length > 0
    && diffDocument.hunks.every((hunk) => collapsedDiffHunks.has(hunk.id));

  const toggleAllDiffHunks = () => {
    setCollapsedDiffHunks(allDiffHunksCollapsed
      ? new Set()
      : new Set(diffDocument.hunks.map((hunk) => hunk.id)));
  };

  const toggleEditorSplit = () => {
    setEditorSplit((current) => {
      if (!current) {
        setSecondaryPath(openPaths.find((path) => path !== selectedPath) ?? selectedPath);
        setActiveEditorGroup('secondary');
      } else {
        setActiveEditorGroup('primary');
      }
      return !current;
    });
  };

  const renderEditor = (
    file: LocalFilePreview | null,
    group: 'primary' | 'secondary',
    loadingState: boolean,
  ) => {
    const path = group === 'secondary' ? secondaryPath : selectedPath;
    if (loadingState) return <div className="flex h-full items-center justify-center text-xs text-[var(--text-tertiary)]"><Loader2 size={14} className="mr-2 animate-spin" />正在读取…</div>;
    if (!file) return <div className="flex h-full flex-col items-center justify-center px-6 text-center"><File size={22} className="text-[var(--text-disabled)]" /><p className="mt-2 text-xs text-[var(--text-tertiary)]">从资源管理器打开文件</p></div>;
    const isMarkdown = isMarkdownWorkspaceFile(file.path);
    return (
      <div
        className={`flex h-full min-h-0 flex-col ${editorSplit && activeEditorGroup === group ? 'ring-1 ring-inset ring-[var(--accent)]' : ''}`}
        onPointerDown={() => setActiveEditorGroup(group)}
      >
        {editorSplit && (
          <div className="flex h-7 shrink-0 items-center gap-2 border-b border-[var(--border-default)] bg-[var(--bg-sunken)] px-2 text-[11px] text-[var(--text-tertiary)]">
            <FileCode2 size={11} /><span className="min-w-0 flex-1 truncate">{path}</span>
            {dirtyPaths.has(file.path) && <span className="h-2 w-2 rounded-full bg-[var(--text-tertiary)]" />}
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-hidden">
          {isMarkdown && markdownPreview ? (
            <div className="h-full overflow-auto bg-[var(--bg-canvas)] px-6 py-5">
              <MarkdownView content={markdownWorkspaceContent(file.path, drafts[file.path] ?? file.content)} className="mx-auto max-w-4xl" copySpecialBlocks />
            </div>
          ) : (
            <CodeEditor
              key={file.path}
              value={drafts[file.path] ?? file.content}
              path={file.path}
              readOnly={permissionMode === 'plan' || file.truncated}
              onChange={(next) => updateDraft(file, next)}
              onSave={() => void saveWorkspaceFile(file, group)}
              onCursorChange={group === activeEditorGroup ? setCursor : () => {}}
            />
          )}
        </div>
      </div>
    );
  };

  return (
    <section className={`min-h-0 min-w-0 flex flex-col overflow-hidden bg-[var(--bg-canvas)] ${className}`}>
      <header className="flex h-9 shrink-0 items-center gap-2 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-2">
        <FolderOpen size={13} className="shrink-0 text-[var(--accent)]" />
        <p className="min-w-0 flex-1 truncate text-xs font-medium text-[var(--text-secondary)]" title={root}>
          {snapshot?.name ?? root}
        </p>
        <button type="button" onClick={() => void loadWorkspace()} disabled={loading} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40" title="刷新工作区">
          {loading ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
        </button>
        {onChooseRoot && <button type="button" onClick={onChooseRoot} className="h-6 rounded px-2 text-[11px] text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]">打开文件夹</button>}
      </header>

      {error && (
        <div className="flex shrink-0 items-start gap-2 border-b border-[var(--danger)] bg-[var(--danger-subtle)] px-3 py-1.5 text-xs text-[var(--danger)]">
          <AlertTriangle size={13} className="mt-0.5 shrink-0" /><span>{error}</span>
        </div>
      )}

      <div className="flex min-h-0 min-w-0 flex-1">
        <nav className="flex w-11 shrink-0 flex-col items-center border-r border-[var(--border-default)] bg-[var(--bg-sunken)] py-1" aria-label="工作区活动栏">
          <ActivityButton active={tab === 'files' && !searchVisible} label="资源管理器 ⇧⌘E" onClick={() => { setTab('files'); setSearchVisible(false); }}><Files size={18} /></ActivityButton>
          <ActivityButton active={tab === 'files' && searchVisible} label="搜索 ⇧⌘F" onClick={() => { setTab('files'); setSearchVisible(true); }}><Search size={18} /></ActivityButton>
          <ActivityButton active={tab === 'changes'} label="源代码管理" badge={git.changes.length} onClick={() => setTab('changes')}><GitBranch size={18} /></ActivityButton>
          <ActivityButton active={panelOpen && bottomPanel === 'terminal'} label="终端 ⌃`" onClick={() => { setTerminalStarted(true); setBottomPanel('terminal'); setPanelOpen((open) => bottomPanel !== 'terminal' || !open); }}><SquareTerminal size={18} /></ActivityButton>
          <ActivityButton active={tab === 'browser'} label="浏览器" onClick={() => setTab('browser')}><Globe2 size={18} /></ActivityButton>
          {onClearRoot && <div className="relative mt-auto">
            <ActivityButton active={moreOpen} label="更多" onClick={() => setMoreOpen((open) => !open)}><MoreHorizontal size={18} /></ActivityButton>
            {moreOpen && (
              <>
                <button type="button" className="fixed inset-0 z-20 cursor-default" aria-label="关闭更多工具" onClick={() => setMoreOpen(false)} />
                <div className="absolute bottom-0 left-10 z-30 w-44 rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] py-1 shadow-[var(--shadow-lg)]">
                  <button type="button" onClick={() => { setMoreOpen(false); onClearRoot(); }} className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-selected)]"><X size={13} />关闭文件夹</button>
                </div>
              </>
            )}
          </div>}
        </nav>

        {tab !== 'browser' && (
          <aside className="flex w-60 shrink-0 flex-col border-r border-[var(--border-default)] bg-[var(--bg-sunken)]">
            <div className="flex h-9 shrink-0 items-center justify-between px-3">
              <span className="text-[10px] font-semibold uppercase tracking-[0.08em] text-[var(--text-secondary)]">{tab === 'files' ? (searchVisible ? '搜索' : '资源管理器') : '源代码管理'}</span>
              <button type="button" onClick={() => void loadWorkspace()} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)]" title="刷新"><RefreshCw size={12} /></button>
            </div>
            {tab === 'files' && (
              <>
                {searchVisible && (
                  <div className="relative mx-2 mb-2">
                    <Search size={11} className="absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
                    <input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="在工作区中搜索文件（⌘P）" className="h-7 w-full rounded-sm border border-[var(--border-default)] bg-[var(--bg-canvas)] pl-6 pr-2 text-xs text-[var(--text-primary)] outline-none focus:border-[var(--accent)]" />
                  </div>
                )}
                <div className="flex h-7 shrink-0 items-center gap-1 border-y border-[var(--border-default)] px-2 text-[11px] font-semibold uppercase text-[var(--text-secondary)]">
                  <ChevronDown size={11} /><span className="truncate">{snapshot?.name ?? root}</span>
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto py-1">
                  {visibleEntries.length > 0 ? visibleEntries.map((entry) => (
                    <WorkspaceEntryRow
                      key={entry.path}
                      entry={entry}
                      selected={entry.kind === 'file' && selectedPath === entry.path}
                      collapsed={collapsed.has(entry.path)}
                      loading={loadingDirectories.has(entry.path)}
                      onClick={() => {
                        if (entry.kind === 'directory') {
                          void toggleDirectory(entry);
                        } else void openProjectFile(entry.path);
                      }}
                    />
                  )) : <p className="px-3 py-4 text-center text-xs text-[var(--text-tertiary)]">{loading ? '正在读取项目…' : '没有匹配的文件'}</p>}
                </div>
              </>
            )}
            {tab === 'changes' && (
              <div className="min-h-0 flex-1 overflow-y-auto py-1">
                {!git.isRepo ? (
                  <div className="px-4 py-5 text-center"><FolderGit2 size={18} className="mx-auto text-[var(--text-disabled)]" /><p className="mt-2 text-xs text-[var(--text-tertiary)]">当前目录不是 Git 仓库</p></div>
                ) : git.changes.length === 0 ? (
                  <div className="px-4 py-5 text-center"><Check size={18} className="mx-auto text-[var(--success)]" /><p className="mt-2 text-xs text-[var(--text-tertiary)]">工作区没有变更</p></div>
                ) : git.changes.map((change) => <ChangeRow key={`${change.status}:${change.path}`} change={change} selected={selectedChangePath === change.path} onClick={() => setSelectedChangePath(change.path)} />)}
              </div>
            )}
          </aside>
        )}

        <div className="flex min-w-0 flex-1 flex-col bg-[var(--bg-canvas)]">
          <div className="flex h-9 shrink-0 items-stretch overflow-x-auto border-b border-[var(--border-default)] bg-[var(--bg-sunken)]">
            {openPaths.map((path) => (
              <button key={path} type="button" onClick={() => openFile(path)} className={`group flex min-w-0 max-w-52 shrink-0 items-center gap-1.5 border-r border-[var(--border-default)] px-3 text-xs ${tab === 'files' && activePath === path ? 'border-t-2 border-t-[var(--accent)] bg-[var(--bg-canvas)] text-[var(--text-primary)]' : 'border-t-2 border-t-transparent text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)]'}`} title={path}>
                <FileCode2 size={12} className="shrink-0" /><span className="truncate">{path.split('/').pop()}</span>
                {dirtyPaths.has(path) ? <span className="ml-auto h-2 w-2 shrink-0 rounded-full bg-[var(--text-tertiary)]" title="未保存" /> : <span onClick={(event) => { event.stopPropagation(); void closeEditor(path); }} className="ml-auto flex h-4 w-4 shrink-0 items-center justify-center rounded opacity-0 hover:bg-[var(--bg-selected)] group-hover:opacity-100"><X size={11} /></span>}
              </button>
            ))}
            {tab === 'changes' && selectedChangePath && <div className="flex min-w-0 max-w-56 shrink-0 items-center gap-1.5 border-r border-t-2 border-t-[var(--accent)] border-[var(--border-default)] bg-[var(--bg-canvas)] px-3 text-xs text-[var(--text-primary)]"><GitBranch size={12} /><span className="truncate">{selectedChangePath} (工作树)</span></div>}
            {tab === 'browser' && <div className="flex min-w-0 max-w-56 shrink-0 items-center gap-1.5 border-r border-t-2 border-t-[var(--accent)] border-[var(--border-default)] bg-[var(--bg-canvas)] px-3 text-xs text-[var(--text-primary)]"><Globe2 size={12} /><span>浏览器</span><button type="button" onClick={() => setTab('files')} className="ml-2 rounded p-0.5 hover:bg-[var(--bg-selected)]"><X size={11} /></button></div>}
          </div>

          {tab !== 'browser' && (
            <div className="flex h-8 shrink-0 items-center gap-1 border-b border-[var(--border-default)] px-2">
              <span className="min-w-0 flex-1 truncate text-[11px] text-[var(--text-tertiary)]">{activePath ? `${snapshot?.name ?? 'workspace'} / ${activePath.split('/').join(' / ')}` : snapshot?.name}</span>
              {tab === 'files' && activeFilePreview?.truncated && activeEditorGroup === 'primary' && <button type="button" onClick={() => void loadFullCurrentFile()} className="flex h-6 items-center gap-1 rounded px-1.5 text-[11px] text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="加载完整文件"><FileText size={11} />加载完整文件</button>}
              {markdownFile && <button type="button" onClick={() => setMarkdownPreview((visible) => !visible)} className={`flex h-6 items-center gap-1 rounded px-1.5 text-[11px] hover:bg-[var(--bg-sunken)] ${markdownPreview ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)]'}`} title={markdownPreview ? '显示 Markdown 源码' : '打开 Markdown 预览'}>{markdownPreview ? <Code2 size={11} /> : <Eye size={11} />}{markdownPreview ? '源码' : '预览'}</button>}
              {tab === 'files' && <button type="button" onClick={toggleEditorSplit} className={`flex h-6 w-6 items-center justify-center rounded hover:bg-[var(--bg-sunken)] ${editorSplit ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)]'}`} title={editorSplit ? '关闭编辑器分屏' : '向右拆分编辑器'} aria-label={editorSplit ? '关闭编辑器分屏' : '向右拆分编辑器'}><Columns2 size={12} /></button>}
              {tab === 'changes' && diffDocument.hunks.length > 0 && <button type="button" onClick={toggleAllDiffHunks} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title={allDiffHunksCollapsed ? '展开全部差异' : '折叠全部差异'} aria-label={allDiffHunksCollapsed ? '展开全部差异' : '折叠全部差异'}>{allDiffHunksCollapsed ? <ChevronsDown size={12} /> : <ChevronsUp size={12} />}</button>}
              {activeContent != null && <button type="button" onClick={() => void copyCurrent()} className="flex h-6 items-center gap-1 rounded px-1.5 text-[11px] text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]">{copied ? <Check size={11} className="text-[var(--success)]" /> : <Clipboard size={11} />}{copied ? '已复制' : '复制'}</button>}
              {tab === 'files' && activeFilePreview && dirtyPaths.has(activeFilePreview.path) && <button type="button" onClick={() => { setDrafts((current) => ({ ...current, [activeFilePreview.path]: activeFilePreview.content })); setDirtyPaths((current) => { const next = new Set(current); next.delete(activeFilePreview.path); return next; }); }} className="flex h-6 items-center gap-1 rounded px-1.5 text-[11px] text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]"><Undo2 size={11} />撤销</button>}
              {tab === 'files' && activeFilePreview && <button type="button" onClick={() => void saveCurrentFile()} disabled={saving || !dirtyPaths.has(activeFilePreview.path) || permissionMode === 'plan'} className="flex h-6 items-center gap-1 rounded px-1.5 text-[11px] text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] disabled:opacity-35">{saving ? <Loader2 size={11} className="animate-spin" /> : <Save size={11} />}保存</button>}
              {tab === 'changes' && selectedChangePath && (git.changes.find((item) => item.path === selectedChangePath)?.staged ? <button type="button" onClick={() => void changeGitState('unstage')} disabled={permissionMode === 'plan'} className="h-6 rounded px-2 text-[11px] text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40">取消暂存</button> : <button type="button" onClick={() => void changeGitState('stage')} disabled={permissionMode === 'plan'} className="h-6 rounded bg-[var(--accent)] px-2 text-[11px] text-white disabled:opacity-40">暂存</button>)}
            </div>
          )}

          <div className="min-h-0 flex-1 overflow-hidden">
            {tab === 'files' ? (
              <ResizableSplitPane
                direction="horizontal"
                label="调整编辑器分屏宽度"
                enabled={editorSplit}
                first={renderEditor(preview, 'primary', previewLoading)}
                second={renderEditor(secondaryPreview, 'secondary', secondaryPreviewLoading)}
              />
            ) : previewLoading ? <div className="flex h-full items-center justify-center text-xs text-[var(--text-tertiary)]"><Loader2 size={14} className="mr-2 animate-spin" />正在读取…</div> : tab === 'browser' ? (
              <div className="flex h-full min-h-0 flex-col">
                <form className="flex h-9 shrink-0 items-center gap-2 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-2" onSubmit={(event) => { event.preventDefault(); navigatePreview(); }}>
                  <button type="button" onClick={() => setPreviewKey((current) => current + 1)} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]"><RefreshCw size={12} /></button>
                  <input value={previewDraft} onChange={(event) => setPreviewDraft(event.target.value)} className="h-7 min-w-0 flex-1 rounded-sm border border-[var(--border-default)] bg-[var(--bg-canvas)] px-2 font-mono text-xs text-[var(--text-secondary)] outline-none focus:border-[var(--accent)]" />
                  <button type="submit" className="h-7 rounded bg-[var(--accent)] px-2.5 text-xs text-white">打开</button>
                  <button type="button" onClick={() => void (previewLocalPath ? openPath(previewLocalPath) : openUrl(previewUrl))} className="flex h-7 w-7 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="在系统应用打开"><ExternalLink size={12} /></button>
                </form>
                <NativeBrowserSurface key={previewKey} url={previewUrl} onError={setError} onFileDrop={openDroppedFiles} />
              </div>
            ) : tab === 'changes' && diff ? (
              <DiffPreview document={diffDocument} collapsedHunks={collapsedDiffHunks} onToggleHunk={(id) => setCollapsedDiffHunks((current) => { const next = new Set(current); if (next.has(id)) next.delete(id); else next.add(id); return next; })} />
            ) : activeContent != null ? <div className="h-full overflow-auto"><CodePreview content={activeContent} /></div> : (
              <div className="flex h-full flex-col items-center justify-center px-6 text-center"><File size={22} className="text-[var(--text-disabled)]" /><p className="mt-2 text-xs text-[var(--text-tertiary)]">从源代码管理选择一项变更</p></div>
            )}
          </div>

          {terminalStarted && (
            <>
            {panelOpen && <HorizontalResizeHandle
              value={panelHeight}
              min={120}
              max={720}
              defaultValue={260}
              onChange={setPanelHeight}
              label="调整底部面板高度"
            />}
            <div className={`${panelOpen ? 'flex' : 'hidden'} shrink-0 flex-col bg-[var(--bg-canvas)]`} style={{ height: panelHeight, maxHeight: 'calc(100% - 120px)' }}>
              <div className="flex h-8 shrink-0 items-center gap-4 px-3 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)]">
                {(['problems', 'output', 'terminal'] as BottomPanel[]).map((panel) => <button key={panel} type="button" onClick={() => setBottomPanel(panel)} className={`h-full border-b-2 px-0.5 ${bottomPanel === panel ? 'border-[var(--accent)] text-[var(--text-primary)]' : 'border-transparent hover:text-[var(--text-secondary)]'}`}>{panel === 'problems' ? `问题 ${error ? 1 : 0}` : panel === 'output' ? '输出' : '终端'}</button>)}
                {bottomPanel === 'terminal' && <span className="normal-case font-normal text-[var(--text-disabled)]">zsh · {snapshot?.name}</span>}
                {bottomPanel === 'terminal' && <button type="button" onClick={() => { if (!terminalSplit) setSecondaryTerminalStarted(true); setTerminalSplit((current) => !current); }} className={`ml-auto flex h-6 w-6 items-center justify-center rounded hover:bg-[var(--bg-sunken)] ${terminalSplit ? 'text-[var(--accent)]' : ''}`} title={terminalSplit ? '关闭终端分屏' : '拆分终端'} aria-label={terminalSplit ? '关闭终端分屏' : '拆分终端'}><Columns2 size={12} /></button>}
                {bottomPanel === 'terminal' && <button type="button" onClick={() => setTerminalClearToken((current) => current + 1)} className="flex h-6 w-6 items-center justify-center rounded hover:bg-[var(--bg-sunken)]" title="清空终端"><Trash2 size={12} /></button>}
                <button type="button" onClick={() => setPanelOpen(false)} className={`${bottomPanel === 'terminal' ? '' : 'ml-auto'} flex h-6 w-6 items-center justify-center rounded hover:bg-[var(--bg-sunken)]`} title="关闭面板"><X size={12} /></button>
              </div>
              <div className={`${bottomPanel === 'terminal' ? 'flex' : 'hidden'} min-h-0 flex-1 overflow-hidden bg-[#15171a]`}>
                  <ResizableSplitPane
                    direction="horizontal"
                    label="调整终端分屏宽度"
                    enabled={terminalSplit}
                    first={<LocalTerminal root={root} permissionMode={permissionMode} clearToken={terminalClearToken} onError={setError} />}
                    second={secondaryTerminalStarted ? <LocalTerminal root={root} permissionMode={permissionMode} clearToken={terminalClearToken} onError={setError} /> : <div />}
                  />
              </div>
              {bottomPanel === 'problems' ? <div className="px-4 py-3 text-xs text-[var(--text-tertiary)]">{error ? <span className="text-[var(--danger)]">{error}</span> : '未检测到问题。'}</div> : bottomPanel === 'output' ? <div className="px-4 py-3 font-mono text-xs text-[var(--text-tertiary)]">{git.isRepo ? `Git · ${git.branch ?? 'HEAD'} · ${git.changes.length} 个变更` : '当前工作区没有输出。'}</div> : null}
            </div>
            </>
          )}
        </div>
      </div>

      <footer className="flex h-6 shrink-0 items-center gap-3 bg-[var(--accent)] px-2 text-[10px] text-white">
        <span className="flex items-center gap-1"><GitBranch size={10} />{git.branch ?? 'no branch'}</span>
        <span>{git.changes.length ? `↻ ${git.changes.length}` : '✓'}</span>
        <span className="min-w-0 flex-1 truncate opacity-90">{snapshot?.name ?? root}</span>
        <span>{permissionMode === 'plan' ? '只读' : permissionMode === 'autoEdit' ? '自动编辑' : '询问权限'}</span>
        {tab === 'files' && selectedPath && <><span>Ln {cursor.line}, Col {cursor.column}</span><span>UTF-8</span><span>{language}</span></>}
        <Bell size={10} />
      </footer>
    </section>
  );
}

function ActivityButton({
  active,
  label,
  badge = 0,
  onClick,
  children,
}: {
  active: boolean;
  label: string;
  badge?: number;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      aria-pressed={active}
      title={label}
      className={`relative mb-0.5 flex h-10 w-full items-center justify-center border-l-2 transition-colors ${active ? 'border-[var(--accent)] text-[var(--text-primary)]' : 'border-transparent text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'}`}
    >
      {children}
      {badge > 0 && <span className="absolute bottom-1 right-1 min-w-4 rounded-full bg-[var(--accent)] px-1 text-center text-[9px] font-semibold leading-4 text-white">{badge > 99 ? '99+' : badge}</span>}
    </button>
  );
}

function WorkspaceEntryRow({ entry, selected, collapsed, loading, onClick }: { entry: LocalWorkspaceEntry; selected: boolean; collapsed: boolean; loading: boolean; onClick: () => void }) {
  const fileKind = entry.kind === 'file' ? browserFileKind(entry.path) : 'unsupported';
  return (
    <button type="button" onClick={onClick} title={entry.path} className={`group w-full h-7 flex items-center gap-1.5 pr-2 text-left text-xs ${selected ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text-secondary)] hover:bg-[var(--bg-surface)]'}`} style={{ paddingLeft: `${8 + entry.depth * 14}px` }}>
      {entry.kind === 'directory' ? (
        <>
          {loading ? <Loader2 size={11} className="shrink-0 animate-spin text-[var(--text-tertiary)]" /> : collapsed ? <ChevronRight size={11} className="shrink-0 text-[var(--text-tertiary)]" /> : <ChevronDown size={11} className="shrink-0 text-[var(--text-tertiary)]" />}
          {collapsed ? <Folder size={13} className="shrink-0 text-[var(--gold)]" /> : <FolderOpen size={13} className="shrink-0 text-[var(--gold)]" />}
        </>
      ) : (
        <><span className="w-[11px] shrink-0" />{fileKind === 'image' ? <FileImage size={13} className="shrink-0 text-[var(--accent)]" /> : fileKind === 'pdf' ? <FileText size={13} className="shrink-0 text-[var(--danger)]" /> : <FileCode2 size={13} className="shrink-0 text-[var(--text-tertiary)]" />}</>
      )}
      <span className="min-w-0 flex-1 truncate">{entry.name}</span>
      {entry.kind === 'file' && <span className="opacity-0 group-hover:opacity-100 text-[10px] text-[var(--text-disabled)]">{formatBytes(entry.size)}</span>}
    </button>
  );
}

function ChangeRow({ change, selected, onClick }: { change: LocalGitChange; selected: boolean; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} title={change.path} className={`w-full min-h-8 flex items-center gap-2 px-2.5 py-1.5 text-left ${selected ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-surface)]'}`}>
      <span className={`h-5 w-5 shrink-0 rounded flex items-center justify-center text-[10px] font-semibold ${statusTone(change.status)}`}>{statusLabel(change.status)}</span>
      <span className={`min-w-0 flex-1 truncate text-xs ${selected ? 'text-[var(--accent)]' : 'text-[var(--text-secondary)]'}`}>{change.path}</span>
      {change.staged && <span className="text-[9px] uppercase tracking-wide text-[var(--text-tertiary)]">staged</span>}
    </button>
  );
}

function DiffPreview({
  document,
  collapsedHunks,
  onToggleHunk,
}: {
  document: WorkspaceDiffDocument;
  collapsedHunks: Set<string>;
  onToggleHunk: (id: string) => void;
}) {
  if (document.hunks.length === 0) {
    return <div className="h-full overflow-auto"><CodePreview content={document.raw} /></div>;
  }

  return (
    <div className="h-full overflow-auto bg-[var(--bg-canvas)] font-mono text-xs leading-5">
      <div className="min-w-max py-2">
        {document.chunks.map((chunk) => {
          if (chunk.kind === 'meta') {
            return (
              <div key={chunk.id} className="py-1 text-[var(--text-tertiary)]">
                {chunk.lines.map((line, index) => <div key={`${chunk.id}:${index}`} className="min-h-5 whitespace-pre px-4">{line || ' '}</div>)}
              </div>
            );
          }

          const { hunk } = chunk;
          const collapsed = collapsedHunks.has(hunk.id);
          return (
            <section key={hunk.id} className="border-y border-[var(--border-default)] first:border-t-0">
              <button type="button" onClick={() => onToggleHunk(hunk.id)} className="sticky left-0 flex h-7 min-w-full items-center gap-2 bg-[var(--bg-sunken)] px-3 text-left text-[11px] text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]" aria-expanded={!collapsed}>
                {collapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
                <span className="max-w-[72ch] truncate">{hunk.header}</span>
                <span className="ml-auto flex shrink-0 gap-2"><span className="text-[var(--success)]">+{hunk.additions}</span><span className="text-[var(--danger)]">-{hunk.deletions}</span></span>
              </button>
              {!collapsed && hunk.lines.map((line, index) => <DiffLine key={`${hunk.id}:${index}`} line={line} />)}
            </section>
          );
        })}
      </div>
    </div>
  );
}

function DiffLine({ line }: { line: WorkspaceDiffLine }) {
  const tone = line.kind === 'addition'
    ? 'bg-[var(--success-subtle)] text-[var(--text-secondary)]'
    : line.kind === 'deletion'
      ? 'bg-[var(--danger-subtle)] text-[var(--text-secondary)]'
      : line.kind === 'meta'
        ? 'text-[var(--text-tertiary)]'
        : 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]';
  const prefix = line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '-' : line.kind === 'context' ? ' ' : '';
  return (
    <div className={`grid min-h-5 grid-cols-[3rem_3rem_minmax(max-content,1fr)] ${tone}`}>
      <span className="select-none border-r border-[var(--border-default)] pr-2 text-right text-[var(--text-disabled)]">{line.oldLine ?? ''}</span>
      <span className="select-none border-r border-[var(--border-default)] pr-2 text-right text-[var(--text-disabled)]">{line.newLine ?? ''}</span>
      <code className="whitespace-pre px-3">{prefix}{line.content || ' '}</code>
    </div>
  );
}

function CodePreview({ content }: { content: string }) {
  const lines = content.split('\n');
  return (
    <div className="min-w-max py-3 font-mono text-xs leading-5 text-[var(--text-secondary)]">
      {lines.map((line, index) => (
        <div key={index} className="group flex min-h-5 hover:bg-[var(--bg-sunken)]">
          <span className="sticky left-0 w-12 shrink-0 select-none border-r border-[var(--border-default)] bg-[var(--bg-canvas)] pr-3 text-right text-[var(--text-disabled)]">{index + 1}</span>
          <code className="whitespace-pre px-4">{line || ' '}</code>
        </div>
      ))}
    </div>
  );
}
