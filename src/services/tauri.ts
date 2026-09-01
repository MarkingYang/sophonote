import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Item, Source, DailyLog, Task, PomodoroSession, DailyPick, DiscoverCategory, PickedRef, Project } from '../types';
// NB-33：类型复用（type-only import 编译期擦除，与下方值导入不构成运行时环）
import type { InlineCompletionRequest } from './inlineCompletion';

interface ApiResponse<T> {
  success: boolean;
  data?: T;
  error?: string;
}

// ==================== 数据库 API ====================

export async function initDatabase(): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_init');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function getItems(params: {
  sourceId?: string;
  itemType?: string;
  status?: string;
  limit?: number;
  query?: string;
  offset?: number;
  excludeArchived?: boolean;
} = {}): Promise<Item[]> {
  const res = await invoke<ApiResponse<Item[]>>('db_get_items', {
    sourceId: params.sourceId,
    itemType: params.itemType,
    status: params.status,
    limit: params.limit,
    query: params.query,
    offset: params.offset,
    excludeArchived: params.excludeArchived,
  });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function insertItem(item: Item): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_insert_item', { item });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function updateItemStatus(id: string, status: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_item_status', { id, status });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function deleteItem(id: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_delete_item', { id });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function getSources(): Promise<Source[]> {
  const res = await invoke<ApiResponse<Source[]>>('db_get_sources');
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function toggleSource(id: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_toggle_source', { id });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function updateSourceInterval(id: string, minutes: number): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_source_interval', { id, minutes });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function updateSourceDiscoveryConfig(
  id: string,
  generationPrompt: string,
  scoringRule: string,
  minScore: number,
): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_source_discovery_config', {
    id,
    generationPrompt,
    scoringRule,
    minScore,
  });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 信源分层调整：core | standard | experimental（借鉴 ai-news-radar source_tier） */
export async function updateSourceTier(id: string, tier: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_source_tier', { id, tier });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 信源准入状态调整：active | probation（试用观察期）| skipped（高风险跳过） */
export async function updateSourceAdmission(id: string, admission: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_source_admission', { id, admission });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function updateItemAI(id: string, summary: string, tags: string[], promptVersion?: string, enrichJson?: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_item_ai', {
    id,
    summary,
    tags: tags.join(','),
    promptVersion: promptVersion ?? null,
    enrichJson: enrichJson ?? null,
  });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 读取速览结构化结果 JSON（阅读视图展示关键点/证据/风险/置信度） */
export async function getItemEnrich(id: string): Promise<string | null> {
  const res = await invoke<ApiResponse<string | null>>('db_get_item_enrich', { id });
  if (!res.success) throw new Error(res.error);
  return res.data ?? null;
}

export async function insertLog(log: DailyLog): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_insert_log', { log });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function getLogs(logType?: string): Promise<DailyLog[]> {
  const res = await invoke<ApiResponse<DailyLog[]>>('db_get_logs', { logType });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function getTasks(status?: string): Promise<Task[]> {
  const res = await invoke<ApiResponse<Task[]>>('db_get_tasks', { status });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function insertTask(task: Task): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_insert_task', { task });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function deleteTask(id: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_delete_task', { id });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function insertPomodoroSession(session: PomodoroSession): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_insert_pomodoro_session', { session });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function listPomodoroSessions(since?: string): Promise<PomodoroSession[]> {
  const res = await invoke<ApiResponse<PomodoroSession[]>>('db_list_pomodoro_sessions', { since });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function getStats(): Promise<{
  totalItems: number;
  unreadItems: number;
  starredItems: number;
  totalTasks: number;
  pendingTasks: number;
  totalLogs: number;
}> {
  const res = await invoke<ApiResponse<Record<string, number>>>('db_get_stats');
  if (!res.success) throw new Error(res.error);
  const data = res.data || {};
  return {
    totalItems: data.total_items || 0,
    unreadItems: data.unread_items || 0,
    starredItems: data.starred_items || 0,
    totalTasks: data.total_tasks || 0,
    pendingTasks: data.pending_tasks || 0,
    totalLogs: data.total_logs || 0,
  };
}

// ==================== 通知 API ====================

export async function sendNotification(title: string, body: string): Promise<void> {
  try {
    const { sendNotification: notify } = await import('@tauri-apps/plugin-notification');
    await notify({ title, body });
  } catch {
    if ('Notification' in window && Notification.permission === 'granted') {
      new Notification(title, { body });
    }
  }
}

export async function requestNotificationPermission(): Promise<boolean> {
  try {
    const { isPermissionGranted, requestPermission } = await import('@tauri-apps/plugin-notification');
    let granted = await isPermissionGranted();
    if (!granted) {
      const result = await requestPermission();
      granted = result === 'granted';
    }
    return granted;
  } catch {
    if ('Notification' in window) {
      const result = await Notification.requestPermission();
      return result === 'granted';
    }
    return false;
  }
}

// ==================== 系统 API ====================

export async function getAppVersion(): Promise<string> {
  return await invoke<string>('get_app_version');
}

export async function getDataDir(): Promise<string> {
  const res = await invoke<ApiResponse<string>>('get_data_dir');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export interface StorageLayoutInfo {
  root: string;
  database: string;
  notes: string;
  workspace: string;
  hermes: string;
  runtime: string;
  logs: string;
  version: string;
  migrationRequired: boolean;
}

export async function getStorageLayout(): Promise<StorageLayoutInfo> {
  const res = await invoke<ApiResponse<StorageLayoutInfo>>('get_storage_layout');
  if (!res.success || !res.data) throw new Error(res.error ?? 'get_storage_layout failed');
  return res.data;
}

export interface LocalWorkspaceEntry {
  path: string;
  name: string;
  kind: 'file' | 'directory';
  depth: number;
  size?: number;
}

export interface LocalWorkspaceSnapshot {
  root: string;
  name: string;
  entries: LocalWorkspaceEntry[];
  truncated: boolean;
}

export interface LocalFilePreview {
  path: string;
  content: string;
  size: number;
  truncated: boolean;
  fingerprint: string;
}

export interface LocalCommandResult {
  command: string;
  output: string;
  exitCode?: number;
  timedOut: boolean;
  truncated: boolean;
}

export interface LocalGitChange {
  path: string;
  status: string;
  staged: boolean;
}

export interface LocalGitStatus {
  isRepo: boolean;
  branch?: string;
  ahead: number;
  behind: number;
  changes: LocalGitChange[];
}

export interface LocalGitDiff {
  path?: string;
  content: string;
  truncated: boolean;
}

export async function scanLocalWorkspace(root: string): Promise<LocalWorkspaceSnapshot> {
  const res = await invoke<ApiResponse<LocalWorkspaceSnapshot>>('local_workspace_scan', { root });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法读取本地项目');
  return res.data;
}

export async function listLocalWorkspaceDirectory(root: string, relativePath: string): Promise<LocalWorkspaceEntry[]> {
  const res = await invoke<ApiResponse<LocalWorkspaceEntry[]>>('local_workspace_list_directory', { root, relativePath });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法读取目录');
  return res.data;
}

export async function readLocalWorkspaceFile(root: string, relativePath: string, full = false): Promise<LocalFilePreview> {
  const res = await invoke<ApiResponse<LocalFilePreview>>('local_workspace_read', { root, relativePath, full });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法预览文件');
  return res.data;
}

export async function authorizeLocalWorkspacePreview(root: string, relativePath: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('local_workspace_preview_file', { root, relativePath });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法在浏览器中打开文件');
  return res.data;
}

export async function writeLocalWorkspaceFile(
  root: string,
  relativePath: string,
  content: string,
  expectedFingerprint: string,
): Promise<LocalFilePreview> {
  const res = await invoke<ApiResponse<LocalFilePreview>>('local_workspace_write', {
    root,
    relativePath,
    content,
    expectedFingerprint,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法保存文件');
  return res.data;
}

export async function getLocalWorkspaceGitStatus(root: string): Promise<LocalGitStatus> {
  const res = await invoke<ApiResponse<LocalGitStatus>>('local_workspace_git_status', { root });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法读取 Git 状态');
  return res.data;
}

export async function getLocalWorkspaceGitDiff(root: string, relativePath?: string): Promise<LocalGitDiff> {
  const res = await invoke<ApiResponse<LocalGitDiff>>('local_workspace_git_diff', { root, relativePath });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法读取 Git 变更');
  return res.data;
}

async function localWorkspaceGitFileAction(command: string, root: string, relativePath: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>(command, { root, relativePath });
  if (!res.success) throw new Error(res.error ?? 'Git 操作失败');
}

export const stageLocalWorkspaceFile = (root: string, relativePath: string) =>
  localWorkspaceGitFileAction('local_workspace_git_stage', root, relativePath);

export const unstageLocalWorkspaceFile = (root: string, relativePath: string) =>
  localWorkspaceGitFileAction('local_workspace_git_unstage', root, relativePath);

export const discardLocalWorkspaceFile = (root: string, relativePath: string) =>
  localWorkspaceGitFileAction('local_workspace_git_discard', root, relativePath);

export async function runLocalWorkspaceCommand(root: string, command: string): Promise<LocalCommandResult> {
  const res = await invoke<ApiResponse<LocalCommandResult>>('local_workspace_run_command', { root, command });
  if (!res.success || !res.data) throw new Error(res.error ?? '命令执行失败');
  return res.data;
}

export interface LocalTerminalOutput {
  sessionId: string;
  data: string;
}

export interface LocalTerminalExit {
  sessionId: string;
}

export async function createLocalTerminal(root: string, cols = 80, rows = 24): Promise<string> {
  const res = await invoke<ApiResponse<string>>('local_terminal_create', { root, cols, rows });
  if (!res.success || !res.data) throw new Error(res.error ?? '无法创建终端');
  return res.data;
}

export async function writeLocalTerminal(sessionId: string, data: Uint8Array): Promise<void> {
  let binary = '';
  for (let offset = 0; offset < data.length; offset += 0x8000) {
    binary += String.fromCharCode(...data.subarray(offset, offset + 0x8000));
  }
  const res = await invoke<ApiResponse<null>>('local_terminal_write', {
    sessionId,
    data: btoa(binary),
  });
  if (!res.success) throw new Error(res.error ?? '无法写入终端');
}

export async function resizeLocalTerminal(sessionId: string, cols: number, rows: number): Promise<void> {
  const res = await invoke<ApiResponse<null>>('local_terminal_resize', { sessionId, cols, rows });
  if (!res.success) throw new Error(res.error ?? '无法调整终端大小');
}

export async function closeLocalTerminal(sessionId: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('local_terminal_close', { sessionId });
  if (!res.success) throw new Error(res.error ?? '无法关闭终端');
}

export function listenLocalTerminalOutput(
  callback: (payload: LocalTerminalOutput) => void,
): Promise<UnlistenFn> {
  return listen<LocalTerminalOutput>('sophonote:terminal-output', (event) => callback(event.payload));
}

export function listenLocalTerminalExit(
  callback: (payload: LocalTerminalExit) => void,
): Promise<UnlistenFn> {
  return listen<LocalTerminalExit>('sophonote:terminal-exit', (event) => callback(event.payload));
}

export interface AppUpdateInfo {
  available: boolean;
  currentVersion: string;
  version?: string;
  notes?: string;
  date?: string;
}

export async function checkAppUpdate(): Promise<AppUpdateInfo> {
  const res = await invoke<ApiResponse<AppUpdateInfo>>('app_update_check');
  if (!res.success || !res.data) throw new Error(res.error ?? 'app_update_check failed');
  return res.data;
}

export async function installAppUpdate(): Promise<void> {
  const res = await invoke<ApiResponse<string>>('app_update_install');
  if (!res.success) throw new Error(res.error ?? 'app_update_install failed');
}

export interface HermesSidecarStatus {
  currentVersion: string;
  currentCommit: string;
  currentSource: 'bundled' | 'official-update' | string;
  pendingVersion?: string;
  pendingCommit?: string;
  updateReady: boolean;
  repository: string;
}

export type HermesSidecarUpdatePhase =
  | 'checking'
  | 'downloading'
  | 'unpacking'
  | 'copying'
  | 'installing'
  | 'verifying'
  | 'signing'
  | 'hashing'
  | 'staging';

export interface HermesSidecarProgress {
  operationId: string;
  phase: HermesSidecarUpdatePhase;
  state: 'running' | 'completed' | 'failed';
  percent: number;
  message: string;
  bytesDownloaded: number | null;
  totalBytes: number | null;
}

export function listenHermesSidecarProgress(
  callback: (progress: HermesSidecarProgress) => void,
): Promise<UnlistenFn> {
  return listen<HermesSidecarProgress>(
    'sophonote:hermes-sidecar-update-progress',
    (event) => callback(event.payload),
  );
}

export async function getHermesSidecarStatus(): Promise<HermesSidecarStatus> {
  const res = await invoke<ApiResponse<HermesSidecarStatus>>('hermes_sidecar_status');
  if (!res.success || !res.data) throw new Error(res.error ?? 'hermes_sidecar_status failed');
  return res.data;
}

export async function pullHermesSidecar(): Promise<HermesSidecarStatus> {
  const res = await invoke<ApiResponse<HermesSidecarStatus>>('hermes_sidecar_pull');
  if (!res.success || !res.data) throw new Error(res.error ?? 'hermes_sidecar_pull failed');
  return res.data;
}

// ==================== 设置 API ====================

export async function updateSetting(key: string, value: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('update_setting', { key, value });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function getSetting(key: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('get_setting', { key });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ==================== AG-30 行内补全（独立 CompletionService 轻量路径，不建 Thread/Run） ====================
// 契约：docs/architecture.md（camelCase 与 Rust serde 对齐）。
// 关闭/未配置/超时/被过滤时 Rust 一律返回 ok + 空 text + finishReason，前端静默不展示、不弹错。

export interface CompletionSuggestResponse {
  requestId: string;
  articleId: string;
  documentVersion: number;
  anchorHash: string;
  text: string;
  finishReason: 'complete' | 'timeout' | 'filtered';
  model: string;
  latencyMs: number;
}

export async function completionSuggest(request: InlineCompletionRequest): Promise<CompletionSuggestResponse> {
  const res = await invoke<ApiResponse<CompletionSuggestResponse>>('completion_suggest', { request });
  if (!res.success || !res.data) throw new Error(res.error ?? 'completion_suggest failed');
  return res.data;
}

/** 取消传播（Rust CancellationToken 注册表）；请求已完成时返回 false，无副作用 */
export async function completionCancel(requestId: string): Promise<boolean> {
  const res = await invoke<ApiResponse<boolean>>('completion_cancel', { requestId });
  if (!res.success) throw new Error(res.error ?? 'completion_cancel failed');
  return res.data ?? false;
}

/** 接受/拒绝反馈（§4.5 聚合计数：只报布尔，不带任何内容） */
export async function completionReportFeedback(accepted: boolean): Promise<boolean> {
  const res = await invoke<ApiResponse<boolean>>('completion_report_feedback', { accepted });
  if (!res.success) throw new Error(res.error ?? 'completion_report_feedback failed');
  return res.data ?? false;
}

/** settings 键 completion_config（与 Rust completion::load_config 同一口径）；缺失/损坏 → enabled=true（同 Rust 兜底） */
export interface CompletionConfigShape {
  enabled: boolean;
  model?: string;
  timeoutMs?: number;
}

export async function getCompletionConfig(): Promise<CompletionConfigShape> {
  let raw = '';
  try {
    raw = await getSetting('completion_config');
  } catch {
    return { enabled: true };
  }
  try {
    const parsed = JSON.parse(raw) as Partial<CompletionConfigShape>;
    return { ...parsed, enabled: parsed.enabled !== false };
  } catch {
    return { enabled: true };
  }
}

/** 总开关写入：读旧值合并后整体写回（保留 model/timeoutMs 不被覆盖） */
export async function setCompletionEnabled(enabled: boolean): Promise<void> {
  const current = await getCompletionConfig();
  await updateSetting('completion_config', JSON.stringify({ ...current, enabled }));
}

// ==================== 钥匙串 API（API Key 安全存储） ====================

export async function saveApiKey(provider: string, apiKey: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('keychain_save_api_key', { provider, apiKey });
  if (!res.success) throw new Error(res.error);
  return res.data ?? 'saved:keychain';
}

export async function hasApiKey(provider: string): Promise<boolean> {
  const res = await invoke<ApiResponse<string>>('keychain_get_api_key', { provider });
  if (!res.success) {
    if (res.error === 'not_found') return false;
    throw new Error(res.error);
  }
  return res.data === 'configured';
}

export async function deleteApiKey(provider: string): Promise<void> {
  const res = await invoke<ApiResponse<string>>('keychain_delete_api_key', { provider });
  if (!res.success) throw new Error(res.error);
}

// ==================== 文章（深度解读） API ====================

export async function getArticles(limit?: number): Promise<import('../types').Article[]> {
  const res = await invoke<ApiResponse<import('../types').Article[]>>('db_get_articles', { limit });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 按条目精确读取最新深度解读；不受笔记列表 limit 影响。 */
export async function getDeepDiveByItem(itemId: string): Promise<import('../types').Article | null> {
  const res = await invoke<ApiResponse<import('../types').Article | null>>('db_get_deep_dive_by_item', { itemId });
  if (!res.success) throw new Error(res.error);
  return res.data ?? null;
}

export async function insertArticle(article: import('../types').Article): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_insert_article', { article });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// blocks_json（BlockSuite 快照）编辑模式已废弃：固定传 null，
// Rust 侧 COALESCE(?3, blocks_json) 保留历史快照不丢数据。
export async function updateArticle(id: string, content: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_update_article', { id, content, blocksJson: null });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function renameArticle(id: string, title: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_rename_article', { id, title });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function deleteArticle(id: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_delete_article', { id });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 单事务批量删除笔记，返回实际删除的笔记 ID。 */
export async function deleteArticles(ids: string[]): Promise<string[]> {
  const res = await invoke<ApiResponse<string[]>>('db_delete_articles', { ids });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

// ==================== 笔记资产（N0：图片落盘 notes/assets/，正文只存相对路径） ====================

/** 粘贴/拖拽图片落盘（传 data URL），返回 Markdown 用的相对路径 `assets/<name>` */
export async function saveNoteAsset(dataUrl: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('save_note_asset', { dataUrl });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function readNoteAsset(relPath: string): Promise<string | null> {
  const res = await invoke<ApiResponse<string | null>>('read_note_asset', { relPath });
  if (!res.success) throw new Error(res.error);
  return res.data ?? null;
}

// ==================== N7 笔记本批量导出（NB-02：整本导出 .md 文件夹，Obsidian 可直接打开） ====================

export interface NotebookExportReport {
  /** 实际导出目录（前端用它唤起 Finder） */
  dir: string;
  /** 导出笔记篇数（manual + journal） */
  notes: number;
  /** 随迁资产（图片）数 */
  assets: number;
}

/** 整本导出笔记本；不传 targetDir 时默认导出到桌面时间戳新目录 */
export async function exportNotebook(targetDir?: string): Promise<NotebookExportReport> {
  const res = await invoke<ApiResponse<NotebookExportReport>>('export_notebook', {
    targetDir: targetDir ?? null,
  });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ==================== NB-13 单篇导出（三空间右键菜单入口） ====================

export interface SingleExportReport {
  /** 导出的 .md 完整路径（前端用于展示/在访达中显示） */
  path: string;
  /** 随迁资产数 */
  assets: number;
}

export async function exportArticle(articleId: string): Promise<SingleExportReport> {
  const res = await invoke<ApiResponse<SingleExportReport>>('export_article', {
    articleId,
  });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ==================== NB-14 全局搜索：三域（笔记/条目/解读）后端融合检索（global_search.rs） ====================

export interface GlobalHit {
  /** note = 笔记/日记 · article = AI 解读 · item = 收件箱条目 */
  kind: 'note' | 'article' | 'item';
  id: string;
  title: string;
  snippet: string;
  score: number;
  /** note/article 为 article_type，item 为 item_type（徽章用） */
  sub_type?: string | null;
}

/** 全局搜索：关键词+语义在后端融合排序，统一结果，不暴露检索通道（用户指令） */
export async function globalSearch(query: string, limit?: number): Promise<GlobalHit[]> {
  const res = await invoke<ApiResponse<GlobalHit[]>>('global_search', {
    query,
    limit: limit ?? null,
  });
  if (!res.success) throw new Error(res.error);
  return res.data ?? [];
}

// ==================== NB-12 存储治理：容量统计 + 孤儿资产 GC（storage_gc.rs） ====================

export interface StorageStats {
  note_count: number;
  notes_bytes: number;
  asset_count: number;
  assets_bytes: number;
  orphan_count: number;
  orphan_bytes: number;
}

export interface GcReport {
  deleted_count: number;
  freed_bytes: number;
  after: StorageStats;
}

/** 笔记本容量统计（只读）：正文/资产/孤儿三组计数与字节数 */
export async function getStorageStats(): Promise<StorageStats> {
  const res = await invoke<ApiResponse<StorageStats>>('notebook_storage_stats');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 清理不被任何笔记引用的孤儿资产；删除时重算防陈旧快照 */
export async function gcOrphanAssets(): Promise<GcReport> {
  const res = await invoke<ApiResponse<GcReport>>('gc_orphan_assets');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

const assetCache = new Map<string, string | null>();

/** `assets/xxx` 相对路径 → data URL（带缓存，预览 400ms 重渲染不重复 invoke）；缺失返回 null */
export async function resolveNoteAsset(relPath: string): Promise<string | null> {
  if (assetCache.has(relPath)) return assetCache.get(relPath)!;
  let url: string | null = null;
  try {
    url = await readNoteAsset(relPath);
  } catch {
    url = null;
  }
  assetCache.set(relPath, url);
  return url;
}

// ==================== 数据抓取（与定时调度同一入口） ====================

export interface SourceFetchResult {
  sourceId: string;
  success: boolean;
  fetched: number;
  newItems: number;
  error: string | null;
}

/** 手动刷新数据源；sourceIds 为空时刷新全部启用源 */
export async function fetchSourcesNow(sourceIds?: string[]): Promise<SourceFetchResult[]> {
  const res = await invoke<ApiResponse<SourceFetchResult[]>>('fetch_sources_now', {
    sourceIds: sourceIds ?? null,
  });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 条目正文内容（item_contents 表，与轻量元数据分离） */
export interface ItemContent {
  itemId: string;
  status: 'pending' | 'fetching' | 'ready' | 'partial' | 'failed' | 'unsupported';
  contentText: string | null;
  excerpt: string | null;
  evidenceJson: string | null;
  contentType: 'readme' | 'article' | 'abstract' | 'model_card' | 'discussion' | 'none' | null;
  qualityLevel: number;  // 0 标题 / 1 简介 / 2 摘要或正文 / 3 正文+多源证据
  contentHash: string | null;
  fetchedAt: string | null;
  errorMessage: string | null;
}

/** 获取条目正文（有缓存直接返回，否则按来源抓取后落库） */
export async function getItemContent(itemId: string): Promise<ItemContent | null> {
  const res = await invoke<ApiResponse<ItemContent | null>>('get_item_content', { itemId });
  if (!res.success) throw new Error(res.error);
  return res.data ?? null;
}

/** 内容抓取覆盖率统计（P0 验收口径） */
export interface SourceCoverage {
  ok: number;
  total: number;
  rate: number;
}

/** 源健康三件套（借鉴 ai-news-radar source-status）：成功率 / 最后成功 / 24h 产量 */
export interface SourceHealth {
  id: string;
  name: string;
  tier: string;
  admission: string;
  successCount: number;
  failCount: number;
  successRate: number;
  lastSuccessAt: string | null;
  lastError: string | null;
  yield24h: number;
  itemsTotal: number;
}

export interface CoverageStats {
  total_items: number;
  with_content: number;
  coverage: number;
  by_status: Record<string, number>;
  rates: { github_readme: number | null; hn_article: number | null; hf_model_card: number | null };
  /** A1：各来源真实分子/分母（分母 = 该源全部受支持条目），用于展示如 43/140 */
  per_source?: {
    github_readme: SourceCoverage | null;
    hn_article: SourceCoverage | null;
    hf_model_card: SourceCoverage | null;
  };
  targets: { github_readme: number; hn_article: number; hf_model_card: number };
  /** 源健康度面板数据（每源：成功率/最后成功/24h 产量/tier/admission） */
  health?: SourceHealth[];
}

export async function getCoverageStats(): Promise<CoverageStats> {
  const res = await invoke<ApiResponse<CoverageStats>>('content_coverage_stats');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 存量正文补抓（A2：按来源配额）：limit 为每个来源的配额（默认 10），五个来源均衡推进，总量 ≈ 50 */
export async function backfillItemContents(limit?: number): Promise<{
  processed: number;
  ready: number;
  partial: number;
  failed: number;
  unsupported: number;
}> {
  const res = await invoke<ApiResponse<any>>('backfill_item_contents', { limit: limit ?? 10 });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ==================== 语义搜索（向量索引） API ====================

export interface SearchHit {
  item: Item;
  distance: number;
  /** chunk 级命中时带回的证据片段（可溯源到正文） */
  snippet?: string;
}

/** chunk 级语义搜索命中（Rust 侧 vec_search_chunks） */
export interface ChunkSearchHit {
  item: Item;
  distance: number;
  chunkText: string;
  chunkIdx: number;
}

/** 故事级合并结果（Rust 侧 stories 表） */
export interface Story {
  id: string;
  title: string;
  itemIds: string[];
  sourceIds: string[];
  sourceCount: number;
  signalLevel: 'single' | 'multi';
  updatedAt?: string | null;
}

export interface VecIndexStats {
  indexedCount: number;
  dimension: number | null;
  totalItems: number;
}

export async function vecUpsertEmbedding(itemId: string, vector: number[]): Promise<string> {
  const res = await invoke<ApiResponse<string>>('vec_upsert_embedding', { itemId, vector });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function vecSearch(vector: number[], limit?: number): Promise<SearchHit[]> {
  const res = await invoke<ApiResponse<SearchHit[]>>('vec_search', { vector, limit });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function vecIndexStats(): Promise<VecIndexStats> {
  const res = await invoke<ApiResponse<VecIndexStats>>('vec_index_stats');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function vecIndexedIds(): Promise<string[]> {
  const res = await invoke<ApiResponse<string[]>>('vec_indexed_ids');
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

export async function vecDeleteEmbedding(itemId: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('vec_delete_embedding', { itemId });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ---------- chunk 级语义索引（借鉴 khoj chunk+embedding 管线） ----------

export interface ChunkInput {
  idx: number;
  text: string;
  vector: number[];
}

/** 全量替换某条目的 chunk 索引（文本 + 向量） */
export async function vecUpsertChunks(itemId: string, chunks: ChunkInput[]): Promise<string> {
  const res = await invoke<ApiResponse<string>>('vec_upsert_chunks', { itemId, chunks });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** chunk 级语义搜索：命中证据片段并带回所属条目 */
export async function vecSearchChunks(vector: number[], limit?: number): Promise<ChunkSearchHit[]> {
  const res = await invoke<ApiResponse<ChunkSearchHit[]>>('vec_search_chunks', { vector, limit });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 已做 chunk 索引的条目 id（增量索引用） */
export async function vecChunkIndexedIds(): Promise<string[]> {
  const res = await invoke<ApiResponse<string[]>>('vec_chunk_indexed_ids');
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

// ---------- N3：笔记/文档 chunk 级语义索引（note_id = articles.id） ----------

/** 笔记 chunk 级命中（Rust 侧 vec_search_note_chunks） */
export interface NoteChunkHit {
  noteId: string;
  title: string;
  articleType: string;
  distance: number;
  chunkText: string;
  chunkIdx: number;
}

/** 全量替换某篇笔记/文档的 chunk 索引（文本 + 向量） */
export async function vecUpsertNoteChunks(noteId: string, chunks: ChunkInput[]): Promise<string> {
  const res = await invoke<ApiResponse<string>>('vec_upsert_note_chunks', { noteId, chunks });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 笔记 chunk 级语义搜索：命中片段并带回所属文档 */
export async function vecSearchNoteChunks(vector: number[], limit?: number): Promise<NoteChunkHit[]> {
  const res = await invoke<ApiResponse<NoteChunkHit[]>>('vec_search_note_chunks', { vector, limit });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 已做 chunk 索引的笔记 id（增量索引用） */
export async function vecNoteChunkIndexedIds(): Promise<string[]> {
  const res = await invoke<ApiResponse<string[]>>('vec_note_chunk_indexed_ids');
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 只读正文缓存（不触发抓取）：chunk 索引只用本地已就绪正文 */
export async function getContentCached(itemId: string): Promise<ItemContent | null> {
  const res = await invoke<ApiResponse<ItemContent | null>>('get_content_cached', { itemId });
  if (!res.success) throw new Error(res.error);
  return res.data ?? null;
}

// ---------- 故事级合并（借鉴 ai-news-radar stories-merged） ----------

export async function rebuildStories(): Promise<{ stories: number; multiSource: number }> {
  const res = await invoke<ApiResponse<{ stories: number; multiSource: number }>>('rebuild_stories');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export async function getStories(limit?: number): Promise<Story[]> {
  const res = await invoke<ApiResponse<Story[]>>('get_stories', { limit: limit ?? 100 });
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

// ==================== 每日 Top5 推荐（发现页数据层） ====================

export interface PickInput {
  itemId: string;
  rank: number;
  heatScore?: number | null;
  aiScore?: number | null;
  reason?: string | null;
}

/** 推荐候选：近 7 天该类别条目（按热度 Top40）+ 历史入选记录（跨天去重用） */
export async function getPickCandidates(category: DiscoverCategory): Promise<{
  candidates: Item[];
  picked: PickedRef[];
}> {
  const res = await invoke<ApiResponse<{ candidates: Item[]; picked: PickedRef[] }>>(
    'db_get_pick_candidates',
    { category }
  );
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 保存某天某类别的 Top5 入选结果（整体替换） */
export async function saveDailyPicks(date: string, category: DiscoverCategory, picks: PickInput[]): Promise<string> {
  const res = await invoke<ApiResponse<string>>('db_save_daily_picks', { date, category, picks });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 入选记录 + 关联条目（时间线：日期倒序、排名升序） */
export async function getDailyPicks(category?: DiscoverCategory, limit?: number): Promise<DailyPick[]> {
  const res = await invoke<ApiResponse<DailyPick[]>>('db_get_daily_picks', {
    category: category ?? null,
    limit: limit ?? 100,
  });
  if (!res.success) throw new Error(res.error);
  return (res.data || []).map((p) => ({
    ...p,
    item: {
      ...p.item,
      topics: typeof p.item.topics === 'string' ? (p.item.topics as unknown as string).split(',').filter(Boolean) : (p.item.topics ?? []),
      aiTags: typeof p.item.aiTags === 'string' ? (p.item.aiTags as unknown as string).split(',').filter(Boolean) : (p.item.aiTags ?? []),
    },
  }));
}

// ==================== NEXT-048 发现五断面：只读 feed（精选/全部） ====================

/** 精选五面（与 Rust discovery::ASPECTS / Skill aspect-rules 同口径） */
export const DISCOVERY_ASPECTS = ['模型', '产品', '行业', '论文', '观点'] as const;
export type DiscoveryAspect = (typeof DISCOVERY_ASPECTS)[number];

export interface DiscoveryFeedRow {
  id: string;
  sourceId: string;
  sourceName: string;
  type: string;
  title: string;
  url?: string | null;
  author?: string | null;
  stars?: number | null;
  publishedAt?: string | null;
  fetchedAt?: string | null;
  aiSummary?: string | null;
  aiTags?: string | null;
  contentStatus?: string | null;
  qualityLevel?: number | null;
  aiScore: number;
  aiScoredAt: string;
  aspect?: DiscoveryAspect | null;
  aiTopics: string[];
  aiReason?: string | null;
  status: string;
  description?: string | null;
  language?: string | null;
  forks?: number | null;
}

export interface DiscoveryFeedPage {
  rows: DiscoveryFeedRow[];
  nextCursor?: string | null;
}

export interface DiscoveryFeedQuery {
  aspect?: DiscoveryAspect | null;
  source?: string | null;
  topic?: string | null;
  minScore?: number | null;
  windowDays?: number | null;
  /** 所有发现时间线专用：仅返回已通过 Hermes 深度解读并成功保存的条目。 */
  requireDeep?: boolean | null;
  /** Hermes 深度解读补全任务专用：仅返回尚无有效 deep 的条目。 */
  missingDeep?: boolean | null;
  cursor?: string | null;
  limit?: number | null;
}

/**
 * 发现 feed（打分时间倒序 + keyset 游标分页）。打分语义归 Skill（sophonote-ai-radar）：
 * 精选 = minScore=8.5 ∧ windowDays=7 ∧ requireDeep（+aspect）；全部 AI 动态 = minScore=7 ∧ requireDeep。
 */
export async function getDiscoveryFeed(query: DiscoveryFeedQuery): Promise<DiscoveryFeedPage> {
  const res = await invoke<ApiResponse<DiscoveryFeedPage>>('db_discovery_feed', {
    aspect: query.aspect ?? null,
    source: query.source ?? null,
    topic: query.topic ?? null,
    minScore: query.minScore ?? null,
    windowDays: query.windowDays ?? null,
    requireDeep: query.requireDeep ?? null,
    missingDeep: query.missingDeep ?? null,
    cursor: query.cursor ?? null,
    limit: query.limit ?? 40,
  });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export interface DiscoveryTopicSummary {
  name: string;
  group: '公司与模型' | '技术方向' | '内容形态';
  count: number;
}

export async function getDiscoveryTopicsSummary(
  minScore = 7,
  windowDays = 7,
): Promise<DiscoveryTopicSummary[]> {
  const res = await invoke<ApiResponse<DiscoveryTopicSummary[]>>('db_discovery_topics_summary', {
    minScore,
    windowDays,
  });
  if (!res.success) throw new Error(res.error);
  return res.data ?? [];
}

export interface ModelLeaderboardRow {
  date: string;
  modelKey: string;
  name: string;
  vendor?: string | null;
  rank: number;
  consensus: number;
  meta: Record<string, unknown>;
}

export interface ModelLeaderboardSnapshot {
  date?: string | null;
  rows: ModelLeaderboardRow[];
}

export async function getModelLeaderboard(date?: string | null): Promise<ModelLeaderboardSnapshot> {
  const res = await invoke<ApiResponse<ModelLeaderboardSnapshot>>('db_model_leaderboard', {
    date: date ?? null,
  });
  if (!res.success) throw new Error(res.error);
  return res.data ?? { date: null, rows: [] };
}

export interface OpenRouterRankingSnapshot {
  asOf: string;
  fetchedAt: string;
  citation: string;
  sourceUrl: string;
  models: unknown;
  rankingsDaily: unknown;
  taskClassifications: unknown;
  sessionCost: unknown;
  benchmarks: unknown;
}

export async function getOpenRouterRankings(): Promise<OpenRouterRankingSnapshot | null> {
  const res = await invoke<ApiResponse<OpenRouterRankingSnapshot | null>>('db_openrouter_rankings');
  if (!res.success) throw new Error(res.error);
  return res.data ?? null;
}

export async function refreshOpenRouterRankings(): Promise<OpenRouterRankingSnapshot> {
  const res = await invoke<ApiResponse<OpenRouterRankingSnapshot>>('openrouter_rankings_refresh');
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

export type DiscoveryReportPeriod = 'daily' | 'weekly' | 'monthly';

export interface DiscoveryReportView {
  id: string;
  title: string;
  period: DiscoveryReportPeriod;
  periodKey: string;
  content: string;
  createdAt: string;
  updatedAt?: string | null;
}

export async function getDiscoveryReports(): Promise<DiscoveryReportView[]> {
  const res = await invoke<ApiResponse<DiscoveryReportView[]>>('db_discovery_reports');
  if (!res.success) throw new Error(res.error);
  return res.data ?? [];
}

// ==================== Track B · AG-02：AI 工作室项目容器 API ====================
// 扁平分组容器 + 单一归属（一篇文档至多一个项目，move 语义），见 docs/architecture.md

/** 项目 ↔ 文档成员关系（前端派生 documentId → projectId 归属 map） */
export interface ProjectMembership {
  projectId: string;
  articleId: string;
  /** NB-19（AG-11 落地）：项目内父文档（Notion 式文档树）；null = 项目根 */
  parentId: string | null;
}

/** 项目列表（含文档数） */
export async function projectList(): Promise<Project[]> {
  const res = await invoke<ApiResponse<Project[]>>('project_list');
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 新建项目（前端生成 uuid 与 createdAt） */
export async function projectCreate(project: Project): Promise<Project> {
  const res = await invoke<ApiResponse<Project>>('project_create', { project });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 重命名项目 */
export async function projectRename(id: string, name: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('project_rename', { id, name });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 置顶/取消置顶项目 */
export async function projectSetPinned(id: string, pinned: boolean): Promise<string> {
  const res = await invoke<ApiResponse<string>>('project_set_pinned', { id, pinned });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 设置项目描述/目标（AG-03：AI 归属整理与未来项目 Chat 的上下文；空串清除） */
export async function projectSetDescription(id: string, description: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('project_set_description', { id, description });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 移除项目关联；本地工作区、代码、Markdown 与会话历史均保留。 */
export async function projectDelete(id: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('project_delete', { id });
  if (!res.success) throw new Error(res.error);
}

/** 全量成员关系（前端派生归属 map） */
export async function projectListMemberships(): Promise<ProjectMembership[]> {
  const res = await invoke<ApiResponse<ProjectMembership[]>>('project_list_memberships');
  if (!res.success) throw new Error(res.error);
  return res.data || [];
}

/** 归属/移动文档到项目（单一归属 move 语义）——未来智能体「文件夹归属整理」的写入原语 */
export async function projectAssignDocument(projectId: string, articleId: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('project_assign_document', { projectId, articleId });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

/** 解除文档归属（文档本身不删） */
export async function projectRemoveDocument(articleId: string): Promise<string> {
  const res = await invoke<ApiResponse<string>>('project_remove_document', { articleId });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ==================== Track A · NB-19（用户指令例外，AG-11 落地）：项目内文档组织树 ====================
// Notion 式：文档即树节点（parent_id 指向同项目另一文档，null = 项目根）

/** 项目内置父（null = 回到项目根；同项目校验与防环在 Rust 侧） */
export async function projectSetDocParent(articleId: string, parentId: string | null): Promise<string> {
  const res = await invoke<ApiResponse<string>>('project_set_doc_parent', { articleId, parentId });
  if (!res.success) throw new Error(res.error);
  return res.data!;
}

// ==================== Track B · AG-24/26：文档 Patch 审批面（预览/批准/拒绝/撤销/审计） ====================
// 契约：docs/architecture.md「先 dry-run；预览后确认保存」。camelCase 与 Rust serde 对齐。
// 模型只能产出 proposed 操作；落盘必须经 apply（用户显式批准），部分批准传 approvedHunks 子集。

/** 行级 diff 块（AG-26 多 hunk：升序、互不重叠；纯插入块 removed 为空） */
export interface PatchHunk {
  /** 0-based：旧正文中首个被删行的行号（纯插入 = 插入点行号） */
  startLine: number;
  contextBefore: string[];
  removed: string[];
  added: string[];
  contextAfter: string[];
}

/** dry-run 预览结果（审批卡数据源） */
export interface PatchPreview {
  operationId: string;
  approvalId: string | null;
  documentId: string;
  title: string;
  baseVersion: number;
  targetVersion: number;
  oldText: string;
  newText: string;
  hunks: PatchHunk[];
  /** pending_approval = 待批准；committed = 幂等重入命中已提交；其余为终局态原值 */
  status: string;
  /** AG-25 范围语义；null = AG-24 文本锚点路径 */
  scope: 'selection' | 'current-block' | 'section' | null;
  rebased: boolean;
  /** NEXT-042：同一审批内的标题改提案；缺省/null = 仅正文变更 */
  proposedTitle?: string | null;
}

/** 应用/撤销结果 */
export interface ApplyResult {
  documentId: string;
  version: number;
  revisionId: string | null;
  /** true = 幂等重入：操作此前已提交，本次零写入 */
  alreadyCommitted: boolean;
  /** NEXT-042：本次应用实际写盘的新标题（null = 未改标题）；前端凭此同步侧边栏 */
  appliedTitle?: string | null;
}

/** 项目维度 patch 审计条目（AG-26：重启后重建审批卡/审计轨迹）。
 *  Rust 侧 #[serde(flatten)] 展平 preview 全字段 + 终局状态 */
export interface ProjectPatchEntry {
  operationId: string;
  approvalId: string | null;
  documentId: string;
  title: string;
  baseVersion: number;
  targetVersion: number;
  oldText: string;
  newText: string;
  hunks: PatchHunk[];
  status: string;
  scope: 'selection' | 'current-block' | 'section' | null;
  rebased: boolean;
  /** NEXT-042：同一审批内的标题改提案（serde skip-if-none → 旧行缺省） */
  proposedTitle?: string | null;
  /** 操作原始终局态：proposed/prepared/committed/rejected/failed/rolled_back */
  opStatus: string;
  /** 失败原因（仅 failed/rolled_back 有值） */
  error: string | null;
  /** 部分批准时实际应用的 hunk 下标子集（null = 全量批准） */
  appliedHunks: number[] | null;
  /** 提案创建时间（毫秒时间戳） */
  createdAt: number;
  /** operation 级 checkpoint 仍是文档最新 revision 时才允许撤销。 */
  undoable: boolean;
  /** 不可撤销时的稳定、面向用户的原因。 */
  undoUnavailableReason: string | null;
}

/** 批准应用提案。approvedHunks = 批准的 hunk 下标子集（逐 hunk 部分批准）；
 *  不传或覆盖全部 = 整块批准（AG-24 行为）。冲突/结构校验失败抛 Error（消息可直接展示） */
export async function documentApplyPatch(
  operationId: string,
  approvedHunks?: number[],
): Promise<ApplyResult> {
  const res = await invoke<ApiResponse<ApplyResult>>('document_apply_patch', {
    operationId,
    approvedHunks: approvedHunks ?? null,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'document_apply_patch failed');
  return res.data;
}

/** 拒绝提案（零文件写入） */
export async function documentRejectPatch(operationId: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('document_reject_patch', { operationId });
  if (!res.success) throw new Error(res.error ?? 'document_reject_patch failed');
}

/** 撤销最近一次修订（快照还原为新版本；可再撤销 = redo） */
export async function documentUndo(documentId: string, idempotencyKey?: string): Promise<ApplyResult> {
  const res = await invoke<ApiResponse<ApplyResult>>('document_undo', {
    documentId,
    idempotencyKey: idempotencyKey ?? null,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'document_undo failed');
  return res.data;
}

/** 精确撤销某次 Agent patch；只恢复该 operation 写入前的 revision。 */
export async function documentUndoPatch(operationId: string): Promise<ApplyResult> {
  const res = await invoke<ApiResponse<ApplyResult>>('document_undo_patch', { operationId });
  if (!res.success || !res.data) throw new Error(res.error ?? 'document_undo_patch failed');
  return res.data;
}

/** AG-26：项目 patch 列表（新→旧 ≤50 条）——重启后重建审批卡与审计轨迹的数据源 */
export async function documentProjectPatches(projectId: string): Promise<ProjectPatchEntry[]> {
  const res = await invoke<ApiResponse<ProjectPatchEntry[]>>('document_project_patches', { projectId });
  if (!res.success) throw new Error(res.error ?? 'document_project_patches failed');
  return res.data || [];
}

/** AG-26：文档当前版本号（选区 chip 的 baseVersion 来源；Article DTO 无 version 字段） */
export async function documentCurrentVersion(documentId: string): Promise<number> {
  const res = await invoke<ApiResponse<number>>('document_current_version', { documentId });
  if (!res.success || res.data === undefined || res.data === null) {
    throw new Error(res.error ?? 'document_current_version failed');
  }
  return res.data;
}

// ==================== Track B · AG-27：Skill 系统（三层加载/启用态/权限交集） ====================
// 契约：docs/architecture.md。Skill 只是提示词配方，不是权限/工作流引擎；
// 有效工具 = 声明 ∩ 可用，只收窄不放大。安装 = 清单文件入 user/workspace 目录；
// 本层只提供列表与启用开关，激活在 agent_run_start（startRun 的 skill 参数）。

/** Skill 来源（三层优先级 workspace > user > bundled） */
export type SkillSource = 'bundled' | 'user' | 'workspace';

/** 管理面板条目（与 Rust SkillInfo camelCase 对齐；无效清单 version=0/execution='invalid'） */
export interface SkillInfo {
  name: string;
  version: number;
  description: string;
  /** agent = 模型决定顺序；workflow = Rust 编排；invalid = 清单解析失败 */
  execution: string;
  source: SkillSource;
  /** 清单文件路径（bundled 为内置标记） */
  origin: string;
  enabled: boolean;
  /** 清单解析校验通过（false = 仅展示问题，不可启用/激活） */
  available: boolean;
  /** 清单声明的工具（「我需要」，不是授权） */
  tools: string[];
  /** 权限交集结果：声明 ∩ 当前可用（§七） */
  effectiveTools: string[];
  /** 声明了但当前不存在的工具 */
  missingTools: string[];
  problems: string[];
  maxModelCalls: number | null;
  maxToolCalls: number | null;
}

/** skill_list 返回：清单 + 安装目录（管理面板的目录说明数据源） */
export interface SkillListReport {
  skills: SkillInfo[];
  userDir: string;
  /** 无项目上下文时为 null（workspace 层按项目隔离） */
  workspaceDir: string | null;
}

/** AG-27：Skill 列表（bundled + user + 可选 workspace 层；含启用态与工具交集） */
export async function skillList(projectId?: string | null): Promise<SkillListReport> {
  const res = await invoke<ApiResponse<SkillListReport>>('skill_list', {
    projectId: projectId ?? null,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'skill_list failed');
  return res.data;
}

export interface HermesSkillInfo {
  name: string;
  description: string;
  origin: string | null;
  category: string;
  enabled: boolean;
  usage: number;
  provenance: string;
}

/** Hermes Runtime 原生 Skill 目录；正文与启用态均由 Hermes 维护。 */
export async function hermesSkillList(): Promise<HermesSkillInfo[]> {
  const res = await invoke<ApiResponse<HermesSkillInfo[]>>('agent_hermes_skills');
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_skills failed');
  return res.data;
}

export interface HermesModelProvider {
  slug: string;
  name: string;
  models: string[];
  authenticated: boolean | null;
  isCurrent: boolean | null;
}

export interface HermesModelOptions {
  model: string | null;
  provider: string | null;
  providers: HermesModelProvider[];
}

export type HermesCronStatus = 'active' | 'running' | 'paused' | 'completed' | 'error';

export interface HermesCronJobInfo {
  id: string;
  name: string;
  prompt: string;
  schedule: string;
  scheduleKind: string;
  scheduleSpec: Record<string, unknown> | null;
  status: HermesCronStatus;
  enabled: boolean;
  nextRunAt: string | null;
  lastRunAt: string | null;
  lastStatus: string | null;
  lastError: string | null;
  skills: string[];
  profile: string;
  executionStatus: string | null;
  createdAt: string | null;
  projectId: string | null;
  projectName: string | null;
  provider: string | null;
  model: string | null;
}

export interface HermesCronDraft {
  name: string;
  prompt: string;
  schedule: string;
  projectId: string | null;
  skills: string[];
  provider: string | null;
  model: string | null;
  /** 公开范例等显式草稿创建后必须保持暂停；普通编辑不设置。 */
  startPaused?: boolean;
}

export type HermesCronRunStatus = 'pending' | 'running' | 'completed' | 'error';

export interface HermesCronRunInfo {
  sessionId: string;
  status: HermesCronRunStatus;
  startedAt: number | null;
  endedAt: number | null;
  preview: string;
  endReason: string | null;
  profile: string;
  model: string | null;
  toolCallCount: number;
  modelCallCount: number;
  lastActivity: string | null;
}

export interface HermesCronRunStep {
  index: number;
  phase: string;
  title: string;
  toolName: string;
  status: 'running' | 'completed' | 'error';
  input: string;
  output: string;
}

export interface HermesCronRunResult {
  sessionId: string;
  markdown: string;
  steps: HermesCronRunStep[];
}

/** Hermes Cron 是唯一计划任务真相源；该接口只返回 SophoNote 展示投影。 */
export async function hermesCronJobs(): Promise<HermesCronJobInfo[]> {
  const res = await invoke<ApiResponse<HermesCronJobInfo[]>>('agent_hermes_cron_jobs');
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_jobs failed');
  return res.data;
}

export async function hermesCronCreate(draft: HermesCronDraft): Promise<HermesCronJobInfo> {
  const res = await invoke<ApiResponse<HermesCronJobInfo>>('agent_hermes_cron_create', { draft });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_create failed');
  return res.data;
}

export async function hermesCronUpdate(
  job: Pick<HermesCronJobInfo, 'id' | 'profile'>,
  draft: HermesCronDraft,
): Promise<HermesCronJobInfo> {
  const res = await invoke<ApiResponse<HermesCronJobInfo>>('agent_hermes_cron_update', {
    id: job.id,
    profile: job.profile,
    draft,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_update failed');
  return res.data;
}

export async function hermesCronSetEnabled(
  job: Pick<HermesCronJobInfo, 'id' | 'profile'>,
  enabled: boolean,
): Promise<HermesCronJobInfo> {
  const res = await invoke<ApiResponse<HermesCronJobInfo>>('agent_hermes_cron_set_enabled', {
    id: job.id,
    profile: job.profile,
    enabled,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_set_enabled failed');
  return res.data;
}

export async function hermesCronTrigger(
  job: Pick<HermesCronJobInfo, 'id' | 'profile'>,
): Promise<HermesCronJobInfo> {
  const res = await invoke<ApiResponse<HermesCronJobInfo>>('agent_hermes_cron_trigger', {
    id: job.id,
    profile: job.profile,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_trigger failed');
  return res.data;
}

export async function hermesCronDelete(
  job: Pick<HermesCronJobInfo, 'id' | 'profile'>,
): Promise<string> {
  const res = await invoke<ApiResponse<string>>('agent_hermes_cron_delete', {
    id: job.id,
    profile: job.profile,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_delete failed');
  return res.data;
}

export async function hermesCronRuns(
  job: Pick<HermesCronJobInfo, 'id' | 'profile'>,
): Promise<HermesCronRunInfo[]> {
  const res = await invoke<ApiResponse<HermesCronRunInfo[]>>('agent_hermes_cron_runs', {
    id: job.id,
    profile: job.profile,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_runs failed');
  return res.data;
}

export async function hermesCronRunResult(run: Pick<HermesCronRunInfo, 'sessionId' | 'profile'>): Promise<HermesCronRunResult> {
  const res = await invoke<ApiResponse<HermesCronRunResult>>('agent_hermes_cron_run_result', {
    sessionId: run.sessionId,
    profile: run.profile,
  });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_cron_run_result failed');
  return res.data;
}

/** Hermes 可执行目录与 SophoNote 设置中已配置供应商/凭据/模型的交集。 */
export async function hermesModelOptions(): Promise<HermesModelOptions> {
  const res = await invoke<ApiResponse<HermesModelOptions>>('agent_hermes_models');
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_models failed');
  return res.data;
}

/** 设置页读取 Runtime 完整发现目录；候选模型仍需显式加入配置。 */
export async function hermesModelCatalog(): Promise<HermesModelOptions> {
  const res = await invoke<ApiResponse<HermesModelOptions>>('agent_hermes_model_catalog');
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_model_catalog failed');
  return res.data;
}

export interface ProviderModelCatalog {
  models: string[];
  endpoint: string;
}

/** Host 使用 Keychain 凭据拉取 OpenAI-compatible `/models`；Key 不进入 WebView。 */
export async function fetchProviderModels(provider: string): Promise<ProviderModelCatalog> {
  const res = await invoke<ApiResponse<ProviderModelCatalog>>('ai_provider_models', { provider });
  if (!res.success || !res.data) throw new Error(res.error ?? 'ai_provider_models failed');
  return res.data;
}

export interface HermesUsageDaily {
  day: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  reasoningTokens: number;
  estimatedCost: number;
  actualCost: number;
  sessions: number;
  apiCalls: number;
}

export interface HermesUsageModel {
  model: string;
  inputTokens: number;
  outputTokens: number;
  estimatedCost: number;
  sessions: number;
  apiCalls: number;
}

export interface HermesUsageReport {
  daily: HermesUsageDaily[];
  byModel: HermesUsageModel[];
  totals: {
    totalInput: number;
    totalOutput: number;
    totalCacheRead: number;
    totalReasoning: number;
    totalEstimatedCost: number;
    totalActualCost: number;
    totalSessions: number;
    totalApiCalls: number;
  };
  periodDays: number;
}

export async function hermesUsage(days: 7 | 30 | 90): Promise<HermesUsageReport> {
  const res = await invoke<ApiResponse<HermesUsageReport>>('agent_hermes_usage', { days });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_usage failed');
  return res.data;
}

export interface HermesToolsetInfo {
  name: string;
  description: string;
  toolCount: number;
  enabled: boolean;
  usage: number;
  tools: string[];
}

export interface HermesSkillDocument {
  name: string;
  content: string;
}

export interface HermesToolInfo {
  name: string;
  description: string;
}

export interface HermesMcpServerInfo {
  name: string;
  transport: string;
  enabled: boolean;
  url: string | null;
  command: string | null;
  args: string[];
  auth: string | null;
  tools: HermesToolInfo[];
}

export interface HermesMcpServerCreate {
  name: string;
  transport: 'http' | 'stdio';
  url: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  auth: 'none' | 'header' | 'oauth';
  bearerToken: string;
}

export interface HermesMcpProbe {
  ok: boolean;
  error: string;
  tools: HermesToolInfo[];
  prompts: number;
  resources: number;
}

export interface HermesMcpOAuthFlow {
  flow_id: string;
  server_name: string;
  status: 'starting' | 'authorization_required' | 'approved' | 'error';
  authorization_url: string | null;
  error: string | null;
  tools: HermesToolInfo[];
}

export interface HermesTerminalBackendInfo {
  name: string;
  label: string;
  description: string;
  active: boolean;
  status: 'ready' | 'needs_setup' | 'unavailable' | string;
  detail: string;
}

export interface HermesTerminalBackends {
  active: string;
  backends: HermesTerminalBackendInfo[];
}

export interface HermesHubSourceInfo {
  id: string;
  label: string;
  available: boolean | null;
  rateLimited: boolean | null;
  searchable: boolean | null;
}

export interface HermesHubSources {
  sources: HermesHubSourceInfo[];
  indexAvailable: boolean;
}

export interface HermesHubPreview {
  name: string;
  description: string;
  source: string;
  identifier: string;
  trustLevel: string;
  repo: string | null;
  tags: string[];
  skillMd: string;
  files: string[];
}

export interface HermesMcpCatalogEntry {
  name: string;
  description: string;
  source: string;
  transport: string;
  authType: string;
  requiredEnv: { name: string; prompt: string; required: boolean }[];
  command: string | null;
  args: string[];
  url: string | null;
  postInstall: string;
  needsInstall: boolean;
  installed: boolean;
  enabled: boolean;
}

export interface HermesMcpCatalog {
  entries: HermesMcpCatalogEntry[];
}

export interface HermesCapabilities {
  commands: HermesCommandInfo[];
  skills: HermesSkillInfo[];
  references: HermesReferenceInfo[];
  toolsets: HermesToolsetInfo[];
  tools: HermesToolInfo[];
  mcpServers: HermesMcpServerInfo[];
  terminalBackends: HermesTerminalBackends;
  hubSources: HermesHubSources;
  browserConnected: boolean;
  browserUrl: string;
}

export interface HermesCommandInfo {
  name: string;
  description: string;
  category: string;
}

export interface HermesReferenceInfo {
  text: string;
  display: string;
  description: string;
}

export interface HermesHubSkillInfo {
  name: string;
  description: string;
  source: string;
  trust: string;
  identifier: string;
}

export interface HermesHubPage {
  items: HermesHubSkillInfo[];
  page: number;
  totalPages: number;
  total: number;
}

/** Hermes Gateway 的统一能力快照；SophoNote 不维护第二份 Skill/Tool/MCP 状态。 */
export async function hermesCapabilities(): Promise<HermesCapabilities> {
  const res = await invoke<ApiResponse<HermesCapabilities>>('agent_hermes_capabilities');
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_capabilities failed');
  return res.data;
}

export interface HermesSessionSurface {
  yolo: boolean;
  contextUsed: number | null;
  contextMax: number | null;
  contextPercent: number | null;
}

export interface HermesSlashSurfaceResult {
  kind: 'output' | 'prefill' | 'prompt' | string;
  message: string;
  notice: string | null;
  trimmedRunIds: string[];
}

export async function hermesSessionSurface(threadId: string): Promise<HermesSessionSurface> {
  const res = await invoke<ApiResponse<HermesSessionSurface>>('agent_hermes_session_surface', { threadId });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_session_surface failed');
  return res.data;
}

export async function hermesSessionSetYolo(threadId: string, enabled: boolean): Promise<boolean> {
  const res = await invoke<ApiResponse<boolean>>('agent_hermes_session_set_yolo', { threadId, enabled });
  if (!res.success || res.data == null) throw new Error(res.error ?? 'agent_hermes_session_set_yolo failed');
  return res.data;
}

export async function hermesSessionSlash(threadId: string, command: string): Promise<HermesSlashSurfaceResult> {
  const res = await invoke<ApiResponse<HermesSlashSurfaceResult>>('agent_hermes_session_slash', { threadId, command });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_session_slash failed');
  return res.data;
}

/** 手动重启 Hermes Runtime（前端触发重连）。 */
export async function restartHermesRuntime(): Promise<void> {
  const res = await invoke<ApiResponse<null>>('restart_hermes_runtime');
  if (!res.success) throw new Error(res.error ?? 'restart_hermes_runtime failed');
}

/** Hermes 连接状态类型。 */
export type HermesConnectionStatus = 'connected' | 'disconnected' | 'restarting';

/** 监听 Hermes 连接状态变化事件（由后端健康监督器推送）。返回取消监听函数。 */
export function listenHermesStatusChanged(
  handler: (status: HermesConnectionStatus) => void,
): Promise<UnlistenFn> {
  return listen<string>('sophonote:hermes-status-changed', (event) => {
    handler(event.payload as HermesConnectionStatus);
  });
}

/** 启停 Hermes Runtime Toolset。 */
export async function hermesToolsetSetEnabled(name: string, enabled: boolean): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_toolset_set_enabled', { name, enabled });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_toolset_set_enabled failed');
}

export async function hermesSkillSetEnabled(name: string, enabled: boolean): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_skill_set_enabled', { name, enabled });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_skill_set_enabled failed');
}

export async function hermesSkillDocument(name: string): Promise<HermesSkillDocument> {
  const res = await invoke<ApiResponse<HermesSkillDocument>>('agent_hermes_skill_document', { name });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_skill_document failed');
  return res.data;
}

export async function hermesSkillDocumentSave(name: string, content: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_skill_document_save', { name, content });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_skill_document_save failed');
}

export async function hermesSkillArchive(name: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_skill_archive', { name });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_skill_archive failed');
}

export async function hermesTerminalBackendSelect(backend: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_terminal_backend_select', { backend });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_terminal_backend_select failed');
}

/** 浏览或搜索 Hermes Skills Hub。 */
export async function hermesSkillsHub(query = '', page = 1): Promise<HermesHubPage> {
  const res = await invoke<ApiResponse<HermesHubPage>>('agent_hermes_skills_hub', { query, page });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_skills_hub failed');
  return res.data;
}

export async function hermesSkillHubPreview(identifier: string): Promise<HermesHubPreview> {
  const res = await invoke<ApiResponse<HermesHubPreview>>('agent_hermes_skill_hub_preview', { identifier });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_skill_hub_preview failed');
  return res.data;
}

export async function hermesMcpCatalog(): Promise<HermesMcpCatalog> {
  const res = await invoke<ApiResponse<HermesMcpCatalog>>('agent_hermes_mcp_catalog');
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_mcp_catalog failed');
  return res.data;
}

export async function hermesMcpCatalogInstall(name: string, env: Record<string, string>): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_mcp_catalog_install', { request: { name, env } });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_mcp_catalog_install failed');
}

/** 让 Hermes Runtime 安装并热重载 Hub Skill。 */
export async function hermesSkillInstall(identifier: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_skill_install', { identifier });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_skill_install failed');
}

/** 让 Hermes Runtime 连接或断开 Browser，并返回新的统一能力快照。 */
export async function hermesBrowserManage(action: 'connect' | 'disconnect'): Promise<HermesCapabilities> {
  const res = await invoke<ApiResponse<HermesCapabilities>>('agent_hermes_browser_manage', { action });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_browser_manage failed');
  return res.data;
}

/** MCP 管理面也以 Hermes Runtime 为真相源；密钥仅作为一次性请求传给 Hermes。 */
export async function hermesMcpAdd(request: HermesMcpServerCreate): Promise<HermesMcpProbe> {
  const res = await invoke<ApiResponse<HermesMcpProbe>>('agent_hermes_mcp_add', { request });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_mcp_add failed');
  return res.data;
}

export async function hermesMcpSetEnabled(name: string, enabled: boolean): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_mcp_set_enabled', { name, enabled });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_mcp_set_enabled failed');
}

export async function hermesMcpTest(name: string): Promise<HermesMcpProbe> {
  const res = await invoke<ApiResponse<HermesMcpProbe>>('agent_hermes_mcp_test', { name });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_mcp_test failed');
  return res.data;
}

export async function hermesMcpRemove(name: string): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_mcp_remove', { name });
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_mcp_remove failed');
}

export async function hermesMcpOAuthStart(name: string): Promise<HermesMcpOAuthFlow> {
  const res = await invoke<ApiResponse<HermesMcpOAuthFlow>>('agent_hermes_mcp_oauth_start', { name });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_mcp_oauth_start failed');
  return res.data;
}

export async function hermesMcpOAuthStatus(flowId: string): Promise<HermesMcpOAuthFlow> {
  const res = await invoke<ApiResponse<HermesMcpOAuthFlow>>('agent_hermes_mcp_oauth_status', { flowId });
  if (!res.success || !res.data) throw new Error(res.error ?? 'agent_hermes_mcp_oauth_status failed');
  return res.data;
}

export async function hermesMcpReload(): Promise<void> {
  const res = await invoke<ApiResponse<null>>('agent_hermes_mcp_reload');
  if (!res.success) throw new Error(res.error ?? 'agent_hermes_mcp_reload failed');
}

/** AG-27：启用/停用 Skill（安装 = 清单入目录，本命令管开关） */
export async function skillSetEnabled(name: string, source: SkillSource, enabled: boolean): Promise<void> {
  const res = await invoke<ApiResponse<null>>('skill_set_enabled', { name, source, enabled });
  if (!res.success) throw new Error(res.error ?? 'skill_set_enabled failed');
}

export interface KnowledgeVersionStatus {
  enabled: boolean;
  repositoryId?: string | null;
  authorizationState?: string | null;
  documentVersionCount: number;
  queuedJobCount: number;
}

export interface KnowledgeBaselineFile {
  path: string;
  contentHash: string;
  bytes: number;
}

export interface KnowledgeBaselinePreview {
  fileCount: number;
  totalBytes: number;
  skipped: number;
  files: KnowledgeBaselineFile[];
}

export async function knowledgeVersionStatus(): Promise<KnowledgeVersionStatus> {
  const res = await invoke<ApiResponse<KnowledgeVersionStatus>>('knowledge_version_status');
  if (!res.success || !res.data) throw new Error(res.error ?? 'knowledge_version_status failed');
  return res.data;
}

export async function knowledgeVersionSetEnabled(enabled: boolean): Promise<KnowledgeVersionStatus> {
  const res = await invoke<ApiResponse<KnowledgeVersionStatus>>('knowledge_version_set_enabled', { enabled });
  if (!res.success || !res.data) throw new Error(res.error ?? 'knowledge_version_set_enabled failed');
  return res.data;
}

export async function knowledgeVersionPreviewBaseline(): Promise<KnowledgeBaselinePreview> {
  const res = await invoke<ApiResponse<KnowledgeBaselinePreview>>('knowledge_version_preview_baseline');
  if (!res.success || !res.data) throw new Error(res.error ?? 'knowledge_version_preview_baseline failed');
  return res.data;
}
