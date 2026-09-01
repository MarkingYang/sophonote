import { memo, useEffect, useRef, useState } from 'react';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { useShallow } from 'zustand/react/shallow';
import { useAppStore } from '../../stores/appStore';
import { type EnrichResult, type EvidenceItem } from '../../services/ai';
import { getDeepDiveByItem, getItemContent, getItemEnrich, type ItemContent } from '../../services/tauri';
import MarkdownView from './MarkdownView';
import MarkdownEditor, { type MarkdownEditorHandle } from '../editor/MarkdownEditor';
import {
  X, ExternalLink, Star, Archive, Trash2, Zap, BookOpen, ArrowLeft,
  Loader2, GitFork, Clock, User, Code2, Database,
  ShieldCheck, ShieldAlert, ShieldQuestion, ChevronDown, Pencil, Check, NotebookPen,
} from 'lucide-react';

const typeLabels: Record<string, { text: string; color: string; bg: string }> = {
  repo: { text: '仓库', color: 'var(--dot-repo)', bg: 'color-mix(in srgb, var(--dot-repo) 12%, transparent)' },
  paper: { text: '论文', color: 'var(--dot-paper)', bg: 'color-mix(in srgb, var(--dot-paper) 12%, transparent)' },
  product: { text: '产品', color: 'var(--dot-product)', bg: 'color-mix(in srgb, var(--dot-product) 12%, transparent)' },
  article: { text: '文章', color: 'var(--dot-article)', bg: 'color-mix(in srgb, var(--dot-article) 12%, transparent)' },
  model: { text: '模型', color: 'var(--dot-model)', bg: 'color-mix(in srgb, var(--dot-model) 12%, transparent)' },
};

const confidenceMeta: Record<string, { label: string; cls: string; Icon: typeof ShieldCheck }> = {
  high: { label: '高可信', cls: 'text-[var(--success)]', Icon: ShieldCheck },
  medium: { label: '中可信', cls: 'text-[var(--warning)]', Icon: ShieldAlert },
  low: { label: '低可信', cls: 'text-[var(--text-tertiary)]', Icon: ShieldQuestion },
};

// 证据条目（DB 中部分来源额外带 title，兼容展示）
type EvidenceView = EvidenceItem & { title?: string };

/**
 * 沉浸式阅读视图：点击条目后全屏展开，像读一篇文章。
 * 上区「速览」——一句话定位 + 关键点/证据/风险/置信度；下区「深度解读」——长文 Markdown。
 */
function ItemDetail() {
  const {
    items, articles, selectedItemId, setSelectedItemId,
    starItem, archiveItem, deleteItem, setActivePage,
    updateArticleContent, saveItemAIEdit, updateItemStatus, sedimentToNote,
    upsertArticle,
  } = useAppStore(useShallow((state) => ({
    items: state.items,
    articles: state.articles,
    selectedItemId: state.selectedItemId,
    setSelectedItemId: state.setSelectedItemId,
    starItem: state.starItem,
    archiveItem: state.archiveItem,
    deleteItem: state.deleteItem,
    setActivePage: state.setActivePage,
    updateArticleContent: state.updateArticleContent,
    saveItemAIEdit: state.saveItemAIEdit,
    updateItemStatus: state.updateItemStatus,
    sedimentToNote: state.sedimentToNote,
    upsertArticle: state.upsertArticle,
  })));

  const item = items.find((i) => i.id === selectedItemId) || null;
  const [diveLoading, setDiveLoading] = useState(false);
  const [diveLoadError, setDiveLoadError] = useState('');
  const [enrichError, setEnrichError] = useState('');
  const [enrich, setEnrich] = useState<EnrichResult | null>(null);
  const [content, setContent] = useState<{ loading: boolean; data?: ItemContent | null; error?: string }>({ loading: false });
  // 速览编辑态（结构化表单；无结构化结果时为纯文本兜底）
  const [editingEnrich, setEditingEnrich] = useState(false);
  const [draftEnrich, setDraftEnrich] = useState<EnrichResult | null>(null);
  const [draftSummary, setDraftSummary] = useState('');
  const [draftTags, setDraftTags] = useState('');
  const [savingEnrich, setSavingEnrich] = useState(false);
  // 深度解读就地编辑态（Milkdown）
  const [editingDive, setEditingDive] = useState(false);
  const [savingDive, setSavingDive] = useState(false);
  const [diveSaveError, setDiveSaveError] = useState('');
  // N1 一键沉淀
  const [sedimenting, setSedimenting] = useState(false);
  const diveEditorRef = useRef<MarkdownEditorHandle>(null);

  // Esc 关闭阅读视图
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setSelectedItemId(null);
    };
    if (selectedItemId) window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [selectedItemId, setSelectedItemId]);

  // 切换条目时重置状态
  useEffect(() => {
    setDiveLoading(false);
    setDiveLoadError('');
    setEnrichError('');
    setEnrich(null);
    setContent({ loading: false });
    setEditingEnrich(false);
    setDraftEnrich(null);
    setEditingDive(false);
    setDiveSaveError('');
    setSedimenting(false);
  }, [selectedItemId]);

  // 打开条目：并行拉取正文内容层、速览结构化结果，以及按 itemId 精确读取深度解读。
  // ISSUE-044：不能以最近 200 篇 articles 列表缓存判断深度解读是否存在。
  useEffect(() => {
    if (!selectedItemId) return;
    // A3 信号补全：打开即标已读（仅 unread 时落库，不覆盖 starred/archived），
    // 供补抓优先级「用户打开过」使用；getState 读取避免 items 进依赖引发重复拉取
    const opened = useAppStore.getState().items.find((i) => i.id === selectedItemId);
    if (opened && opened.status === 'unread') updateItemStatus(selectedItemId, 'read');
    let cancelled = false;
    setContent({ loading: true });
    getItemContent(selectedItemId)
      .then((data) => { if (!cancelled) setContent({ loading: false, data }); })
      .catch((e) => { if (!cancelled) setContent({ loading: false, error: e instanceof Error ? e.message : String(e) }); });
    getItemEnrich(selectedItemId)
      .then((json) => {
        if (cancelled || !json) return;
        try { setEnrich(JSON.parse(json) as EnrichResult); } catch { /* 旧数据兼容 */ }
      })
      .catch(() => {});
    setDiveLoading(true);
    setDiveLoadError('');
    getDeepDiveByItem(selectedItemId)
      .then((article) => {
        if (!cancelled && article) upsertArticle(article);
      })
      .catch((e) => {
        if (!cancelled) setDiveLoadError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setDiveLoading(false);
      });
    return () => { cancelled = true; };
  }, [selectedItemId, updateItemStatus, upsertArticle]);

  if (!item) return null;

  const typeInfo = typeLabels[item.type] || typeLabels.article;
  const isStarred = item.status === 'starred';
  const savedArticle = articles.find((a) => a.itemId === item.id && a.articleType === 'deep-dive');
  const diveContent = savedArticle?.content;

  // 内容层门禁：unsupported 或 quality<2 不生成 AI 解读（P0-4）
  const aiBlocked = content.data
    ? content.data.status === 'unsupported' || content.data.qualityLevel < 2
    : false;
  const aiBlockReason = content.data?.status === 'unsupported'
    ? content.data.errorMessage || '该来源证据不足，暂不生成 AI 解读'
    : content.data && content.data.qualityLevel < 2
      ? '未获取到有效正文，证据不足'
      : null;
  const evidenceItems: EvidenceView[] = (() => {
    try {
      return content.data?.evidenceJson ? (JSON.parse(content.data.evidenceJson) as EvidenceView[]) : [];
    } catch {
      return [];
    }
  })();
  const evidenceMap = new Map(evidenceItems.map((e) => [e.id, e]));
  const qualityLabels = ['只有标题', '只有简介', '摘要/正文', '正文+多源证据'];
  const qualityLabel = content.data ? qualityLabels[Math.min(content.data.qualityLevel, 3)] : null;

  const close = () => setSelectedItemId(null);

  const handleDelete = async () => {
    if (await confirmDialog('删除这条内容？将同时删除其向量索引，且不可恢复。', { title: 'SophoNote', kind: 'warning' })) {
      await deleteItem(item.id);
      close();
    }
  };

  // ===== 速览手动编辑 =====
  const startEditEnrich = () => {
    setDraftTags((item.aiTags ?? []).join('，'));
    if (enrich) {
      setDraftEnrich(JSON.parse(JSON.stringify(enrich)) as EnrichResult);
    } else {
      setDraftEnrich(null);
      setDraftSummary(item.aiSummary || '');
    }
    setEnrichError('');
    setEditingEnrich(true);
  };

  const saveEditEnrich = async () => {
    setSavingEnrich(true);
    setEnrichError('');
    try {
      const tags = draftTags.split(/[,，、]/).map((s) => s.trim()).filter(Boolean);
      if (draftEnrich) {
        const edited: EnrichResult = {
          ...draftEnrich,
          tags,
          keyPoints: draftEnrich.keyPoints.filter((k) => k.text.trim()),
          risks: draftEnrich.risks.filter((r) => r.trim()),
        };
        await saveItemAIEdit(item.id, tags, edited, '');
        setEnrich(edited);
      } else {
        await saveItemAIEdit(item.id, tags, null, draftSummary);
      }
      setEditingEnrich(false);
    } catch (e) {
      setEnrichError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavingEnrich(false);
    }
  };

  // ===== 深度解读就地编辑 =====
  const saveDive = async () => {
    if (!savedArticle) return;
    setSavingDive(true);
    setDiveSaveError('');
    try {
      const md = diveEditorRef.current?.getMarkdown() ?? '';
      await updateArticleContent(savedArticle.id, md);
      setEditingDive(false);
    } catch (e) {
      setDiveSaveError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavingDive(false);
    }
  };

  // N1 一键沉淀：生成带来源反链与证据的笔记并跳转笔记本（已沉淀则直接打开）
  const manualNote = articles.find((a) => a.itemId === item.id && a.articleType === 'manual');
  const handleSediment = async () => {
    setSedimenting(true);
    try {
      const r = await sedimentToNote(item.id);
      if (r) close();
    } catch (e) {
      // NB-31：沉淀创建失败不跳转不关闭（可重试），记录错误
      console.error('Sediment to note failed:', e);
    } finally {
      setSedimenting(false);
    }
  };

  const conf = confidenceMeta[enrich?.confidence || ''] || null;

  return (
    <div className="fixed inset-0 z-50 bg-[var(--bg-canvas)] overflow-y-auto">
      {/* 顶栏：返回 + 快捷操作（吸顶） */}
      <div className="sticky top-0 z-10 bg-[color-mix(in_srgb,var(--bg-canvas)_90%,transparent)] backdrop-blur border-b border-border">
        <div className="max-w-[760px] mx-auto px-6 h-10 flex items-center justify-between">
          <button
            onClick={close}
            className="flex items-center gap-1.5 text-xs text-[var(--text-tertiary)] hover:text-[var(--text-primary)] transition-colors"
          >
            <ArrowLeft size={14} /> 返回
          </button>
          <div className="flex items-center gap-1">
            <button
              onClick={() => starItem(item.id)}
              className={`p-1.5 rounded-md transition-colors ${isStarred ? 'text-[var(--gold)]' : 'text-[var(--text-tertiary)] hover:text-[var(--gold)] hover:bg-[var(--bg-sunken)]'}`}
              title={isStarred ? '取消收藏' : '收藏'}
            >
              <Star size={15} fill={isStarred ? 'currentColor' : 'none'} />
            </button>
            <button
              onClick={() => archiveItem(item.id)}
              className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] transition-colors"
              title="归档"
            >
              <Archive size={15} />
            </button>
            <button
              onClick={() => void handleSediment()}
              disabled={sedimenting}
              className={`p-1.5 rounded-md transition-colors disabled:opacity-50 ${manualNote ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)] hover:text-[var(--accent)] hover:bg-[var(--bg-sunken)]'}`}
              title={manualNote ? '打开已沉淀的笔记' : '存为笔记（带来源与证据）'}
            >
              {sedimenting ? <Loader2 size={15} className="animate-spin" /> : <NotebookPen size={15} />}
            </button>
            {item.url && (
              <a
                href={item.url}
                target="_blank"
                rel="noopener noreferrer"
                className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--accent)] hover:bg-[var(--bg-sunken)] transition-colors"
                title="查看原文"
              >
                <ExternalLink size={15} />
              </a>
            )}
            <button
              onClick={handleDelete}
              className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--danger)] hover:bg-[var(--danger-subtle)] transition-colors"
              title="删除"
            >
              <Trash2 size={15} />
            </button>
            <div className="w-px h-4 bg-border mx-1" />
            <button
              onClick={close}
              className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] transition-colors"
              title="关闭（Esc）"
            >
              <X size={16} />
            </button>
          </div>
        </div>
      </div>

      {/* 阅读栏 */}
      <div className="max-w-[760px] mx-auto px-6 pt-10 pb-24">
        {/* 刊头 */}
        <header className="mb-8">
          <div className="flex items-center gap-2 text-[12px] text-[var(--text-tertiary)] mb-3">
            <span
              className="font-bold px-2 py-0.5 rounded-full"
              style={{ color: typeInfo.color, backgroundColor: typeInfo.bg }}
            >
              {typeInfo.text}
            </span>
            <span className="flex items-center gap-1"><Database size={11} /><span className="font-mono text-[13px] px-1.5 rounded-[6px] bg-[var(--bg-sunken)]">{item.sourceId}</span></span>
            <span className="flex items-center gap-1"><Clock size={11} />{new Date(item.publishedAt).toLocaleDateString('zh-CN')}</span>
          </div>
          <h1 className="text-[26px] leading-snug font-extrabold tracking-tight text-[var(--text-primary)] break-all">
            {item.title}
          </h1>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 mt-3 text-[12px] text-[var(--text-tertiary)]">
            {item.author && <span className="flex items-center gap-1"><User size={12} />{item.author}</span>}
            {item.language && <span className="flex items-center gap-1"><Code2 size={12} />{item.language}</span>}
            {item.stars != null && (
              <span className="flex items-center gap-1 text-[var(--gold)]"><Star size={12} fill="currentColor" />{item.stars.toLocaleString()}</span>
            )}
            {item.forks != null && <span className="flex items-center gap-1"><GitFork size={12} />{item.forks.toLocaleString()}</span>}
            {item.aiTags?.map((tag) => (
              <span key={tag} className="hb-chip">
                {tag}
              </span>
            ))}
          </div>
        </header>

        {/* ========== 上区：速览 ========== */}
        <section aria-label="速览">
          <div className="flex items-center justify-between mb-4">
            <h2 className="flex items-center gap-1.5 text-[12px] font-semibold uppercase tracking-widest text-[var(--text-primary)]">
              <Zap size={12} className="text-[var(--accent)]" /> 速览
            </h2>
            <div className="flex items-center gap-2">
              {!editingEnrich && item.aiSummary && (
                <button
                  onClick={startEditEnrich}
                  className="flex items-center gap-1 text-[12px] text-[var(--accent)] hover:underline"
                >
                  <Pencil size={11} /> 编辑
                </button>
              )}
            </div>
          </div>

          {enrichError && <p className="text-[12px] text-[var(--danger)] mb-3">✗ {enrichError}</p>}

          {editingEnrich ? (
            <div className="space-y-4 rounded-xl border border-border bg-[var(--bg-surface)] p-5">
              {draftEnrich ? (
                <>
                  <div>
                    <label className="block text-[12px] font-semibold text-[var(--text-tertiary)] mb-1.5">概要（一句话定位）</label>
                    <textarea
                      value={draftEnrich.summary}
                      rows={2}
                      onChange={(e) => setDraftEnrich({ ...draftEnrich, summary: e.target.value })}
                      className="w-full rounded-md border border-border bg-[var(--bg-sunken)] p-2 text-sm leading-6 text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)] resize-y"
                    />
                  </div>
                  <div>
                    <label className="block text-[12px] font-semibold text-[var(--text-tertiary)] mb-1.5">关键点（证据 [Ex] 关联自动保留）</label>
                    <div className="space-y-2">
                      {draftEnrich.keyPoints.map((k, idx) => (
                        <div key={idx} className="flex items-start gap-2">
                          <textarea
                            value={k.text}
                            rows={2}
                            onChange={(e) =>
                              setDraftEnrich({
                                ...draftEnrich,
                                keyPoints: draftEnrich.keyPoints.map((p, i) => (i === idx ? { ...p, text: e.target.value } : p)),
                              })
                            }
                            className="flex-1 rounded-md border border-border bg-[var(--bg-sunken)] p-2 text-sm leading-6 text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)] resize-y"
                          />
                          <button
                            onClick={() => setDraftEnrich({ ...draftEnrich, keyPoints: draftEnrich.keyPoints.filter((_, i) => i !== idx) })}
                            className="mt-1.5 p-1 text-[var(--text-tertiary)] hover:text-[var(--danger)] transition-colors"
                            title="删除关键点"
                          >
                            <X size={13} />
                          </button>
                        </div>
                      ))}
                    </div>
                    <button
                      onClick={() => setDraftEnrich({ ...draftEnrich, keyPoints: [...draftEnrich.keyPoints, { text: '', evidence: [] }] })}
                      className="mt-2 text-[12px] text-[var(--accent)] hover:underline"
                    >
                      + 添加关键点
                    </button>
                  </div>
                  <div>
                    <label className="block text-[12px] font-semibold text-[var(--text-tertiary)] mb-1.5">为何重要</label>
                    <textarea
                      value={draftEnrich.whyImportant}
                      rows={2}
                      onChange={(e) => setDraftEnrich({ ...draftEnrich, whyImportant: e.target.value })}
                      className="w-full rounded-md border border-border bg-[var(--bg-sunken)] p-2 text-sm leading-6 text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)] resize-y"
                    />
                  </div>
                  <div>
                    <label className="block text-[12px] font-semibold text-[var(--text-tertiary)] mb-1.5">风险与限制</label>
                    <div className="space-y-2">
                      {draftEnrich.risks.map((r, idx) => (
                        <div key={idx} className="flex items-start gap-2">
                          <textarea
                            value={r}
                            rows={1}
                            onChange={(e) =>
                              setDraftEnrich({ ...draftEnrich, risks: draftEnrich.risks.map((x, i) => (i === idx ? e.target.value : x)) })
                            }
                            className="flex-1 rounded-md border border-border bg-[var(--bg-sunken)] p-2 text-sm leading-6 text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)] resize-y"
                          />
                          <button
                            onClick={() => setDraftEnrich({ ...draftEnrich, risks: draftEnrich.risks.filter((_, i) => i !== idx) })}
                            className="mt-1.5 p-1 text-[var(--text-tertiary)] hover:text-[var(--danger)] transition-colors"
                            title="删除风险"
                          >
                            <X size={13} />
                          </button>
                        </div>
                      ))}
                    </div>
                    <button
                      onClick={() => setDraftEnrich({ ...draftEnrich, risks: [...draftEnrich.risks, ''] })}
                      className="mt-2 text-[12px] text-[var(--warning)] hover:underline"
                    >
                      + 添加风险
                    </button>
                  </div>
                </>
              ) : (
                <div>
                  <label className="block text-[12px] font-semibold text-[var(--text-tertiary)] mb-1.5">速览正文</label>
                  <textarea
                    value={draftSummary}
                    rows={6}
                    onChange={(e) => setDraftSummary(e.target.value)}
                    className="w-full rounded-md border border-border bg-[var(--bg-sunken)] p-2 text-sm leading-6 text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)] resize-y"
                  />
                </div>
              )}
              <div>
                <label className="block text-[12px] font-semibold text-[var(--text-tertiary)] mb-1.5">标签（用 ，或 、 分隔）</label>
                <input
                  value={draftTags}
                  onChange={(e) => setDraftTags(e.target.value)}
                  className="w-full rounded-md border border-border bg-[var(--bg-sunken)] px-2.5 py-1.5 text-[12px] text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)]"
                />
              </div>
              <div className="flex items-center gap-2 pt-1">
                <button
                  onClick={saveEditEnrich}
                  disabled={savingEnrich}
                  className="flex items-center gap-1 text-xs px-3 py-1.5 rounded-lg bg-[var(--accent)] text-white hover:bg-[var(--accent-strong)] disabled:opacity-50"
                >
                  {savingEnrich ? <Loader2 size={12} className="animate-spin" /> : <Check size={12} />} 保存
                </button>
                <button
                  onClick={() => { setEditingEnrich(false); setEnrichError(''); }}
                  className="flex items-center gap-1 text-xs px-3 py-1.5 rounded-lg border border-border text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]"
                >
                  <X size={12} /> 取消
                </button>
              </div>
            </div>
          ) : item.aiSummary && (
            <div className="space-y-5">
              {/* 关键点 + 证据标注（证据轨：左侧色条 + [Ex] 角标） */}
              {enrich && enrich.keyPoints.length > 0 ? (
                <ul className="space-y-3">
                  {enrich.keyPoints.map((k, idx) => (
                    <li
                      key={idx}
                      className="border-l-2 border-[var(--accent-border)] pl-3 text-[14px] leading-6 text-[var(--text-secondary)]"
                    >
                      {k.text}
                      {k.evidence.length > 0 && (
                        <span className="inline-flex gap-1 ml-2 align-middle">
                          {k.evidence.map((eid) => {
                            const ev = evidenceMap.get(eid);
                            const chip = (
                              <span className="font-mono text-[13px] px-1.5 py-0.5 rounded-[6px] bg-[var(--bg-sunken)] text-[var(--accent)]">
                                {eid}
                              </span>
                            );
                            return ev?.url ? (
                              <a key={eid} href={ev.url} target="_blank" rel="noopener noreferrer" title={`${ev.kind || ''} ${ev.title || ''}`}>
                                {chip}
                              </a>
                            ) : (
                              <span key={eid}>{chip}</span>
                            );
                          })}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-[15px] leading-7 text-[var(--text-secondary)] whitespace-pre-line">
                  {item.aiSummary}
                </p>
              )}

              {enrich?.whyImportant && (
                <p className="text-[13px] leading-6 text-[var(--text-secondary)] bg-[var(--bg-surface)] border border-border rounded-lg px-4 py-3">
                  💡 {enrich.whyImportant}
                </p>
              )}

              {enrich && enrich.risks.length > 0 && (
                <div className="text-[13px] leading-6 text-[var(--warning)]">
                  <p className="font-semibold mb-1">风险与限制</p>
                  <ul className="space-y-1">
                    {enrich.risks.map((r, idx) => <li key={idx}>⚠ {r}</li>)}
                  </ul>
                </div>
              )}

              {conf && (
                <p className={`flex items-center gap-1.5 text-[12px] ${conf.cls}`}>
                  <conf.Icon size={12} /> 解读可信度：{conf.label}（基于证据完整度评定）
                </p>
              )}
            </div>
          )}

          {!item.aiSummary && (
            <p className="text-[13px] leading-6 text-[var(--text-tertiary)]">
              {aiBlocked
                ? `⛔ ${aiBlockReason}，暂不生成 AI 解读。`
                : item.description || '暂无描述。'}
              {!aiBlocked && ' 速览由计划任务自动生成，也可在会话中用自然语言触发 AI 雷达 Skill。'}
            </p>
          )}
        </section>

        {/* 分栏线 */}
        <div className="my-10 flex items-center gap-3 text-[var(--text-disabled)]" aria-hidden>
          <div className="flex-1 border-t border-border" />
          <BookOpen size={13} />
          <div className="flex-1 border-t border-border" />
        </div>

        {/* ========== 下区：深度解读 ========== */}
        <section aria-label="深度解读">
          <div className="flex items-center justify-between mb-4">
            <h2 className="flex items-center gap-1.5 text-[12px] font-semibold uppercase tracking-widest text-[var(--text-primary)]">
              <BookOpen size={12} className="text-[var(--dot-paper)]" /> 深度解读
            </h2>
            <div className="flex items-center gap-2">
              {diveSaveError && <span className="text-[12px] text-[var(--danger)]">✗ {diveSaveError}</span>}
              {editingDive ? (
                <>
                  <button
                    onClick={saveDive}
                    disabled={savingDive}
                    className="flex items-center gap-1 text-[12px] px-2.5 py-1 rounded-md bg-[var(--accent)] text-white hover:bg-[var(--accent-strong)] disabled:opacity-50"
                  >
                    {savingDive ? <Loader2 size={11} className="animate-spin" /> : <Check size={11} />} 保存
                  </button>
                  <button
                    onClick={() => { setEditingDive(false); setDiveSaveError(''); }}
                    className="flex items-center gap-1 text-[12px] px-2.5 py-1 rounded-md border border-border text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]"
                  >
                    <X size={11} /> 取消
                  </button>
                </>
              ) : (
                <>
                  {savedArticle && (
                    <button
                      onClick={() => setEditingDive(true)}
                      className="flex items-center gap-1 text-[12px] text-[var(--accent)] hover:underline"
                    >
                      <Pencil size={11} /> 编辑
                    </button>
                  )}
                  {savedArticle && (
                    <button
                      onClick={() => { setActivePage('articles'); close(); }}
                      className="text-[12px] text-[var(--text-tertiary)] hover:text-[var(--accent)] hover:underline"
                    >
                      在解读页打开 →
                    </button>
                  )}
                </>
              )}
            </div>
          </div>

          {editingDive && savedArticle ? (
            /* NB-27：配合 MarkdownEditor 高度契约改为列 flex（flex-1 取高）——
               外壳保留 h-[70vh] 显式高度，编辑器经 flex-1 填满，内部滚动确定 */
            <div className="flex h-[70vh] min-h-[480px] flex-col rounded-xl border border-border bg-[var(--bg-surface)] p-3">
              <MarkdownEditor
                key={savedArticle.id}
                ref={diveEditorRef}
                docKey={savedArticle.id}
                defaultValue={savedArticle.content}
              />
            </div>
          ) : diveLoading ? (
            <p className="flex items-center gap-2 text-[13px] leading-6 text-[var(--text-tertiary)]">
              <Loader2 size={13} className="animate-spin" /> 正在读取深度解读…
            </p>
          ) : diveContent ? (
            <div className="text-[14px] leading-7 text-[var(--text-secondary)]">
              <MarkdownView content={diveContent} />
            </div>
          ) : (
            <p className="text-[13px] leading-6 text-[var(--text-tertiary)]">
              {diveLoadError
                ? `深度解读读取失败：${diveLoadError}`
                : aiBlocked
                ? `⛔ ${aiBlockReason}，暂不生成深度解读。`
                : '深度解读由每日计划任务自动生成；也可在对话中说「给这条内容写深度解读」，由 AI 雷达 Skill 触发。'}
            </p>
          )}
        </section>

        {/* 原始信息与证据来源（折叠，P0-5） */}
        <details className="group mt-12 border-t border-border pt-4">
          <summary className="flex items-center gap-1 text-[12px] text-[var(--text-tertiary)] cursor-pointer select-none hover:text-[var(--text-secondary)] transition-colors">
            <ChevronDown size={12} className="transition-transform group-open:rotate-180" />
            原始信息与证据来源
          </summary>
          <div className="mt-3 p-3.5 rounded-lg bg-[var(--bg-surface)] border border-border text-[12px] space-y-1.5">
            {content.loading ? (
              <p className="flex items-center gap-1.5 text-[var(--text-tertiary)]">
                <Loader2 size={11} className="animate-spin" /> 正在获取正文内容（README / Model Card / 文章）…
              </p>
            ) : content.data ? (
              <>
                <div className="flex items-center gap-x-3 flex-wrap text-[var(--text-tertiary)]">
                  <span className={content.data.status === 'ready' ? 'text-[var(--success)]' : content.data.status === 'partial' ? 'text-[var(--warning)]' : content.data.status === 'failed' ? 'text-[var(--danger)]' : 'text-[var(--text-tertiary)]'}>
                    {content.data.status === 'ready' ? '✓ 正文已获取' : content.data.status === 'partial' ? '◐ 部分获取' : content.data.status === 'failed' ? '✗ 获取失败' : '⛔ 来源不支持'}
                  </span>
                  {content.data.contentType && content.data.contentType !== 'none' && <span>类型：{content.data.contentType}</span>}
                  {content.data.contentText && <span>长度：{content.data.contentText.length.toLocaleString()} 字符</span>}
                  {qualityLabel && <span>质量：{qualityLabel}</span>}
                  {content.data.fetchedAt && (
                    <span>获取于 {new Date(content.data.fetchedAt).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>
                  )}
                </div>
                {content.data.errorMessage && <p className="text-[var(--warning)]">⚠ {content.data.errorMessage}</p>}
                {evidenceItems.length > 0 && (
                  <p className="text-[var(--text-tertiary)]">
                    证据来源：
                    {evidenceItems.map((e, idx) => (
                      <span key={e.id}>
                        {idx > 0 && ' / '}
                        {e.url ? (
                          <a href={e.url} target="_blank" rel="noopener noreferrer" className="text-[var(--accent)] hover:underline">
                            {e.id} {e.title || e.kind}
                          </a>
                        ) : (
                          `${e.id} ${e.title || e.kind}`
                        )}
                      </span>
                    ))}
                  </p>
                )}
              </>
            ) : (
              <p className="text-[var(--text-tertiary)]">{content.error || '暂无正文记录'}</p>
            )}
          </div>
        </details>

        {/* 文末操作 */}
        <div className="mt-10 flex items-center justify-between">
          <button
            onClick={() => starItem(item.id)}
            className={`flex items-center gap-1.5 text-xs px-3.5 py-2 rounded-lg border transition-colors ${
              isStarred
                ? 'border-[var(--gold-border)] bg-[var(--warning-subtle)] text-[var(--gold)]'
                : 'border-border text-[var(--text-secondary)] hover:bg-[var(--bg-surface)]'
            }`}
          >
            <Star size={13} fill={isStarred ? 'currentColor' : 'none'} />
            {isStarred ? '已收藏' : '收藏'}
          </button>
          {item.url && (
            <a
              href={item.url}
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1.5 text-xs px-3.5 py-2 rounded-lg bg-[var(--accent)] text-white hover:bg-[var(--accent-strong)] transition-colors"
            >
              <ExternalLink size={13} />
              查看原文
            </a>
          )}
        </div>
      </div>
    </div>
  );
}

export default memo(ItemDetail);
