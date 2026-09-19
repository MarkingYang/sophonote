import { useEffect, useMemo, useState } from 'react';
import { ArrowUpRight, FolderOpen, Pin, Plus, Search } from 'lucide-react';
import type { Project } from '../../types';
import type { AgentThread } from '../../stores/agentStore';
import { loadWorkspaceBinding, PROJECT_WORKSPACE_KEY_PREFIX, type WorkspaceBinding } from '../../services/workspaceBinding';

function ProjectCard({ project, count, updatedAt, onOpen }: {
  project: Project; count: number; updatedAt: number; onOpen: () => void;
}) {
  const [folder, setFolder] = useState<WorkspaceBinding | null>(null);
  useEffect(() => {
    let alive = true;
    void loadWorkspaceBinding(`${PROJECT_WORKSPACE_KEY_PREFIX}${project.id}`).then((value) => { if (alive) setFolder(value); });
    return () => { alive = false; };
  }, [project.id]);
  return <button type="button" onClick={onOpen} aria-label={`打开项目 ${project.name}`}
    className="sn-project-card group">
    <div className="sn-project-card-heading">
      <span className="sn-project-icon"><FolderOpen size={16} strokeWidth={1.6} /></span>
      <h2 title={project.name}>{project.name}</h2>
      {project.pinned && <span className="sn-project-pin"><Pin size={12} />置顶</span>}
      <ArrowUpRight size={16} className="sn-project-arrow" />
    </div>
    <p className="sn-project-folder" title={folder?.root}>{folder?.name ?? '待选择文件夹'}</p>
    <div className="sn-project-card-footer">
      <span><strong>{count}</strong> 个任务</span>
      <span>{updatedAt > 0 && Number.isFinite(updatedAt) ? `${new Date(updatedAt).toLocaleDateString('zh-CN', { month: 'long', day: 'numeric' })}更新` : '尚无更新'}</span>
    </div>
  </button>;
}

export default function ProjectHome({ projects, threads, loading, onCreate, onOpen }: {
  projects: Project[]; threads: AgentThread[]; loading: boolean;
  onCreate: () => void; onOpen: (id: string) => void;
}) {
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState('updated');
  const cards = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return projects.filter((project) => project.name.toLocaleLowerCase().includes(needle)).map((project) => {
      const members = threads.filter((thread) => thread.projectId === project.id);
      const updatedAt = Math.max(Date.parse(project.updatedAt || project.createdAt) || 0, ...members.map((thread) => thread.updatedAt));
      return { project, count: members.length, updatedAt };
    }).sort((a, b) => Number(!!b.project.pinned) - Number(!!a.project.pinned) || (sort === 'name' ? a.project.name.localeCompare(b.project.name, 'zh-CN') : b.updatedAt - a.updatedAt));
  }, [projects, threads, query, sort]);
  return <main aria-label="项目首页" className="min-w-0 flex-1 overflow-y-auto bg-[var(--bg-canvas)]">
    <div className="sn-project-home">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-baseline gap-2"><h1 className="text-xl font-semibold tracking-tight">项目</h1>{projects.length > 0 && <span className="sn-project-total">{projects.length}</span>}</div>
        <div className="sn-project-toolbar">
          <label className="relative"><Search size={14} className="pointer-events-none absolute left-2.5 top-2 text-[var(--text-tertiary)]" /><input aria-label="搜索项目" placeholder="搜索项目" value={query} onChange={(e) => setQuery(e.target.value)} className="h-8 w-40 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] pl-8 pr-3 text-xs outline-none focus:border-[var(--accent)]" /></label>
          <select aria-label="项目排序" value={sort} onChange={(e) => setSort(e.target.value)} className="h-8 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] px-3 text-xs"><option value="updated">最近更新</option><option value="name">项目名称</option></select>
          <button type="button" onClick={onCreate} className="inline-flex h-8 items-center gap-2 rounded-lg bg-[var(--text-primary)] px-4 text-xs font-medium text-[var(--bg-canvas)] hover:opacity-85"><Plus size={14} />创建项目</button>
        </div>
      </header>
      {loading && projects.length === 0 ? <p role="status" className="py-20 text-center text-xs text-[var(--text-tertiary)]">正在加载项目…</p> : projects.length === 0 ? null : <div className="sn-project-grid">
        {cards.map((card) => <ProjectCard key={card.project.id} {...card} onOpen={() => onOpen(card.project.id)} />)}
        {cards.length === 0 && <p className="col-span-full py-12 text-center text-xs text-[var(--text-tertiary)]">没有找到匹配的项目</p>}
      </div>}
    </div>
  </main>;
}
