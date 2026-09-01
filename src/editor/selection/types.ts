/**
 * AG-25：SelectionSnapshot 类型（docs/architecture.md 的契约）。
 *
 * 核心语义：ProseMirror 位置（proseFrom/proseTo）仅供当前 EditorView 高亮，
 * **不作为持久化地址**；跨会话/跨版本的持久化位置 = selectedMarkdown +
 * selectedTextHash + 前后文 hash + 唯一匹配（Rust 侧 TextAnchor 解析）。
 */
export interface SelectionSnapshot {
  selectionId: string;
  articleId: string;
  projectId?: string;
  baseVersion: number;
  /** 仅供当前 EditorView 高亮，不作为持久化地址 */
  proseFrom: number;
  proseTo: number;
  /** 选中 Slice 序列化出的 Markdown（Milkdown serializer；降级 = 选中文本） */
  selectedMarkdown: string;
  selectedTextHash: string;
  /** 临时结构路径（ProseMirror 祖先索引，重捕即失效，不参与持久化定位） */
  blockPath: number[];
  beforeContext: string;
  afterContext: string;
  beforeHash: string;
  afterHash: string;
  capturedAt: number;
}

/** 前后文窗口（字符数）：足够消歧、又不至于把半篇文档塞进锚点 */
export const SELECTION_CONTEXT_CHARS = 80;
