import { highlightSegments } from '../../services/noteSearch';

/**
 * NB-08：搜索关键词高亮渲染（⌘K 切换器与笔记本列表搜索共用）。
 * 大小写不敏感命中，命中段以 mark 高亮；无命中时原样输出。
 */
export default function SearchHighlight({ text, query }: { text: string; query: string }) {
  return (
    <>
      {highlightSegments(text, query).map((s, i) =>
        s.hit ? (
          <mark
            key={i}
            className="bg-[var(--highlight)] text-inherit rounded-[2px] px-px"
          >
            {s.text}
          </mark>
        ) : (
          <span key={i}>{s.text}</span>
        )
      )}
    </>
  );
}
