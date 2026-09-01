// ============================================================
// Track B · 智能体演进（AG-02 · AI 工作室 = 智能体试验田 / AG-09 交互重构）
// AI 工作室 = IDE-first 项目工作台（本地文件主画布 + 右侧 Agent 协作面板）。
// AG-09：报告生成 UI 入口已隐藏；能力保留在 ReportStudio，后续由 Chat Skill / CLI 触发。
// 对话模式占位移除（并入项目模式，Chat 挂项目上）；页头副标题移除。
// 设计基线：docs/architecture.md
// ============================================================
import { useState, useEffect, useCallback, useRef } from 'react';
import { useAppStore } from '../stores/appStore';
import * as tauri from '../services/tauri';
import { generateDailyReport, generateWeeklyReport } from '../services/ai';
import { providerCredentialReady } from '../services/modelProviders';
import type { DailyLog } from '../types';
import ProjectMode from '../components/aistudio/ProjectMode';
import EmptyState from '../components/ui/EmptyState';
import {
  Sparkles, FileText, Calendar, Copy, Download,
  Save, History, ChevronRight, ChevronLeft, AlertCircle, CheckCircle,
  PanelRightClose, PanelRightOpen,
} from 'lucide-react';

interface ReportItem {
  id: string;
  title: string;
  description: string;
  type: string;
}

const promptTemplates = [
  {
    id: 'daily',
    name: '生成日报',
    icon: Calendar,
    description: '基于今日收集的内容生成技术日报',
  },
  {
    id: 'weekly',
    name: '生成周报',
    icon: FileText,
    description: '基于本周内容生成技术周报',
  },
];

// DEC-036：AI 是 IDE 的协作侧栏，默认展开；用户可从右上角收起。
const AGENT_COLLAPSED_KEY = 'ui:project-agent-collapsed';

export default function AIStudio() {
  const [agentCollapsed, setAgentCollapsed] = useState(false);

  useEffect(() => {
    tauri.getSetting(AGENT_COLLAPSED_KEY)
      .then((v) => setAgentCollapsed(v === '1'))
      .catch(() => {});
  }, []);

  const toggleAgentCollapsed = () => {
    setAgentCollapsed((v) => {
      const next = !v;
      tauri.updateSetting(AGENT_COLLAPSED_KEY, next ? '1' : '0').catch(() => {});
      return next;
    });
  };

  return (
    <div className="flex flex-col h-full">
      {/* DEC-036：右上角控制 IDE 的 AI 协作侧栏。 */}
      <div className="px-4 h-10 border-b border-[var(--border-default)] bg-[var(--bg-surface)] flex items-center justify-between gap-1 shrink-0" data-tauri-drag-region>
        <div className="flex items-center gap-2" data-tauri-drag-region>
          <Sparkles size={14} className="text-[var(--accent)]" />
          <span className="text-xs font-semibold text-[var(--text-primary)]" data-tauri-drag-region>工作室</span>
        </div>
        <button
          onClick={toggleAgentCollapsed}
          className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] transition-colors"
          title={agentCollapsed ? '打开 AI 助手' : '收起 AI 助手'}
          aria-label={agentCollapsed ? '打开 AI 助手' : '收起 AI 助手'}
        >
          {agentCollapsed ? <PanelRightOpen size={15} /> : <PanelRightClose size={15} />}
        </button>
      </div>

      {/* NB-27：补列 flex 上下文——ProjectMode 高度从「h-full 百分比对本层 flex 计算高」
          改为「flex-1 主轴取高」。百分比对 flex 计算的父高在 WebKit 下解析不稳（NB-26 同因），
          此前项目空间预览/编辑滚动整链失效而笔记本空间正常，差异即此层 */}
      <div className="flex-1 min-h-0 flex flex-col">
        <ProjectMode agentCollapsed={agentCollapsed} />
      </div>
    </div>
  );
}

/** 报告生成能力本体（AG-09）：无顶栏入口；供后续 Chat Skill / CLI 挂载。 */
export function ReportStudio({ onBack }: { onBack: () => void }) {
  const { items, settings, initialized, apiKeys } = useAppStore();
  const [selectedTemplate, setSelectedTemplate] = useState<string>('daily');
  const [isGenerating, setIsGenerating] = useState(false);
  const [generatedContent, setGeneratedContent] = useState('');
  const [logs, setLogs] = useState<DailyLog[]>([]);
  const [selectedLog, setSelectedLog] = useState<DailyLog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [activeTab, setActiveTab] = useState<'generate' | 'history'>('generate');
  const abortRef = useRef<AbortController | null>(null);

  const aiCfg = settings.aiConfig ?? { activeProvider: '', providers: {} };
  const activeProviderCfg = aiCfg.providers?.[aiCfg.activeProvider];
  const aiEnabled = activeProviderCfg ? providerCredentialReady(activeProviderCfg, apiKeys) : false;

  // 加载历史日报
  const loadLogs = useCallback(async () => {
    try {
      const data = await tauri.getLogs(selectedTemplate === 'daily' ? 'daily' : 'weekly');
      setLogs(data.map((l: any) => {
        let sources = l.sources;
        if (typeof sources === 'string') {
          try { sources = JSON.parse(sources); } catch { sources = []; }
        }
        return { ...l, sources: sources || [] };
      }));
    } catch (e) {
      console.error('Failed to load logs:', e);
    }
  }, [selectedTemplate]);

  useEffect(() => {
    if (initialized) loadLogs();
  }, [initialized, loadLogs]);

  // 获取今日/本周素材
  const getSourceItems = (): ReportItem[] => {
    const now = new Date();
    return items
      .filter((i) => {
        const itemDate = new Date(i.fetchedAt);
        if (selectedTemplate === 'daily') {
          return itemDate.toDateString() === now.toDateString();
        }
        // 本周
        const weekAgo = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000);
        return itemDate >= weekAgo;
      })
      .map((i) => ({
        id: i.id,
        title: i.title,
        description: (i.aiSummary || i.description || '').slice(0, 300),
        type: i.type,
      }));
  };

  const sourceItems = getSourceItems();

  const handleGenerate = async () => {
    if (!aiEnabled) {
      setError('请先配置 Deepseek API Key（设置 → AI 配置）');
      return;
    }
    if (sourceItems.length === 0) {
      setError('暂无素材，请先在「发现」页抓取内容');
      return;
    }

    setError(null);
    setIsGenerating(true);
    setGeneratedContent('');
    setSaved(false);

    abortRef.current = new AbortController();

    try {
      // 故事级合并信号（借鉴 ai-news-radar stories-merged）：多源报道的条目标注交叉验证
      let annotated = sourceItems;
      try {
        const stories = await tauri.getStories(200);
        const multi = new Map<string, number>();
        for (const s of stories) {
          if (s.signalLevel === 'multi') s.itemIds.forEach((id) => multi.set(id, s.sourceCount));
        }
        if (multi.size > 0) {
          annotated = sourceItems.map((i) =>
            multi.has(i.id) ? { ...i, title: `${i.title}（多源验证：${multi.get(i.id)} 个信源）` } : i
          );
        }
      } catch {
        // stories 不可用时静默降级，不影响报告生成
      }
      const generate = selectedTemplate === 'daily' ? generateDailyReport : generateWeeklyReport;
      const result = await generate(annotated);
      setGeneratedContent(result);
    } catch (e: any) {
      setError(e.message || '生成失败，请检查网络或 API Key');
    } finally {
      setIsGenerating(false);
      abortRef.current = null;
    }
  };

  const handleSave = async () => {
    if (!generatedContent) return;
    try {
      const now = new Date();
      const dateStr = now.toISOString().split('T')[0];
      const id = `${selectedTemplate}-${dateStr}`;

      const log = {
        id,
        date: dateStr,
        logType: selectedTemplate,
        content: generatedContent,
        sources: JSON.stringify(sourceItems.map((i) => i.id)),
        generatedBy: 'ai',
        createdAt: now.toISOString(),
      };

      await tauri.insertLog(log as any);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      loadLogs();
    } catch (e) {
      setError('保存失败');
    }
  };

  const handleCopy = () => {
    navigator.clipboard.writeText(generatedContent);
  };

  const handleDownload = () => {
    const blob = new Blob([generatedContent], { type: 'text/markdown' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `sophonote-${selectedTemplate}-${new Date().toISOString().split('T')[0]}.md`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const renderMarkdown = (content: string) => {
    return content.split('\n').map((line, i) => {
      if (line.startsWith('# ')) return <h1 key={i} className="text-xl font-bold text-[var(--text-primary)] mt-0 mb-3">{line.slice(2)}</h1>;
      if (line.startsWith('## ')) return <h2 key={i} className="text-lg font-bold text-[var(--text-primary)] mt-5 mb-2 pb-1 border-b border-[var(--border-default)]">{line.slice(3)}</h2>;
      if (line.startsWith('### ')) return <h3 key={i} className="text-base font-semibold text-[var(--text-primary)] mt-4 mb-2">{line.slice(4)}</h3>;
      if (line.startsWith('#### ')) return <h4 key={i} className="text-sm font-semibold text-[var(--text-primary)] mt-3 mb-1">{line.slice(5)}</h4>;
      if (line.startsWith('**') && line.endsWith('**')) return <p key={i} className="font-semibold text-[var(--text-primary)] mt-2">{line.replace(/\*\*/g, '')}</p>;
      if (line.startsWith('- ')) return <li key={i} className="text-sm text-[var(--text-secondary)] ml-4 leading-relaxed">{line.slice(2)}</li>;
      if (/^\d+\.\s/.test(line)) return <li key={i} className="text-sm text-[var(--text-secondary)] ml-4 leading-relaxed">{line.replace(/^\d+\.\s/, '')}</li>;
      if (line.startsWith('---')) return <hr key={i} className="my-4 border-[var(--border-default)]" />;
      if (line.trim() === '') return <div key={i} className="h-2" />;
      if (line.startsWith('> ')) return <blockquote key={i} className="text-sm text-[var(--text-tertiary)] italic border-l-2 border-[var(--accent-border)] pl-3 my-2">{line.slice(2)}</blockquote>;
      if (line.startsWith('```')) return null;
      return <p key={i} className="text-sm text-[var(--text-secondary)] leading-relaxed">{line}</p>;
    });
  };

  return (
    <div className="flex h-full">
      {/* 左侧：模板选择 + 历史 */}
      {/* NB-24：首行 h-10 底线与右栏对齐；Tab 切换移入首行 */}
      <div className="w-64 border-r border-[var(--border-default)] bg-[var(--bg-sunken)] flex flex-col shrink-0">
        <header className="h-10 border-b border-[var(--border-default)] flex items-center px-3 shrink-0" data-tauri-drag-region>
          <div className="flex gap-1 p-1 bg-[var(--bg-canvas)] rounded-lg w-full">
          <button
            onClick={() => setActiveTab('generate')}
            className={`flex-1 py-1 px-2 rounded-md text-xs font-medium transition-all ${
              activeTab === 'generate' ? 'bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] text-[var(--text-primary)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'
            }`}
          >
            <Sparkles size={12} className="inline mr-1" />
            生成
          </button>
          <button
            onClick={() => setActiveTab('history')}
            className={`flex-1 py-1 px-2 rounded-md text-xs font-medium transition-all ${
              activeTab === 'history' ? 'bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] text-[var(--text-primary)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'
            }`}
          >
            <History size={12} className="inline mr-1" />
            历史
          </button>
          </div>
        </header>
        <div className="flex-1 overflow-y-auto p-4">
        {activeTab === 'generate' ? (
          <>
            <h3 className="text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider mb-3">AI 模板</h3>
            <div className="space-y-2">
              {promptTemplates.map((t) => {
                const Icon = t.icon;
                const selected = selectedTemplate === t.id && !selectedLog;
                return (
                  <button
                    key={t.id}
                    onClick={() => { setSelectedTemplate(t.id); setSelectedLog(null); }}
                    className={`w-full p-3 rounded-xl text-left transition-all border ${
                      selected
                        ? 'bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] border-[var(--accent-border)]'
                        : 'hover:bg-[var(--bg-surface)] border-transparent'
                    }`}
                  >
                    <div className="flex items-center gap-2">
                      <Icon size={16} className={selected ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)]'} />
                      <span className={`text-sm font-medium ${selected ? 'text-[var(--accent-strong)]' : 'text-[var(--text-secondary)]'}`}>
                        {t.name}
                      </span>
                    </div>
                    <p className="text-xs text-[var(--text-tertiary)] mt-1">{t.description}</p>
                  </button>
                );
              })}
            </div>

            <div className="mt-6 p-3 rounded-lg bg-[var(--bg-surface)] border border-[var(--border-default)]">
              <p className="text-xs text-[var(--text-tertiary)]">
                可用素材：<span className="font-semibold text-[var(--text-primary)]">{sourceItems.length}</span> 条
              </p>
              <p className="text-xs text-[var(--text-tertiary)] mt-0.5">
                AI 模型：{activeProviderCfg?.model || '未配置'}（{activeProviderCfg?.name || '未启用供应商'}）
              </p>
              {!aiEnabled && (
                <p className="text-xs text-[var(--warning)] mt-1.5 flex items-center gap-1">
                  <AlertCircle size={11} />
                  API Key 未配置
                </p>
              )}
            </div>
          </>
        ) : (
          <>
            <h3 className="text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider mb-3">历史记录</h3>
            <div className="space-y-1">
              {logs.length === 0 ? (
                <p className="text-xs text-[var(--text-tertiary)] text-center py-4">暂无历史记录</p>
              ) : (
                logs.map((log) => (
                  <button
                    key={log.id}
                    onClick={() => { setSelectedLog(log); setGeneratedContent(log.content); }}
                    className={`w-full p-2.5 rounded-lg text-left transition-all border ${
                      selectedLog?.id === log.id
                        ? 'bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] border-[var(--accent-border)]'
                        : 'hover:bg-[var(--bg-surface)] border-transparent'
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="text-xs font-medium text-[var(--text-secondary)]">
                        {log.type === 'daily' ? '日报' : '周报'} · {log.date}
                      </span>
                      <ChevronRight size={12} className="text-[var(--text-tertiary)]" />
                    </div>
                    <p className="text-xs text-[var(--text-tertiary)] mt-0.5 truncate">
                      {log.content.slice(0, 60)}...
                    </p>
                  </button>
                ))
              )}
            </div>
          </>
        )}
        </div>
      </div>

      {/* 右侧：生成结果 */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* NB-24：首行 h-10，底线与右栏首行对齐 */}
        <header className="px-5 h-10 border-b border-[var(--border-default)] flex items-center justify-between bg-[var(--bg-surface)]" data-tauri-drag-region>
          <div className="flex items-center gap-3 min-w-0">
            <button
              onClick={onBack}
              className="flex items-center gap-0.5 text-xs text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors shrink-0"
            >
              <ChevronLeft size={14} />
              返回项目
            </button>
            <h2 className="text-base font-semibold text-[var(--text-primary)] truncate" data-tauri-drag-region>
            {selectedLog ? `${selectedLog.type === 'daily' ? '日报' : '周报'} · ${selectedLog.date}` :
              selectedTemplate === 'daily' ? '生成技术日报' : '生成技术周报'}
            </h2>
          </div>
          <div className="flex items-center gap-2">
            {generatedContent && (
              <>
                <button
                  onClick={handleCopy}
                  className="btn-secondary flex items-center gap-1.5 py-1.5 px-3 text-xs"
                >
                  <Copy size={13} />
                  复制
                </button>
                <button
                  onClick={handleDownload}
                  className="btn-secondary flex items-center gap-1.5 py-1.5 px-3 text-xs"
                >
                  <Download size={13} />
                  导出
                </button>
                {!selectedLog && (
                  <button
                    onClick={handleSave}
                    disabled={saved}
                    className={`flex items-center gap-1.5 py-1.5 px-3 text-xs rounded-lg font-medium transition-all ${
                      saved
                        ? 'bg-[var(--success-subtle)] text-[var(--success)]'
                        : 'bg-[var(--accent)] text-white hover:bg-[var(--accent-strong)]'
                    }`}
                  >
                    {saved ? <CheckCircle size={13} /> : <Save size={13} />}
                    {saved ? '已保存' : '保存'}
                  </button>
                )}
              </>
            )}
            {!selectedLog && (
              <button
                onClick={handleGenerate}
                disabled={isGenerating}
                className="btn-primary flex items-center gap-1.5 py-1.5 px-3 text-xs disabled:opacity-50"
              >
                <Sparkles size={13} />
                {isGenerating ? '生成中...' : '生成'}
              </button>
            )}
          </div>
        </header>

        <div className="flex-1 overflow-y-auto p-5">
          {error && (
            <div className="mb-4 p-3 rounded-lg bg-[var(--danger-subtle)] border border-[var(--danger)] text-[var(--danger)] text-sm flex items-center gap-2">
              <AlertCircle size={14} />
              {error}
            </div>
          )}

          {isGenerating ? (
            <div className="flex flex-col items-center justify-center h-full text-[var(--text-tertiary)]">
              <div className="w-8 h-8 border-2 border-[var(--accent-border)] border-t-[var(--accent)] rounded-full animate-spin mb-3" />
              <p className="text-sm">AI 正在分析内容并生成报告...</p>
              <p className="text-xs mt-1">基于 {sourceItems.length} 条素材 · Deepseek API</p>
            </div>
          ) : generatedContent ? (
            <div className="max-w-3xl mx-auto">
              <div className="bg-[var(--bg-surface)] rounded-xl border border-[var(--border-default)] p-6 shadow-[var(--shadow-sm)]">
                <div className="prose prose-sm max-w-none">
                  {renderMarkdown(generatedContent)}
                </div>
              </div>
            </div>
          ) : (
            <EmptyState
              icon={Sparkles}
              title="选择模板并点击「生成」开始"
              desc={sourceItems.length > 0
                ? `基于 ${sourceItems.length} 条素材生成`
                : '暂无素材，请先在「发现」页抓取内容'}
              className="h-full"
            />
          )}
        </div>
      </div>
    </div>
  );
}
