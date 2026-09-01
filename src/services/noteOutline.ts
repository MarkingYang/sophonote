/**
 * NB-23：大纲提取共享模块（原 DocWorkspace 内联函数抽出，供 NoteWorkbench 等同源复用；
 * DocWorkspace 已改为导入本模块，两处语义永远一致）。
 */

/**
 * 提取标题大纲（忽略代码围栏内的 # 行）。
 * line 为 1-based 行号，与 MarkdownView 标题锚点 hb-line-N 的 mdast position 对齐
 * （wikilinkify 只做行内替换不增删换行，行号保持一致）。
 */
export function extractOutline(md: string): { level: number; text: string; line: number }[] {
  const out: { level: number; text: string; line: number }[] = [];
  let inFence = false;
  md.split('\n').forEach((ln, i) => {
    if (/^\s*```/.test(ln)) {
      inFence = !inFence;
      return;
    }
    if (inFence) return;
    const m = /^(#{1,6})\s+(.+)$/.exec(ln);
    if (m) {
      out.push({
        level: m[1].length,
        text: m[2].replace(/[*_`$~]|\[\[|\]\]/g, '').trim(),
        line: i + 1,
      });
    }
  });
  return out;
}
