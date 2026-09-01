import { Sparkles } from 'lucide-react';

/**
 * AI 评分徽章（全站统一）：裸文字评分 → 胶囊徽章。
 * 分级（0-10 口径）：
 * - ≥8.5：gold 文字 + warning-subtle 底 + gold-border 描边，前缀 sparkle 图标
 * - 7.5–8.4：success 文字 + success-subtle 底
 * - <7.5：tertiary 文字 + sunken 底
 * 规格：12px/600、2px 8px、圆角 999px（.hb-score-badge，见 index.css）。
 */
interface ScoreBadgeProps {
  score: number;
  /** 放大一号（hero 条目用） */
  large?: boolean;
  /** 前缀文案，默认 "AI" */
  prefix?: string;
  title?: string;
}

export default function ScoreBadge({ score, large = false, prefix = 'AI', title }: ScoreBadgeProps) {
  const tier = score >= 8.5 ? ' hb-score-badge-high' : score >= 7.5 ? ' hb-score-badge-mid' : '';
  return (
    <span
      className={`hb-score-badge${tier}${large ? ' hb-score-badge-lg' : ''}`}
      title={title ?? 'AI 评分'}
    >
      {score >= 8.5 && <Sparkles size={large ? 13 : 11} aria-hidden />}
      {prefix} {Number(score).toFixed(1)}
    </span>
  );
}
