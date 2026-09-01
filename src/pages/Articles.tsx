import DocWorkspace from '../components/features/DocWorkspace';

/**
 * 深度解读：只承载 AI 产出（深度解读 / 夜间生成）。
 * 个人笔记已独立到「笔记本」页（articleType === 'manual' | 'journal'）。
 * 生成入口已收敛：一律由计划任务或对话自然语言触发 AI 雷达 Skill，界面不再有生成按钮。
 */
export default function Articles() {
  return (
    <DocWorkspace
      scope={(a) => a.articleType !== 'manual' && a.articleType !== 'journal'}
      listTitle="AI 解读"
      emptyHint="暂无解读。由每日计划任务自动生成，或在对话中说「给这条内容写深度解读」。"
    />
  );
}
