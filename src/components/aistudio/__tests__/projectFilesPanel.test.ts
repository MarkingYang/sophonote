import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/tauri', () => ({ scanLocalWorkspace: vi.fn(), listLocalWorkspaceDirectory: vi.fn(), readLocalWorkspaceFile: vi.fn() }));
vi.mock('../../features/MarkdownView', () => ({ default: ({ content }: { content: string }) => createElement('p', null, content) }));
import ProjectFilesPanel from '../ProjectFilesPanel';
import { scanLocalWorkspace, listLocalWorkspaceDirectory, readLocalWorkspaceFile } from '../../../services/tauri';
let host: HTMLDivElement; let root: Root;
const attach = vi.fn();
const button = (text: string) => [...host.querySelectorAll('button')].find((node) => node.textContent === text)!;
beforeEach(() => {
  vi.clearAllMocks(); Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });
describe('项目文件按需读取', () => {
  it('展开目录才读取子项，阅读后明确加入文件路径', async () => {
    vi.mocked(scanLocalWorkspace).mockResolvedValue({ root: '/workspace/research', name: 'research', entries: [{ name: '资料', path: '资料', kind: 'directory', size: 0 }] });
    vi.mocked(listLocalWorkspaceDirectory).mockResolvedValue([{ name: '简介.md', path: '资料/简介.md', kind: 'file', size: 10 }]);
    vi.mocked(readLocalWorkspaceFile).mockResolvedValue({ path: '资料/简介.md', content: '# 项目资料', truncated: false, size: 10 });
    await act(async () => root.render(createElement(ProjectFilesPanel, { root: '/workspace/research', onAttach: attach, onClose: vi.fn() })));
    expect(listLocalWorkspaceDirectory).not.toHaveBeenCalled(); expect(readLocalWorkspaceFile).not.toHaveBeenCalled();
    await act(async () => button('资料').click());
    expect(listLocalWorkspaceDirectory).toHaveBeenCalledWith('/workspace/research', '资料');
    await act(async () => button('简介.md').click());
    expect(readLocalWorkspaceFile).toHaveBeenCalledWith('/workspace/research', '资料/简介.md');
    expect(host.textContent).toContain('# 项目资料');
    await act(async () => button('加入任务').click());
    expect(attach).toHaveBeenCalledWith('/workspace/research/资料/简介.md', '简介.md');
  });
  it('拒绝读取的文件显示错误，不可加入任务', async () => {
    vi.mocked(scanLocalWorkspace).mockResolvedValue({ root: '/workspace', name: 'workspace', entries: [{ name: 'secret', path: 'secret', kind: 'file', size: 10 }] });
    vi.mocked(readLocalWorkspaceFile).mockRejectedValue(new Error('不允许读取符号链接'));
    await act(async () => root.render(createElement(ProjectFilesPanel, { root: '/workspace', onAttach: attach, onClose: vi.fn() })));
    await act(async () => button('secret').click());
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('不允许读取符号链接');
    expect(button('加入任务')).toBeUndefined();
  });
});
