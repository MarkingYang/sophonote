import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('../../services/tauri', () => ({
  projectList: vi.fn(), projectListMemberships: vi.fn().mockResolvedValue([]),
  projectCreate: vi.fn(), projectAssignThread: vi.fn(), getSetting: vi.fn(),
}));
vi.mock('../../stores/agentStore', async () => {
  const { create } = await import('zustand');
  const useAgentStore = create<{ selectedThreadId: string | null }>((set) => ({
    threads: [], historyThreads: [], selectedThreadId: null, runningRunByThreadId: {},
    loadThreads: vi.fn().mockResolvedValue(undefined), loadThreadHistory: vi.fn(),
    selectThread: (id: string | null) => set({ selectedThreadId: id }),
    reopenThread: vi.fn().mockResolvedValue(true),
  }));
  return { useAgentStore, isPlaceholderThreadTitle: (title: string) => title === '新会话' };
});
vi.mock('../../components/aistudio/ProjectChatPanel', () => ({
  default: (props: { projectId: string | null; workspaceRoot: string | null }) => createElement('div', {
    'data-testid': 'chat', 'data-project': props.projectId ?? '', 'data-root': props.workspaceRoot ?? '',
  }),
}));
import Conversation from '../Conversation';
import { open } from '@tauri-apps/plugin-dialog';
import * as tauri from '../../services/tauri';
import { useProjectStore } from '../../stores/projectStore';
import { useAgentStore, type AgentThread } from '../../stores/agentStore';
import { rememberWorkspaceBinding, parseWorkspaceBinding } from '../../services/workspaceBinding';

let host: HTMLDivElement;
let root: Root;
const projects = [{ id: 'pa', name: '研究甲', docCount: 0, createdAt: '2026-09-18' }, { id: 'pb', name: '研究乙', docCount: 0, createdAt: '2026-09-18' }];
const button = (label: string) => [...host.querySelectorAll('button')].find((node) => node.textContent === label || node.title === label || node.getAttribute('aria-label') === label || node.querySelector('span')?.textContent === label)!;
const click = async (label: string) => { await act(async () => button(label).click()); };
beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  vi.mocked(tauri.projectList).mockResolvedValue(projects);
  vi.mocked(tauri.getSetting).mockResolvedValue('');
  useProjectStore.setState({ projects: [], memberships: [], selectedProjectId: null, loaded: false, loading: false });
  useAgentStore.setState({ threads: [], historyThreads: [], selectedThreadId: null, runningRunByThreadId: {} });
  rememberWorkspaceBinding('ui:project-workspace:pa', parseWorkspaceBinding('/workspace/a'));
  rememberWorkspaceBinding('ui:project-workspace:pb', null);
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });

describe('项目入口与任务范围', () => {
  it('首页以项目为中心，不挂载会话；新建目录使用系统原生创建能力', async () => {
    await act(async () => root.render(createElement(Conversation)));
    expect(host.querySelector('[data-testid="chat"]')).toBeNull();
    expect(host.querySelector('[aria-label="项目会话目录"]')).toBeNull();
    expect(button('打开项目 研究甲')).toBeDefined();
    await click('创建项目');
    expect(button('新建文件夹')).toBeDefined();
    await click('新建文件夹');
    vi.mocked(open).mockResolvedValueOnce('/workspace/new-project');
    await click('选择文件夹…');
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ canCreateDirectories: true, directory: true }));
    expect(tauri.projectCreate).not.toHaveBeenCalled();
    await click('取消');
    expect(host.querySelector('[aria-label="项目首页"]')).not.toBeNull();
  });
  it('通过首页选择项目隔离置顶会话，返回后恢复最近打开的任务', async () => {
    vi.mocked(tauri.getSetting).mockImplementation(async (key) => key.endsWith(':pa') ? '/workspace/a' : '/workspace/b');
    const base = { updatedAt: 1, createdAt: 1, pinnedAt: 1, archivedAt: null, engine: 'pi', closedAt: null };
    useAgentStore.setState({ threads: [
      { ...base, id: 'a', title: '甲的任务', projectId: 'pa' } as AgentThread,
      { ...base, id: 'b', title: '乙的任务', projectId: 'pb' } as AgentThread,
    ] });
    await act(async () => root.render(createElement(Conversation)));
    await click('打开项目 研究甲');
    const left = () => host.querySelector('[aria-label="项目会话目录"]')!;
    expect(left().textContent).toContain('甲的任务');
    expect(left().textContent).not.toContain('乙的任务');
    const task = [...host.querySelectorAll('button')].find((node) => node.textContent?.startsWith('甲的任务'))!;
    await act(async () => task.click());
    expect(host.querySelector('[aria-label="切换项目"]')).toBeNull();
    const switchTo = async (id: string) => { await click('全部项目'); await click(`打开项目 ${id === 'pa' ? '研究甲' : '研究乙'}`); };
    await switchTo('pb');
    expect(left().textContent).toContain('乙的任务');
    expect(left().textContent).not.toContain('甲的任务');
    await switchTo('pa');
    expect(useAgentStore.getState().selectedThreadId).toBe('a');
    expect(host.querySelector('[data-testid="chat"]')?.getAttribute('data-root')).toBe('/workspace/a');
    await act(async () => window.dispatchEvent(new Event('sophonote:projects-home')));
    expect(host.querySelector('[data-testid="chat"]')).toBeNull();
    expect(host.querySelector('[aria-label="项目首页"]')).not.toBeNull();
  });

  it('选择另一个未绑定项目时绝不沿用前一个目录', async () => {
    vi.mocked(tauri.getSetting).mockImplementation(async (key) => key.endsWith(':pa') ? '/workspace/a' : '');
    await act(async () => root.render(createElement(Conversation)));
    await click('打开项目 研究甲');
    expect(host.querySelector('[data-testid="chat"]')?.getAttribute('data-root')).toBe('/workspace/a');
    await click('全部项目');
    await click('打开项目 研究乙');
    expect(host.querySelector('[data-testid="chat"]')).toBeNull();
    expect(host.textContent).toContain('为项目选择资料与交付文件夹');
    await click('全部项目');
    expect(host.querySelector('[data-testid="chat"]')).toBeNull();
  });
  it('取消原生目录选择不会创建项目，选择后提交同一目录', async () => {
    await act(async () => root.render(createElement(Conversation)));
    await click('创建项目');
    await click('使用已有文件夹');
    const submit = host.querySelector('button[type="submit"]') as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    vi.mocked(open).mockResolvedValueOnce(null);
    await click('选择文件夹…');
    expect(submit.disabled).toBe(true);
    expect(tauri.projectCreate).not.toHaveBeenCalled();
    vi.mocked(open).mockResolvedValueOnce('/workspace/deliverables');
    await click('选择文件夹…');
    expect(submit.disabled).toBe(false);
    vi.mocked(tauri.projectCreate).mockImplementation(async (project) => project);
    vi.mocked(tauri.getSetting).mockResolvedValue('/workspace/deliverables');
    await act(async () => submit.click());
    expect(tauri.projectCreate).toHaveBeenCalledWith(expect.objectContaining({ name: 'deliverables' }), '/workspace/deliverables');
    expect(host.querySelector('[data-testid="chat"]')?.getAttribute('data-root')).toBe('/workspace/deliverables');
    expect(host.querySelector('dialog')?.open).toBe(false);
  });
  it('移除未归属入口，项目历史可恢复，任务菜单仅保留置顶与归档', async () => {
    const base = { updatedAt: 1, createdAt: 1, pinnedAt: null, archivedAt: null, engine: 'pi' };
    useAgentStore.setState({ threads: [{ ...base, id: 'a', title: '项目内任务', projectId: 'pa', closedAt: null } as AgentThread], historyThreads: [
      { ...base, id: 'old', title: '旧收藏任务', projectId: null, collectionId: 'legacy', closedAt: 2 } as AgentThread,
      { ...base, id: 'history', title: '项目历史任务', projectId: 'pa', closedAt: 2 } as AgentThread,
    ] });
    await act(async () => root.render(createElement(Conversation)));
    expect(host.textContent).not.toContain('未归属任务');
    expect(host.querySelector('[data-testid="chat"]')).toBeNull();
    expect(useAgentStore.getState().loadThreads).not.toHaveBeenCalledWith(undefined, expect.anything());
    await click('打开项目 研究甲');
    expect(host.textContent).toContain('项目内任务');
    expect(host.textContent).not.toContain('旧收藏任务');
    const history = [...host.querySelectorAll('button')].find((node) => node.textContent?.startsWith('项目历史任务'))!;
    await act(async () => history.click());
    expect(useAgentStore.getState().reopenThread).toHaveBeenCalledWith('history', 'pa');
    await click('任务操作：项目历史任务');
    expect(host.querySelector('[aria-label="任务所属项目"]')).toBeNull();
    expect(button('置顶任务')).toBeDefined();
    expect(button('归档任务')).toBeDefined();
    expect(tauri.projectAssignThread).not.toHaveBeenCalled();
    expect(useProjectStore.getState().selectedProjectId).toBe('pa');
  });

  it('当前项目被移除后返回首页，不产生无归属任务面板', async () => {
    vi.mocked(tauri.getSetting).mockImplementation(async (key) => key.endsWith(':pa') ? '/workspace/a' : '');
    await act(async () => root.render(createElement(Conversation)));
    await click('打开项目 研究甲');
    expect(host.querySelector('[data-testid="chat"]')).not.toBeNull();
    await act(async () => useProjectStore.setState({ projects: [] }));
    expect(host.querySelector('[data-testid="chat"]')).toBeNull();
    expect(host.querySelector('[aria-label="项目首页"]')).not.toBeNull();
    expect(host.textContent).not.toContain('新建任务');
  });
});
