import { useEffect, useState } from 'react';
import { NotebookPen, PanelRightClose, PanelRightOpen } from 'lucide-react';
import DocWorkspace from '../components/features/DocWorkspace';
import { getSetting, updateSetting } from '../services/tauri';

const NOTES_AGENT_COLLAPSED_KEY = 'ui:notes-agent-collapsed';

/**
 * 笔记本：个人笔记专属空间（articleType === 'manual' | 'journal'）。
 * 与 AI 解读分离，用 #标签 + 搜索组织；[[双链]] 与解读跨空间互通。
 * 进入页面只读取已有笔记；只有用户点击「新建笔记」或模板入口才创建文档。
 */
export default function Notes() {
  // 笔记本默认收起，避免首次进入挤压写作区；用户选择持久化。
  const [agentCollapsed, setAgentCollapsed] = useState(true);

  useEffect(() => {
    getSetting(NOTES_AGENT_COLLAPSED_KEY)
      .then((value) => setAgentCollapsed(value !== '0'))
      .catch(() => {});
  }, []);

  const toggleAgent = () => {
    setAgentCollapsed((value) => {
      const next = !value;
      updateSetting(NOTES_AGENT_COLLAPSED_KEY, next ? '1' : '0').catch(() => {});
      return next;
    });
  };

  return (
    <div className="h-full flex flex-col min-w-0">
      <header className="h-10 shrink-0 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-4 flex items-center justify-between" data-tauri-drag-region>
        <div className="flex items-center gap-2" data-tauri-drag-region>
          <NotebookPen size={14} className="text-[var(--accent)]" />
          <span className="text-xs font-semibold text-[var(--text-primary)]">笔记本</span>
        </div>
        <button
          type="button"
          onClick={toggleAgent}
          className={`w-7 h-7 rounded-md flex items-center justify-center transition-colors ${
            agentCollapsed
              ? 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'
              : 'bg-[var(--accent-subtle)] text-[var(--accent)]'
          }`}
          title={agentCollapsed ? '展开 Agent' : '折叠 Agent'}
        >
          {agentCollapsed ? <PanelRightOpen size={15} /> : <PanelRightClose size={15} />}
        </button>
      </header>
      <div className="flex-1 min-h-0">
        <DocWorkspace
          scope={(a) => a.articleType === 'manual' || a.articleType === 'journal'}
          listTitle="我的笔记"
          newDocLabel="新建笔记"
          emptyHint="还没有笔记。点击「新建笔记」开始写第一篇，用 #标签 归类、[[双链]] 把笔记与解读串成网络。"
          showTags
          journal
          enableTemplates
          enableStarterExamples
          agentCollapsed={agentCollapsed}
          onRequestAgent={() => {
            setAgentCollapsed(false);
            updateSetting(NOTES_AGENT_COLLAPSED_KEY, '0').catch(() => {});
          }}
        />
      </div>
    </div>
  );
}
