import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../services/tauri', () => ({
  projectCreate: vi.fn(),
}));

vi.mock('../appStore', () => ({
  useAppStore: { setState: vi.fn() },
}));

import * as tauri from '../../services/tauri';
import { useProjectStore } from '../projectStore';

const projectCreateMock = vi.mocked(tauri.projectCreate);

beforeEach(() => {
  projectCreateMock.mockReset();
  useProjectStore.setState({
    projects: [],
    memberships: [],
    selectedProjectId: null,
    selectedDocumentId: null,
    loaded: true,
    loading: false,
  });
});

describe('创建项目即时反馈', () => {
  it('数据库返回前就插入并选中项目卡', async () => {
    let confirm!: (value: Awaited<ReturnType<typeof tauri.projectCreate>>) => void;
    projectCreateMock.mockReturnValue(new Promise((resolve) => { confirm = resolve; }));

    const pending = useProjectStore.getState().createProject('新项目');
    const optimistic = useProjectStore.getState();
    expect(optimistic.projects).toHaveLength(1);
    expect(optimistic.projects[0].name).toBe('新项目');
    expect(optimistic.selectedProjectId).toBe(optimistic.projects[0].id);

    confirm(optimistic.projects[0]);
    await expect(pending).resolves.toEqual(optimistic.projects[0]);
    expect(useProjectStore.getState().projects).toHaveLength(1);
  });

  it('创建失败精确回滚项目与此前选中态', async () => {
    useProjectStore.setState({
      projects: [{ id: 'project-old', name: '原项目', docCount: 0, createdAt: '2026-08-08' }],
      selectedProjectId: 'project-old',
      selectedDocumentId: 'doc-old',
    });
    projectCreateMock.mockRejectedValue(new Error('db locked'));

    await expect(useProjectStore.getState().createProject('失败项目')).resolves.toBeNull();
    const state = useProjectStore.getState();
    expect(state.projects.map((project) => project.id)).toEqual(['project-old']);
    expect(state.selectedProjectId).toBe('project-old');
    expect(state.selectedDocumentId).toBe('doc-old');
  });
});
