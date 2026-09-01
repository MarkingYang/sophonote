import { useMemo, useState } from 'react';
import { Plus, Check, Circle, Calendar, NotebookPen, FileText } from 'lucide-react';
import { useAppStore } from '../stores/appStore';
import type { Task } from '../types';
import { parseNoteTasks } from '../services/noteTasks';
import EmptyState from '../components/ui/EmptyState';

/**
 * 任务工具（DEC-041）：独立整页。全量任务管理 = 笔记任务（派生自全库 `- [ ]`）+ 独立任务。
 * 与「今日」工具的区别：今日只看截止日期维度的当天切片，本页管理全部待办。
 */

const priorityLabels = {
  1: { text: '高', color: 'text-[var(--danger)] bg-[var(--danger-subtle)]' },
  2: { text: '中', color: 'text-[var(--warning)] bg-[var(--warning-subtle)]' },
  3: { text: '低', color: 'text-[var(--text-tertiary)] bg-[var(--bg-sunken)]' },
};

/** N5：articleType → 中文标签（与收件箱语义命中卡一致） */
const typeLabel = (t: string) => (t === 'manual' ? '笔记' : t === 'journal' ? '日记' : 'AI 解读');

export default function TasksTool() {
  const { tasks, addTask, toggleTask, articles, toggleNoteTask, openArticleAtLine } = useAppStore();
  const [newTaskTitle, setNewTaskTitle] = useState('');
  const [filter, setFilter] = useState<'all' | 'active' | 'completed'>('all');

  // —— N5：笔记任务 = 实时派生，不复制数据 ——
  // 全库扫描 `- [ ]`（围栏感知，规则见 noteTasks.ts），按来源文档分组；
  // 勾选直接写回源 .md（真相源唯一），预览态勾选与此处天然双向同步。
  // 安全性：App.tsx 按页互斥挂载，本页展示时 DocWorkspace 已卸载，无编辑器心跳冲突。
  const noteGroups = useMemo(() => {
    const groups = articles
      .map((a) => ({ article: a, tasks: parseNoteTasks(a.content) }))
      .filter((g) => g.tasks.length > 0)
      .map((g) => ({
        ...g,
        tasks: g.tasks.filter((t) =>
          filter === 'active' ? !t.done : filter === 'completed' ? t.done : true
        ),
      }))
      .filter((g) => g.tasks.length > 0);
    // 组内待办优先（已完成沉底），组间保持文档原序
    return groups.map((g) => ({
      ...g,
      tasks: [...g.tasks.filter((t) => !t.done), ...g.tasks.filter((t) => t.done)],
    }));
  }, [articles, filter]);

  const filteredTasks = tasks.filter((t) => {
    if (filter === 'active') return t.status !== 'done';
    if (filter === 'completed') return t.status === 'done';
    return true;
  });

  const handleAddTask = () => {
    if (!newTaskTitle.trim()) return;
    const task: Task = {
      id: `task-${Date.now()}`,
      title: newTaskTitle.trim(),
      status: 'todo',
      priority: 2,
      createdAt: new Date().toISOString(),
    };
    addTask(task);
    setNewTaskTitle('');
  };

  const notePending = noteGroups.reduce((n, g) => n + g.tasks.filter((t) => !t.done).length, 0);
  const noteDone = noteGroups.reduce((n, g) => n + g.tasks.filter((t) => t.done).length, 0);
  const pendingCount = tasks.filter((t) => t.status !== 'done').length + notePending;
  const doneCount = tasks.filter((t) => t.status === 'done').length + noteDone;

  return (
    <div className="h-full overflow-y-auto p-5">
      <div className="max-w-3xl mx-auto">
        {/* 工具条：计数 + 筛选（40px Header 已由工具域壳承担） */}
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <span className="text-xs px-2 py-0.5 rounded-full bg-[var(--warning-subtle)] text-[var(--warning)] font-medium">
              {pendingCount} 待办
            </span>
            <span className="text-xs px-2 py-0.5 rounded-full bg-[var(--success-subtle)] text-[var(--success)] font-medium">
              {doneCount} 已完成
            </span>
          </div>
          <div className="flex items-center gap-1">
            {(['all', 'active', 'completed'] as const).map((f) => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={`px-2.5 py-1 rounded-md text-xs font-medium transition-colors ${
                  filter === f ? 'bg-[var(--accent)] text-white' : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'
                }`}
              >
                {f === 'all' ? '全部' : f === 'active' ? '待办' : '已完成'}
              </button>
            ))}
          </div>
        </div>

        {/* —— N5：笔记任务（派生自全库 `- [ ]`，勾选写回源文档） —— */}
        <section className="mb-8">
          <div className="flex items-center gap-2 mb-3">
            <NotebookPen size={14} className="text-[var(--accent)]" />
            <h3 className="text-sm font-semibold text-[var(--text-primary)]">笔记任务</h3>
            <span className="text-xs text-[var(--text-tertiary)]">来自笔记中的 - [ ] 清单，勾选直接写回原文</span>
          </div>

          {noteGroups.length === 0 ? (
            <EmptyState
              icon={NotebookPen}
              title="笔记里暂无任务清单"
              desc="在任意笔记中写 `- [ ] 事项`，会自动汇集到这里"
              className="py-8 border border-dashed border-[var(--border-default)] rounded-lg"
            />
          ) : (
            <div className="space-y-4">
              {noteGroups.map(({ article, tasks: ts }) => (
                <div key={article.id} className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] overflow-hidden">
                  {/* 来源文档头：类型 + 标题 + 行级回链入口 */}
                  <button
                    onClick={() => openArticleAtLine(article.id, ts[0].line)}
                    className="w-full flex items-center gap-2 px-3 py-2 bg-[var(--bg-sunken)] hover:bg-[var(--border-default)] transition-colors text-left border-b border-[var(--border-default)]"
                    title="打开来源笔记"
                  >
                    <FileText size={12} className="text-[var(--text-tertiary)] shrink-0" />
                    <span className="text-xs px-1.5 py-0.5 rounded bg-[var(--accent-subtle)] text-[var(--accent)] font-medium shrink-0">
                      {typeLabel(article.articleType)}
                    </span>
                    <span className="text-xs font-medium text-[var(--text-secondary)] truncate">{article.title}</span>
                    <span className="ml-auto text-xs text-[var(--text-tertiary)] shrink-0">
                      {ts.filter((t) => !t.done).length} 待办
                    </span>
                  </button>
                  <div className="divide-y divide-[var(--border-default)]">
                    {ts.map((t) => (
                      <div key={`${article.id}:${t.line}`} className="flex items-center gap-3 px-3 py-2 hover:bg-[var(--bg-sunken)] transition-colors">
                        <button
                          onClick={() =>
                            void toggleNoteTask(article.id, t.line).catch((e) =>
                              // NB-31：勾选写回失败不得成为未处理 rejection（内存已翻转，落盘失败可见于日志）
                              console.error('Failed to toggle note task:', e)
                            )
                          }
                          className={`shrink-0 w-[18px] h-[18px] rounded border-2 flex items-center justify-center transition-colors ${
                            t.done
                              ? 'bg-[var(--success)] border-[var(--success)] text-white'
                              : 'border-[var(--border-strong)] hover:border-[var(--accent)]'
                          }`}
                          title={t.done ? '标记未完成' : '标记完成'}
                        >
                          {t.done && <Check size={11} />}
                        </button>
                        {/* 点击任务文本 → 跳回来源笔记并定位到该行 */}
                        <button
                          onClick={() => openArticleAtLine(article.id, t.line)}
                          className={`flex-1 min-w-0 text-left text-sm truncate ${
                            t.done ? 'line-through text-[var(--text-tertiary)]' : 'text-[var(--text-primary)] hover:text-[var(--accent)]'
                          }`}
                          title="跳回笔记上下文"
                        >
                          {t.text || '（无描述）'}
                        </button>
                        <span className="text-xs text-[var(--text-tertiary)] shrink-0" title="源码行号（行级回链）">
                          L{t.line}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>

        {/* —— 独立任务（tasks 表，与笔记无关的待办） —— */}
        <section>
          <div className="flex items-center gap-2 mb-3">
            <Circle size={14} className="text-[var(--warning)]" />
            <h3 className="text-sm font-semibold text-[var(--text-primary)]">独立任务</h3>
            <span className="text-xs text-[var(--text-tertiary)]">不挂在笔记里的待办</span>
          </div>

          <div className="flex gap-2 mb-4">
            <input
              type="text"
              value={newTaskTitle}
              onChange={(e) => setNewTaskTitle(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleAddTask()}
              placeholder="添加新任务..."
              className="input flex-1"
            />
            <button onClick={handleAddTask} className="btn-primary flex items-center gap-1.5">
              <Plus size={14} />
              添加
            </button>
          </div>

          {filteredTasks.length === 0 ? (
            <EmptyState
              icon={Circle}
              title="暂无独立任务"
              className="py-8 border border-dashed border-[var(--border-default)] rounded-lg"
            />
          ) : (
            <div className="space-y-2">
              {filteredTasks.map((task) => {
                const isDone = task.status === 'done';
                const pLabel = priorityLabels[task.priority] || priorityLabels[2];
                return (
                  <div
                    key={task.id}
                    className={`flex items-center gap-3 p-3 rounded-lg border transition-all ${
                      isDone
                        ? 'bg-[var(--bg-sunken)] border-[var(--border-default)]'
                        : 'bg-[var(--bg-surface)] border-[var(--border-default)] hover:border-[var(--accent-border)]'
                    }`}
                  >
                    <button
                      onClick={() => toggleTask(task.id)}
                      className={`shrink-0 w-5 h-5 rounded-full border-2 flex items-center justify-center transition-colors ${
                        isDone
                          ? 'bg-[var(--success)] border-[var(--success)] text-white'
                          : 'border-[var(--border-strong)] hover:border-[var(--accent)]'
                      }`}
                    >
                      {isDone && <Check size={12} />}
                    </button>

                    <div className="flex-1 min-w-0">
                      <p className={`text-sm ${isDone ? 'line-through text-[var(--text-tertiary)]' : 'text-[var(--text-primary)] font-medium'}`}>
                        {task.title}
                      </p>
                      {task.description && (
                        <p className="text-xs text-[var(--text-tertiary)] mt-0.5">{task.description}</p>
                      )}
                    </div>

                    <span className={`text-xs px-1.5 py-0.5 rounded font-medium shrink-0 ${pLabel.color}`}>
                      {pLabel.text}
                    </span>

                    {task.dueDate && (
                      <span className="flex items-center gap-1 text-xs text-[var(--text-tertiary)] shrink-0">
                        <Calendar size={11} />
                        {new Date(task.dueDate).toLocaleDateString('zh-CN')}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
