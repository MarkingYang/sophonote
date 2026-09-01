// ============================================================
// Track B · 智能体演进（AG-02 · AI 工作室试验田 · 项目容器 slice）
// §3.9 规则⑤：新 slice 独立文件，零改动 appStore.ts。
// 数据来源 = SQLite（Rust projects 模块），不 persist 到 localStorage。
// ============================================================
import { create } from 'zustand';
import type { Project } from '../types';
import * as tauri from '../services/tauri';

interface ProjectState {
  projects: Project[];
  /** 单一归属：一篇文档至多一条 */
  memberships: tauri.ProjectMembership[];
  selectedProjectId: string | null;
  /** AG-09：当前打开的文档（左栏树点选；null = 项目级对话状态窗口） */
  selectedDocumentId: string | null;
  loaded: boolean;
  loading: boolean;

  load: () => Promise<void>;
  select: (id: string | null) => void;
  selectDocument: (id: string | null) => void;
  createProject: (name: string) => Promise<Project | null>;
  renameProject: (id: string, name: string) => Promise<void>;
  setPinned: (id: string, pinned: boolean) => Promise<void>;
  /** AG-03：设置项目描述/目标（AI 归属整理与未来项目 Chat 的上下文；空串清除） */
  setDescription: (id: string, description: string) => Promise<void>;
  removeProject: (id: string) => Promise<void>;
  /** 归属/移动（move 语义：原归属自动释放） */
  assignDocument: (projectId: string, articleId: string) => Promise<void>;
  removeDocument: (articleId: string) => Promise<void>;
  /** NB-19（AG-11 落地）：项目内置父（Notion 式文档树；null = 项目根） */
  setDocParent: (articleId: string, parentId: string | null) => Promise<void>;
  /** 派生：文档当前归属项目 id（未归属 = null） */
  projectOf: (articleId: string) => string | null;
  /** 派生：项目当前文档数（以 memberships 实时为准，不信任服务端快照） */
  docCountOf: (projectId: string) => number;
}

function sortProjects(projects: Project[]): Project[] {
  return [...projects].sort((left, right) => {
    if (!!left.pinned !== !!right.pinned) return left.pinned ? -1 : 1;
    return left.createdAt.localeCompare(right.createdAt);
  });
}

export const useProjectStore = create<ProjectState>()((set, get) => ({
  projects: [],
  memberships: [],
  selectedProjectId: null,
  selectedDocumentId: null,
  loaded: false,
  loading: false,

  load: async () => {
    if (get().loading) return;
    set({ loading: true });
    try {
      const [projects, memberships] = await Promise.all([
        tauri.projectList(),
        tauri.projectListMemberships(),
      ]);
      set({ projects: sortProjects(projects), memberships, loaded: true });
    } catch (e) {
      console.error('Failed to load projects:', e);
    } finally {
      set({ loading: false });
    }
  },

  // 切换项目即回到项目级对话状态窗口（清文档选中）
  select: (id) => set({ selectedProjectId: id, selectedDocumentId: null }),

  selectDocument: (id) => set({ selectedDocumentId: id }),

  createProject: async (name) => {
    const draft: Project = {
      id: crypto.randomUUID(),
      name: name.trim(),
      docCount: 0,
      pinned: false,
      createdAt: new Date().toISOString(),
    };
    const previousSelectedProjectId = get().selectedProjectId;
    const previousSelectedDocumentId = get().selectedDocumentId;
    // 乐观插入：项目卡与选中态在同一帧出现，避免一次本地 SQLite IPC 往返
    // 被用户感知成“点击创建后卡住”。失败时按 draft id 精确回滚。
    set((s) => ({
      projects: [...s.projects, draft],
      selectedProjectId: draft.id,
      selectedDocumentId: null,
    }));
    try {
      const created = await tauri.projectCreate(draft);
      set((s) => ({
        projects: s.projects.map((project) => project.id === draft.id ? created : project),
      }));
      return created;
    } catch (e) {
      console.error('Failed to create project:', e);
      set((s) => {
        const draftStillSelected = s.selectedProjectId === draft.id;
        return {
          projects: s.projects.filter((project) => project.id !== draft.id),
          selectedProjectId: draftStillSelected ? previousSelectedProjectId : s.selectedProjectId,
          selectedDocumentId: draftStillSelected ? previousSelectedDocumentId : s.selectedDocumentId,
        };
      });
      return null;
    }
  },

  renameProject: async (id, name) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      await tauri.projectRename(id, trimmed);
      set((s) => ({
        projects: s.projects.map((p) => (p.id === id ? { ...p, name: trimmed } : p)),
      }));
    } catch (e) {
      console.error('Failed to rename project:', e);
      await get().load();
    }
  },

  setPinned: async (id, pinned) => {
    try {
      await tauri.projectSetPinned(id, pinned);
      set((state) => ({
        projects: sortProjects(state.projects.map((project) => (
          project.id === id ? { ...project, pinned } : project
        ))),
      }));
    } catch (error) {
      console.error('Failed to pin project:', error);
      await get().load();
    }
  },

  setDescription: async (id, description) => {
    const desc = description.trim();
    try {
      await tauri.projectSetDescription(id, desc);
      set((s) => ({
        projects: s.projects.map((p) =>
          p.id === id ? { ...p, description: desc || null } : p
        ),
      }));
    } catch (e) {
      console.error('Failed to set project description:', e);
      await get().load();
    }
  },

  removeProject: async (id) => {
    try {
      // DEC-036：只移除 SophoNote 项目元数据与关联，不触碰本地目录或正文。
      await tauri.projectDelete(id);
      set((s) => ({
        projects: s.projects.filter((p) => p.id !== id),
        memberships: s.memberships.filter((m) => m.projectId !== id),
        selectedProjectId: s.selectedProjectId === id ? null : s.selectedProjectId,
        selectedDocumentId: s.selectedProjectId === id ? null : s.selectedDocumentId,
      }));
    } catch (e) {
      console.error('Failed to remove project:', e);
      await get().load();
      // 交给项目菜单展示失败态；此前吞掉异常会让界面看起来“点击无效果”。
      throw e;
    }
  },

  assignDocument: async (projectId, articleId) => {
    try {
      await tauri.projectAssignDocument(projectId, articleId);
      set((s) => ({
        memberships: [
          ...s.memberships.filter((m) => m.articleId !== articleId),
          // NB-19：跨项目 move 经 INSERT OR REPLACE 重置 parent_id → 落新项目根
          { projectId, articleId, parentId: null },
        ],
      }));
    } catch (e) {
      console.error('Failed to assign document:', e);
      await get().load();
    }
  },

  removeDocument: async (articleId) => {
    try {
      await tauri.projectRemoveDocument(articleId);
      set((s) => ({
        memberships: s.memberships.filter((m) => m.articleId !== articleId),
        selectedDocumentId: s.selectedDocumentId === articleId ? null : s.selectedDocumentId,
      }));
    } catch (e) {
      console.error('Failed to remove document:', e);
      await get().load();
    }
  },

  // NB-19（AG-11 落地）：项目内置父。成功后就地改 memberships 的 parentId，
  // 失败（跨项目/成环等 Rust 校验拒绝）回读全量兜底
  setDocParent: async (articleId, parentId) => {
    try {
      await tauri.projectSetDocParent(articleId, parentId);
      set((s) => ({
        memberships: s.memberships.map((m) =>
          m.articleId === articleId ? { ...m, parentId } : m
        ),
      }));
    } catch (e) {
      console.error('Failed to set doc parent:', e);
      await get().load();
    }
  },

  projectOf: (articleId) =>
    get().memberships.find((m) => m.articleId === articleId)?.projectId ?? null,

  docCountOf: (projectId) =>
    get().memberships.filter((m) => m.projectId === projectId).length,
}));
