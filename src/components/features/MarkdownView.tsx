import { memo, useEffect, useMemo, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeHighlight from 'rehype-highlight';
import type { Mermaid } from 'mermaid';
import { FileText, RefreshCw, CornerDownRight, Check, Copy } from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import { resolveNoteAsset } from '../../services/tauri';
import { scanTaskLines, toggleTaskLine, remapSectionToggle } from '../../services/noteTasks';
import { parseWikilink, wikilinkDisplay, extractSection, extractBlock } from '../../services/noteLinks';
import { copyMarkdownSource } from '../../services/clipboard';
import { isMermaidFence, shouldRenderMermaid } from '../../services/mermaidSource';
// NB-09：与 WikiHoverCard 互为导入（卡内再渲染 MarkdownView）——双方均只在渲染期引用，
// 且都是函数声明导出（可提升），ESM 循环安全
import { HoverWikiLink } from './WikiHoverCard';
// katex / highlight.js 样式已提升到 main.tsx 入口静态引入（性能债治理：
// 留在懒加载组件会迫使 Vite 为异步 CSS 注入 preload helper，拉重入口依赖）。

/* ==================== Mermaid 懒加载与渲染 ==================== */

let mermaidInstance: Mermaid | null = null;
let mermaidLoading: Promise<Mermaid> | null = null;
let mermaidTheme: string | null = null;

/**
 * 懒加载 mermaid（按需 code-split，不拖慢首屏），并保证主题跟随明暗切换。
 */
function getMermaid(): Promise<Mermaid> {
  const dark = document.documentElement.classList.contains('dark');
  const theme = dark ? 'dark' : 'default';
  if (mermaidInstance && mermaidTheme === theme) {
    return Promise.resolve(mermaidInstance);
  }
  if (!mermaidLoading) {
    mermaidLoading = import('mermaid').then((m) => {
      mermaidInstance = m.default;
      return mermaidInstance;
    });
  }
  return mermaidLoading.then((mm) => {
    mm.initialize(mermaidVisualConfig(dark));
    mermaidTheme = theme;
    return mm;
  });
}

let mmdSeq = 0;
// 同一份源码的渲染失败警告去重（防列表/预览反复重渲染时刷屏）；上限后清空重来
const warnedMmd = new Set<string>();

/** Mermaid 11.16 neo 视觉基线：Chat 与笔记共用，深浅色只切换同一套品牌色阶。
 *  渲染库例外：themeVariables 由 mermaid 内部消费生成 SVG（fill/stroke 表现属性 +
 *  内部色彩运算），无法解析 CSS var()，故此处保留 hex 调色板，不走设计令牌。 */
export function mermaidVisualConfig(dark: boolean) {
  const colors = dark
    ? {
        background: '#111827', surface: '#172033', surfaceMuted: '#1e293b',
        border: '#5273b9', text: '#eef4ff', mutedText: '#c5d1e6',
        line: '#8da4cf', accentSoft: '#20345d',
      }
    : {
        background: '#ffffff', surface: '#f4f7fc', surfaceMuted: '#e9eff9',
        border: '#7693c8', text: '#172033', mutedText: '#42526d',
        line: '#63789e', accentSoft: '#dfeaff',
      };

  return {
    startOnLoad: false,
    // 解析失败时禁用 mermaid 自带的「错误 SVG」渲染：默认行为会把临时 div（#d<id>）
    // 留在 document.body，撑高文档超出视口，macOS WKWebView 下整窗可滚动——
    // 首行/导航被滚出视野、首行拖拽区点不到，错误 SVG 还会在窗口底部裸奔。
    // 关闭后失败直接 throw，由组件内联错误 UI（mermaid-error）接管展示。
    suppressErrorRendering: true,
    theme: 'base' as const,
    look: 'neo' as const,
    darkMode: dark,
    securityLevel: 'loose' as const,
    fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", sans-serif',
    deterministicIds: true,
    maxTextSize: 80_000,
    maxEdges: 600,
    themeVariables: {
      background: colors.background,
      mainBkg: colors.surface,
      primaryColor: colors.surface,
      primaryTextColor: colors.text,
      primaryBorderColor: colors.border,
      secondaryColor: colors.accentSoft,
      secondaryTextColor: colors.text,
      secondaryBorderColor: colors.border,
      tertiaryColor: colors.surfaceMuted,
      tertiaryTextColor: colors.text,
      tertiaryBorderColor: colors.border,
      lineColor: colors.line,
      textColor: colors.text,
      nodeTextColor: colors.text,
      edgeLabelBackground: colors.background,
      clusterBkg: colors.surfaceMuted,
      clusterBorder: colors.border,
      titleColor: colors.text,
      actorBkg: colors.surface,
      actorBorder: colors.border,
      actorTextColor: colors.text,
      actorLineColor: colors.line,
      signalColor: colors.line,
      signalTextColor: colors.text,
      labelBoxBkgColor: colors.background,
      labelBoxBorderColor: colors.border,
      labelTextColor: colors.text,
      loopTextColor: colors.mutedText,
      noteBkgColor: colors.accentSoft,
      noteBorderColor: colors.border,
      noteTextColor: colors.text,
      fontSize: '14px',
    },
    themeCSS: `
      .nodeLabel, .label, .actor, .messageText { font-weight: 520; }
      .edgeLabel, .loopText, .noteText { color: ${colors.mutedText}; }
      .edgeLabel rect, .labelBkg { fill: ${colors.background}; opacity: 0.96; }
      .flowchart-link, .messageLine0, .messageLine1 { stroke-width: 1.5px; }
      .cluster-label { font-weight: 650; letter-spacing: 0.01em; }
      .node rect, .node polygon, .node circle, .node path { stroke-width: 1.25px; }
    `,
    flowchart: {
      useMaxWidth: true,
      curve: 'basis' as const,
      nodeSpacing: 48,
      rankSpacing: 58,
      padding: 18,
      diagramPadding: 12,
      wrappingWidth: 220,
      inheritDir: true,
    },
    sequence: {
      useMaxWidth: true,
      diagramMarginX: 24,
      diagramMarginY: 18,
      actorMargin: 64,
      width: 168,
      height: 48,
      boxMargin: 12,
      boxTextMargin: 8,
      noteMargin: 14,
      messageMargin: 42,
      mirrorActors: false,
      actorFontWeight: 600,
      messageFontWeight: 500,
    },
  };
}

/**
 * Mermaid flowchart/graph 自动修复（仅在原样渲染失败后尝试，不影响合法图）：
 * AI 生成的流程图常在节点文字里带括号/中括号（如 `B[LangSmith(追踪平台)]`），
 * mermaid 会直接 Parse error。修复 = 给节点文字加双引号（`B["LangSmith(追踪平台)"]`），
 * 并把文字内的 `"` 转义为 &quot;、`#` 转义为 #35;（mermaid 实体码）。
 * 由内向外迭代替换，兼容 stadium/diamond/双圆/子流程/圆柱/六边形等嵌套形状。
 * 返回 null 表示不是 flowchart/graph（不修复）。
 */
function repairFlowchart(code: string): string | null {
  const firstLine = code.split('\n').find((l) => l.trim().length > 0) ?? '';
  if (!/^(flowchart|graph)\b/.test(firstLine.trim())) return null;
  // 形状定界符（长者优先匹配，避免把 (( 当成两个 (）。closer 与 opener 一一对应。
  const OPENERS = ['((', '([', '[(', '[[', '{{', '[', '(', '{'];
  const CLOSER: Record<string, string> = {
    '((': '))', '([': '])', '[(': ')]', '[[': ']]', '{{': '}}', '[': ']', '(': ')', '{': '}',
  };
  const escapeText = (s: string) => s.trim().replace(/"/g, '&quot;').replace(/#/g, '#35;');

  /** 单行扫描：把每个形状定界符内的文字整体加引号（由外向内，引号区透传），内部嵌套定界符降级为字面文字 */
  const quoteShapesInLine = (line: string): string => {
    let out = '';
    let i = 0;
    const n = line.length;
    while (i < n) {
      const ch = line[i];
      // 已加引号的区域原样透传，避免二次包裹
      if (ch === '"') {
        out += ch; i += 1;
        while (i < n) { out += line[i]; if (line[i] === '"') { i += 1; break; } i += 1; }
        continue;
      }
      let opener = '';
      for (const op of OPENERS) if (line.startsWith(op, i)) { opener = op; break; }
      if (!opener) { out += ch; i += 1; continue; }
      const closer = CLOSER[opener];
      // 深度匹配同一定界符对，定位配对的收口（引号区内的定界符忽略）
      let depth = 0;
      let j = i;
      let found = -1;
      while (j < n) {
        if (line[j] === '"') { j += 1; while (j < n && line[j] !== '"') j += 1; j += 1; continue; }
        if (line.startsWith(opener, j)) { depth += 1; j += opener.length; continue; }
        if (line.startsWith(closer, j)) {
          // )) ]] }} 等同字符收口：内部形状自身的右括号会连成一段 run（如 ((a(x))) 尾部 )))，
          // 取 run 末尾 closer.length 个字符作为本形状收口，其余归内部文字——避免截断内部内容
          if (closer.length > 1 && closer.split('').every((c) => c === closer[0])) {
            let k = j;
            while (k < n && line[k] === closer[0]) k += 1;
            found = k - closer.length;
            break;
          }
          depth -= 1;
          if (depth === 0) { found = j; break; }
          j += closer.length;
          continue;
        }
        j += 1;
      }
      if (found < 0) { out += ch; i += 1; continue; } // 无配对 → 原样保留
      const inner = line.slice(i + opener.length, found);
      const t = inner.trim();
      if (t.length >= 2 && t.startsWith('"') && t.endsWith('"')) {
        out += line.slice(i, found + closer.length); // 内容已整体带引号
      } else {
        out += opener + '"' + escapeText(inner) + '"' + closer;
      }
      i = found + closer.length;
    }
    return out;
  };

  return code
    .split('\n')
    .map((line) => {
      const lt = line.trim();
      // 指令行不动（style/classDef/linkStyle/click/direction/注释）；
      // subgraph 保留——其标题 `subgraph s1[标题(x)]` 同样需要修复
      if (/^(end\b|style\b|classDef\b|class\s|linkStyle\b|click\b|direction\b|%%)/.test(lt)) return line;
      return quoteShapesInLine(line);
    })
    .join('\n');
}

/**
 * 渲染 ```mermaid 代码块为 SVG 流程图/时序图/架构图。
 * 进视口后再解析：文档切换时避免首屏同步跑多张图卡住点击。
 * 先按原样渲染；失败且为 flowchart/graph 时尝试自动修复（节点文字加引号）重渲一次。
 * 仍失败则不再静默回退：显示错误原因 + 源码，便于修正。
 */
function MermaidBlock({ code }: { code: string }) {
  const hostRef = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [triedRepair, setTriedRepair] = useState(false);

  useEffect(() => {
    const el = hostRef.current;
    if (!el) return;
    if (typeof IntersectionObserver === 'undefined') {
      setVisible(true);
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setVisible(true);
          io.disconnect();
        }
      },
      { rootMargin: '240px 0px' }
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    setError(null);
    setTriedRepair(false);
    // BOM/CRLF/首尾空白会直接导致 mermaid 解析失败，先规范化
    const normalized = code.replace(/\uFEFF/g, '').replace(/\r\n?/g, '\n').trim();

    /** 渲染到 DOM；成功返回 true。调用方负责捕获解析异常 */
    const renderInto = async (src: string): Promise<boolean> => {
      const mm = await getMermaid();
      const id = `mmd-${Date.now()}-${mmdSeq++}`;
      let svg: string;
      try {
        ({ svg } = await mm.render(id, src));
      } catch (e) {
        // 防御清理：mermaid 失败路径若未删尽 body 临时节点（#d<id> / 沙箱 #i<id>），
        // 残留的错误 SVG 会撑高文档使整窗可滚动（首行被滚出视野的根因）
        document.getElementById(`d${id}`)?.remove();
        document.getElementById(`i${id}`)?.remove();
        throw e;
      }
      if (cancelled || !hostRef.current) return false;
      hostRef.current.innerHTML = svg;
      // SVG 自适应容器宽度
      const svgEl = hostRef.current.querySelector('svg');
      if (svgEl) {
        svgEl.style.maxWidth = '100%';
        svgEl.style.height = 'auto';
      }
      return true;
    };

    (async () => {
      let firstErr: unknown = null;
      try {
        if (await renderInto(normalized)) return;
      } catch (e) {
        firstErr = e;
      }
      // 原样失败 → 尝试自动修复（仅 flowchart/graph）
      const repaired = repairFlowchart(normalized);
      if (repaired && repaired !== normalized) {
        setTriedRepair(true);
        try {
          if (await renderInto(repaired)) return;
        } catch {
          // 修复后仍失败，落到错误展示（保留原始错误信息，更贴近用户源码）
        }
      }
      if (warnedMmd.size > 200) warnedMmd.clear();
      const shouldWarn = !warnedMmd.has(normalized);
      if (shouldWarn) warnedMmd.add(normalized);
      if (shouldWarn) console.warn('Mermaid render failed:', firstErr);
      if (!cancelled) {
        setError(firstErr instanceof Error ? firstErr.message : String(firstErr ?? '未知错误'));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [code, visible]);

  if (error) {
    return (
      <span className="mermaid-error">
        <span className="mermaid-error-title">⚠ Mermaid 图表渲染失败</span>
        <span className="mermaid-error-msg">
          {error}
          {triedRepair && '（已尝试自动修复节点文字特殊字符，仍未通过）'}
        </span>
        <pre className="mermaid-error-code">
          <code>{code}</code>
        </pre>
      </span>
    );
  }
  // 用 span(display:block) 而非 div：<pre> 内只允许 phrasing 内容
  return (
    <span
      ref={hostRef}
      className="mermaid-block"
      style={visible ? undefined : { minHeight: 72, display: 'block' }}
      aria-busy={!visible}
    />
  );
}

/* ==================== ![[标题]] 嵌入转引（Obsidian transclusion） ==================== */

/** 最大嵌入深度，防止 A→B→A 之外的长链嵌套把页面撑爆 */
const MAX_EMBED_DEPTH = 3;

interface EmbedBlockProps {
  title: string;
  /** NB-10：![[笔记#标题]] 的段落标题段——只嵌入该段落（到下一个同级标题为止）而非全文 */
  heading?: string;
  /** NB-29：![[笔记#^块id]] 的块 id——只嵌入该块（连续非空行段落，含 ^id 锚标签） */
  blockId?: string;
  /** 当前嵌套链路上已嵌入的标题（含自身则判定循环） */
  embedPath: string[];
  onOpenArticle?: (title: string, heading?: string, blockId?: string) => void;
  onOpenItem?: (itemId: string) => void;
}

/**
 * ![[标题]] / ![[标题#段落]] / ![[标题#^块id]]：把另一篇文档（或其一个段落/一个块）内联渲染为嵌入卡片（Obsidian transclusion）。
 * - 循环嵌入 / 超过深度上限 → 提示不展开
 * - 目标不存在 → 缺失提示 + 一键创建（由 onOpenArticle 承接）
 * - 段落/块不存在（标题改名/删除）→ 提示未找到
 * - 卡片标题可点击跳转到原文（段落/块嵌入定位到对应行）
 * - 段落/块内的任务清单勾选经 remapSectionToggle 映射回全文档行号写回（不覆盖段落外内容）
 */
function EmbedBlock({ title, heading, blockId, embedPath, onOpenArticle, onOpenItem }: EmbedBlockProps) {
  const articles = useAppStore((s) => s.articles);
  const updateArticleContent = useAppStore((s) => s.updateArticleContent);

  if (embedPath.includes(title)) {
    return (
      <span className="md-embed md-embed-note">
        <RefreshCw size={11} /> 循环嵌入，已停止展开：{title}
      </span>
    );
  }
  if (embedPath.length >= MAX_EMBED_DEPTH) {
    return (
      <span className="md-embed md-embed-note">
        <CornerDownRight size={11} /> 已达嵌入深度上限（{MAX_EMBED_DEPTH} 层）：{title}
      </span>
    );
  }

  const target = articles.find((a) => a.title === title);
  if (!target) {
    return (
      <span className="md-embed md-embed-missing">
        <span className="md-embed-note">未找到「{title}」</span>
        {onOpenArticle && (
          <button className="md-embed-create" onClick={() => onOpenArticle(title)}>
            创建该笔记
          </button>
        )}
      </span>
    );
  }

  // NB-10 段落级嵌入：截取目标标题段；段落缺失时明确提示（不静默展开全文）
  const section = heading ? extractSection(target.content, heading) : null;
  if (heading && !section) {
    return (
      <span className="md-embed md-embed-missing">
        <span className="md-embed-note">「{title}」中未找到「{heading}」段落</span>
        {onOpenArticle && (
          <button className="md-embed-create" onClick={() => onOpenArticle(title)}>
            打开笔记
          </button>
        )}
      </span>
    );
  }

  // NB-29 块级嵌入：截取目标块内容；块不存在时明确提示
  const block = blockId ? extractBlock(target.content, blockId) : null;
  if (blockId && !block) {
    return (
      <span className="md-embed md-embed-missing">
        <span className="md-embed-note">「{title}」中未找到块 ^{blockId}</span>
        {onOpenArticle && (
          <button className="md-embed-create" onClick={() => onOpenArticle(title)}>
            打开笔记
          </button>
        )}
      </span>
    );
  }

  // 嵌入体：块 > 段落 > 全文；任务勾选经 remapSectionToggle 映射回全文档行号
  const body = block ? block.content : section ? section.content : target.content;
  const tag = block ? '块嵌入' : section ? '段落嵌入' : '嵌入';
  const locHint = block ? ` › ^${blockId}` : heading ? ` › ${heading}` : '';

  return (
    <span className="md-embed">
      <span className="md-embed-header">
        <FileText size={11} />
        <button
          className="md-embed-title"
          onClick={() => onOpenArticle?.(title, heading, blockId)}
          title={`打开「${title}」${locHint}`}
        >
          {title}
          {heading && <span className="text-[var(--text-tertiary)]"> › {heading}</span>}
          {blockId && !heading && <span className="text-[var(--text-tertiary)]"> › ^{blockId}</span>}
        </button>
        <span className="md-embed-tag">{tag}</span>
      </span>
      <MarkdownView
        content={body}
        onOpenArticle={onOpenArticle}
        onOpenItem={onOpenItem}
        onContentChange={
          section || block
            ? (newBody) => {
                // NB-10 / NB-29：段落/块嵌入内的任务勾选经 remapSectionToggle 映射回全文档行号
                // remapSectionToggle 逐行对比新旧内容，仅翻转变化的一行，段落/块外不受影响
                const refBody = section ? section.content : block!.content;
                const refStart = section ? section.startLine : block!.startLine;
                const full = remapSectionToggle(target.content, refBody, newBody, refStart);
                // NB-31：嵌入勾选写回失败不得成为未处理 rejection（内存已翻转，日志可见）
                if (full != null)
                  void updateArticleContent(target.id, full).catch((e) =>
                    console.error('Failed to write embedded doc:', e)
                  );
              }
            : (md) => void updateArticleContent(target.id, md).catch((e) => console.error('Failed to write embedded doc:', e))
        }
        embedPath={[...embedPath, title]}
        embedded
      />
    </span>
  );
}

/* ==================== Markdown 渲染 ==================== */

/**
 * N0：`assets/<name>` 相对路径图片 → read_note_asset 解析为 data URL 渲染。
 * 模块级缓存（resolveNoteAsset 内置）保证分屏预览 400ms 重渲染不重复 invoke；
 * 文件缺失显示占位提示而非裂图
 */
function NoteAssetImage({ src, alt }: { src: string; alt?: string }) {
  const [url, setUrl] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);

  useEffect(() => {
    let alive = true;
    setMissing(false);
    void resolveNoteAsset(src).then((u) => {
      if (!alive) return;
      if (u) setUrl(u);
      else setMissing(true);
    });
    return () => {
      alive = false;
    };
  }, [src]);

  if (missing) {
    return <span className="md-embed md-embed-missing">图片缺失：{src}</span>;
  }
  if (!url) {
    return <span className="text-xs text-[var(--text-tertiary)]">图片加载中…</span>;
  }
  return <img src={url} alt={alt ?? ''} style={{ maxWidth: '100%' }} />;
}

interface MarkdownViewProps {
  content: string;
  className?: string;
  /** Obsidian 式 [[双链]]：提供后双链可点击跳转（不存在则可由此创建）；
   * NB-10：heading 为标题链接（[[笔记#标题]]）的目标标题段，回调方负责定位该标题行；
   * NB-29：blockId 为块引用（[[笔记#^块id]]）的目标块 id，回调方负责定位块起始行 */
  onOpenArticle?: (title: string, heading?: string, blockId?: string) => void;
  /** N1：来源反链 `sophonote:item/<id>` 点击回调（跳回原条目阅读视图） */
  onOpenItem?: (itemId: string) => void;
  /**
   * 提供后任务清单（- [ ] / - [x]）在预览中可点击勾选并写回源文档；
   * 不提供则复选框为只读（如纯展示场景）
   */
  onContentChange?: (newContent: string) => void;
  /**
   * NB-03：分屏态勾选通道（与 onContentChange 二选一，优先本通道）。
   * 传任务序号（文档顺序第 N 个，0-based）而非改写结果——分屏时渲染快照可能滞后编辑器 ≤400ms，
   * 由父组件以编辑器实时内容为基线重新定位行号并写回（防心跳覆盖，模式同 N2 快速捕获）
   */
  onToggleTask?: (taskOrdinal: number) => void;
  /** 内部使用：当前嵌入链路（用于循环/深度检测） */
  embedPath?: string[];
  /** 内部使用：作为嵌入内容渲染时用 span 包裹，避免 div 嵌入 <p> 的嵌套问题 */
  embedded?: boolean;
  /**
   * NB-09：开启 [[双链]] 悬停预览卡（对标 Obsidian Page Preview）。
   * 仅预览态主文档开启；嵌入卡片与预览卡内部保持关闭，杜绝无限套娃。
   */
  hoverPreview?: boolean;
  /** Chat 回复使用：表格、Mermaid、围栏代码块显示独立 Markdown 源码复制入口。 */
  copySpecialBlocks?: boolean;
  /**
   * 流式预览：跳过 KaTeX / highlight / 重型插件，加快边输出边渲染。
   * 定稿后关闭。
   */
  lite?: boolean;
}

/**
 * 把代码围栏外的 Obsidian 语法转成内部协议链接，围栏内保持原样：
 * - ![[标题]] / ![[标题#段落]] → ![显示名](sophonote:embed/<enc>[?h=<enc>])（img 渲染器拦截为嵌入卡片）
 * - [[标题]] / [[标题|别名]] / [[标题#段落]] → [显示名](sophonote:article/<enc>[?h=<enc>])（a 渲染器拦截为双链）
 * 显示名 = 别名优先，标题链接无别名时为「标题 › 段落」（同 Obsidian）；标题解析失败（空标题）原样保留。
 * 替换只做行内 token 变换，不增删换行——mdast position 行号与原文一致（大纲定位依赖此性质）
 */
function wikilinkify(md: string): string {
  const parts = md.split('```');
  for (let i = 0; i < parts.length; i += 2) {
    // 必须先替换 ![[...]]，否则 [[...]] 规则会吞掉其中的内层方括号
    parts[i] = parts[i]
      .replace(/!\[\[([^\]\n]+)\]\]/g, (m, raw: string) => {
        const p = parseWikilink(raw);
        if (!p.title) return m;
        const loc = p.heading
          ? `?h=${encodeURIComponent(p.heading)}`
          : p.blockId
            ? `?b=${encodeURIComponent(p.blockId)}`
            : '';
        return `![${wikilinkDisplay(p)}](sophonote:embed/${encodeURIComponent(p.title)}${loc})`;
      })
      .replace(/\[\[([^\]\n]+)\]\]/g, (m, raw: string) => {
        const p = parseWikilink(raw);
        if (!p.title) return m;
        const loc = p.heading
          ? `?h=${encodeURIComponent(p.heading)}`
          : p.blockId
            ? `?b=${encodeURIComponent(p.blockId)}`
            : '';
        return `[${wikilinkDisplay(p)}](sophonote:article/${encodeURIComponent(p.title)}${loc})`;
      });
  }
  return parts.join('```');
}

/** NB-10：解析内部协议链接尾部的 `?h=<enc>` 段落参数（标题链接 / 段落级嵌入共用） */
function splitLocParam(encoded: string): { main: string; heading?: string; blockId?: string } {
  const hIdx = encoded.indexOf('?h=');
  if (hIdx >= 0) return { main: encoded.slice(0, hIdx), heading: decodeURIComponent(encoded.slice(hIdx + 3)) };
  const bIdx = encoded.indexOf('?b=');
  if (bIdx >= 0) return { main: encoded.slice(0, bIdx), blockId: decodeURIComponent(encoded.slice(bIdx + 3)) };
  return { main: encoded };
}

/** 提取 code 元素的纯文本（children 可能是字符串或已被高亮的节点数组） */
function textOf(children: React.ReactNode): string {
  if (typeof children === 'string') return children;
  if (typeof children === 'number') return String(children);
  if (Array.isArray(children)) return children.map(textOf).join('');
  if (children && typeof children === 'object') {
    const el = children as React.ReactElement<{ children?: React.ReactNode }>;
    if (el.props && 'children' in el.props) return textOf(el.props.children);
  }
  return '';
}

/** 从 hast 节点取源码行范围；wikilinkify 不增删换行，因此可安全回切原 Markdown。 */
type NodeWithPosition = {
  position?: {
    start: { line: number };
    end?: { line: number };
  };
};

function markdownSourceAtNode(content: string, node?: NodeWithPosition): string | null {
  const start = node?.position?.start.line;
  const end = node?.position?.end?.line;
  if (start == null || end == null || start < 1 || end < start) return null;
  return content.split('\n').slice(start - 1, end).join('\n');
}

export function specialMarkdownBlockLabel(source: string): '复制 Mermaid' | '复制代码' {
  return isMermaidFence(source) ? '复制 Mermaid' : '复制代码';
}

function SpecialMarkdownBlock({
  source,
  label,
  children,
}: {
  source: string | null;
  label: string;
  children: React.ReactNode;
}) {
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const resetRef = useRef<number | null>(null);
  useEffect(() => () => {
    if (resetRef.current != null) window.clearTimeout(resetRef.current);
  }, []);

  if (!source) return <>{children}</>;
  const copy = async () => {
    const copied = await copyMarkdownSource(source);
    setCopyState(copied ? 'copied' : 'failed');
    if (resetRef.current != null) window.clearTimeout(resetRef.current);
    resetRef.current = window.setTimeout(() => setCopyState('idle'), 1600);
  };

  return (
    <div className="hb-markdown-copy-block">
      {children}
      <button
        type="button"
        className={`hb-markdown-copy-button ${copyState !== 'idle' ? 'hb-markdown-copy-button-active' : ''}`}
        onClick={() => void copy()}
        title={`${label}（保留原始 Markdown 语法）`}
        aria-label={label}
      >
        {copyState === 'copied' ? <Check size={12} /> : <Copy size={12} />}
        <span>{copyState === 'copied' ? '已复制' : copyState === 'failed' ? '复制失败' : label}</span>
      </button>
    </div>
  );
}

/**
 * URL 协议白名单：react-markdown 默认 urlTransform 只放行 http/https/mailto/tel，
 * 会把内部协议 sophonote:（双链/嵌入）清空导致点击失效——这里显式放行 sophonote:，
 * 同时拦截 javascript: 等危险协议（内容来自 AI 生成，需防 XSS）。
 * N0：另放行笔记资产相对路径 assets/（img 渲染器异步解析为 data URL）
 * 与 data:image/（迁移兜底：未能解码落盘的历史内联图）
 */
function safeUrlTransform(url: string): string | undefined {
  if (/^(https?:|mailto:|sophonote:)/i.test(url)) return url;
  if (url.startsWith('assets/') || url.startsWith('data:image/')) return url;
  return undefined;
}

/** 标题组件工厂：以 mdast 行号生成稳定锚点（hb-line-N），供大纲点击滚动定位 */
function withHeadingId(Tag: 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6') {
  return function Heading({ children, node }: { children?: React.ReactNode; node?: NodeWithPosition }) {
    const line = node?.position?.start.line;
    return <Tag id={line != null ? `hb-line-${line}` : undefined}>{children}</Tag>;
  };
}

/**
 * 统一 Markdown 渲染管线：
 * - GFM：表格 / 删除线 / 任务列表 / 脚注（remark-gfm）
 * - 数学公式：$行内$ 与 $$块级$$（remark-math + rehype-katex）
 * - 代码高亮：围栏代码块语法着色（rehype-highlight / highlight.js）
 * - ```mermaid 图表（懒加载渲染，失败可视化）
 * - [[双链]] 点击跳转；![[标题]] 嵌入转引（防循环 / 深度上限）
 * - 标题锚点（hb-line-N）供大纲面板滚动定位
 * 排版样式由 index.css 的 .md-body 提供。
 */
function MarkdownView({
  content,
  className = '',
  onOpenArticle,
  onOpenItem,
  onContentChange,
  onToggleTask,
  embedPath = [],
  embedded = false,
  hoverPreview = false,
  copySpecialBlocks = false,
  lite = false,
}: MarkdownViewProps) {
  const Wrapper = embedded ? 'span' : 'div';
  // 任务行号表 + 复选框序号计数器：hast 渲染按文档序调用 input 组件，
  // 计数器在每次 render 重置，序号与 taskLines 一一对应（StrictMode 双渲染同样成立）。
  // scanTaskLines 来自 noteTasks.ts，与 Tasks 页聚合共用同一规则（防两边漂移）。
  const taskLines = useMemo(() => scanTaskLines(content), [content]);
  const renderedContent = useMemo(() => wikilinkify(content), [content]);
  // 大多数聊天回复是普通 Markdown。没有公式/围栏代码时不挂 KaTeX 与 highlight
  // AST 插件，避免生成结果出现的那一帧为整段文本执行无效的语法树遍历。
  // lite（流式）：只挂 remarkGfm，保证标题/列表/表格边收边排版。
  const hasMath = !lite && content.includes('$');
  const hasFencedCode = !lite && /(^|\n)\s*(?:```|~~~)/.test(content);
  const remarkPlugins = useMemo(
    () => (hasMath ? [remarkGfm, remarkMath] : [remarkGfm]),
    [hasMath]
  );
  const rehypePlugins = useMemo(
    () => [
      ...(hasFencedCode ? [rehypeHighlight] : []),
      ...(hasMath ? [rehypeKatex] : []),
    ],
    [hasFencedCode, hasMath]
  );
  let checkboxSeq = 0;

  /** 翻转指定行（1-based，mdast position 行号）的任务复选框（规则与 Tasks 页共用 toggleTaskLine） */
  const toggleTaskAtLine = (line: number) => {
    const next = toggleTaskLine(content, line);
    if (next !== content) onContentChange?.(next);
  };
  return (
    <Wrapper className={`md-body ${className}`}>
      <ReactMarkdown
        urlTransform={safeUrlTransform}
        remarkPlugins={remarkPlugins}
        // 注意：数组项必须是插件函数本身（或 [插件, 选项]），不能写成已调用的 rehypeHighlight()——
        // 否则 unified 会把返回的 transformer 当作 attacher 在 freeze 期以空 tree 调用，整页崩溃
        rehypePlugins={rehypePlugins}
        components={{
          h1: withHeadingId('h1'),
          h2: withHeadingId('h2'),
          h3: withHeadingId('h3'),
          h4: withHeadingId('h4'),
          h5: withHeadingId('h5'),
          h6: withHeadingId('h6'),
          // N5：列表项同样落 hb-line-N 锚点——任务行不一定是标题，
          // Tasks 页行级回链靠它滚动定位到具体任务行
          li({ children, node, ...rest }) {
            const line = (node as NodeWithPosition | undefined)?.position?.start.line;
            return <li id={line != null ? `hb-line-${line}` : undefined} {...rest}>{children}</li>;
          },
          // NB-29：段落也落 hb-line-N 锚点——块引用 [[笔记#^块id]] 的块首行定位靠它
          // （与标题/列表项复用同一套 hb-line-N 锚点机制；scrollspy/大纲不受影响，仍只过滤 h1-h6）
          p({ children, node, ...rest }) {
            const line = (node as NodeWithPosition | undefined)?.position?.start.line;
            return <p id={line != null ? `hb-line-${line}` : undefined} {...rest}>{children}</p>;
          },
          table({ children, node, ...rest }) {
            const table = <table {...rest}>{children}</table>;
            if (!copySpecialBlocks) return table;
            return (
              <SpecialMarkdownBlock
                source={markdownSourceAtNode(content, node as NodeWithPosition | undefined)}
                label="复制表格"
              >
                {table}
              </SpecialMarkdownBlock>
            );
          },
          pre({ children, node, ...rest }) {
            const source = markdownSourceAtNode(content, node as NodeWithPosition | undefined);
            const mermaid = specialMarkdownBlockLabel(source ?? '') === '复制 Mermaid';
            const block = mermaid
              ? <div className="hb-mermaid-frame">{children}</div>
              : <pre {...rest}>{children}</pre>;
            if (!copySpecialBlocks) return block;
            return (
              <SpecialMarkdownBlock source={source} label={mermaid ? '复制 Mermaid' : '复制代码'}>
                {block}
              </SpecialMarkdownBlock>
            );
          },
          code({ className: cls, children }) {
            const match = /(?:^|\s)language-([^\s]+)/.exec(cls || '');
            const code = textOf(children).replace(/\n$/, '');
            if (!lite && shouldRenderMermaid(match?.[1], code)) {
              return <MermaidBlock code={code} />;
            }
            return <code className={cls}>{children}</code>;
          },
          input({ type, checked }) {
            if (type !== 'checkbox') {
              return <input type={type} defaultChecked={checked ?? false} disabled />;
            }
            const ordinal = checkboxSeq++;
            const line = taskLines[ordinal];
            // NB-03：分屏态走 onToggleTask（序号上行，父组件以编辑器实时内容定位），预览态走 onContentChange
            const interactive = line != null && (onToggleTask != null || onContentChange != null);
            return (
              <input
                type="checkbox"
                checked={checked ?? false}
                disabled={!interactive}
                onChange={() => {
                  if (line == null) return;
                  if (onToggleTask) onToggleTask(ordinal);
                  else if (onContentChange) toggleTaskAtLine(line);
                }}
                className="task-checkbox"
              />
            );
          },
          img({ src, alt }) {
            if (src?.startsWith('sophonote:embed/')) {
              // NB-10：?h= 段携带段落标题（![[笔记#标题]]），只嵌入该段落而非全文
              // NB-29：?b= 段携带块 id（![[笔记#^块id]]），只嵌入该块
              const { main, heading, blockId } = splitLocParam(src.slice('sophonote:embed/'.length));
              const title = decodeURIComponent(main);
              return (
                <EmbedBlock
                  title={title}
                  heading={heading}
                  blockId={blockId}
                  embedPath={embedPath}
                  onOpenArticle={onOpenArticle}
                  onOpenItem={onOpenItem}
                />
              );
            }
            if (src?.startsWith('assets/')) {
              return <NoteAssetImage src={src} alt={alt} />;
            }
            return <img src={src} alt={alt ?? ''} style={{ maxWidth: '100%' }} />;
          },
          a({ href, children }) {
            if (href?.startsWith('sophonote:article/')) {
              // NB-10：?h= 段携带标题链接目标（[[笔记#标题]]），点击跳转并定位该标题行
              // NB-29：?b= 段携带块 id（[[笔记#^块id]]），点击跳转并定位块起始行
              const { main, heading, blockId } = splitLocParam(href.slice('sophonote:article/'.length));
              const title = decodeURIComponent(main);
              const locHint = blockId ? ` › ^${blockId}` : heading ? ` › ${heading}` : '';
              // NB-09：预览态主文档的双链包一层 HoverWikiLink（悬停预览卡，替代原生 title 提示）；
              // 嵌入卡片与预览卡内部不开启，杜绝无限套娃
              if (hoverPreview && onOpenArticle) {
                return (
                  <HoverWikiLink title={title} onOpen={() => onOpenArticle(title, heading, blockId)}>
                    {children}
                  </HoverWikiLink>
                );
              }
              return (
                <a
                  href="#"
                  className="wiki-link"
                  title={onOpenArticle ? `打开「${title}」${locHint}` : title}
                  onClick={(e) => {
                    e.preventDefault();
                    onOpenArticle?.(title, heading, blockId);
                  }}
                >
                  {children}
                </a>
              );
            }
            if (href?.startsWith('sophonote:item/')) {
              const id = decodeURIComponent(href.slice('sophonote:item/'.length));
              return (
                <a
                  href="#"
                  className="wiki-link"
                  title="跳回原条目阅读视图"
                  onClick={(e) => {
                    e.preventDefault();
                    onOpenItem?.(id);
                  }}
                >
                  {children}
                </a>
              );
            }
            return (
              <a href={href} target="_blank" rel="noopener noreferrer">
                {children}
              </a>
            );
          },
        }}
      >
        {renderedContent}
      </ReactMarkdown>
    </Wrapper>
  );
}

// 保存状态、标题输入等工作台更新不应让同一篇长文重复跑 unified/highlight/KaTeX 管线。
export default memo(MarkdownView);
