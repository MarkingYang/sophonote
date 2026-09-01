/**
 * N2 Journals：今日工作台辅助函数。
 * journal 笔记以 title = 'YYYY-MM-DD'、articleType = 'journal' 标识，
 * 与普通笔记同库同目录，仅入口与聚合 UI 特殊。
 */

/** 本地时区今日字符串（与 daily_picks 的 sv-SE 口径一致） */
export function todayStr(d: Date = new Date()): string {
  return d.toLocaleDateString('sv-SE');
}

export function isJournalTitle(title: string): boolean {
  return /^\d{4}-\d{2}-\d{2}$/.test(title);
}

/** 今日页模板：留白优先，痕迹聚合走 UI 面板不进正文（避免污染用户内容） */
export function buildJournalTemplate(dateStr: string): string {
  const d = new Date(`${dateStr}T00:00:00`);
  const weekdays = ['日', '一', '二', '三', '四', '五', '六'];
  const head = `${d.getFullYear()} 年 ${d.getMonth() + 1} 月 ${d.getDate()} 日 · 星期${weekdays[d.getDay()]}`;
  return [
    `> ${head} · 今日工作台：速记当下、沉淀所读、汇总所想。`,
    '',
    '## 速记',
    '',
    '## 今日思考',
    '',
  ].join('\n');
}

/** 快速捕获：向「## 速记」区块追加 `- HH:MM text`（时间顺序沉底）；无该区块则创建 */
export function appendCaptureLine(md: string, text: string): string {
  const time = new Date().toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit' });
  const line = `- ${time} ${text}`;
  const lines = md.split('\n');
  const start = lines.findIndex((l) => /^##\s*速记\s*$/.test(l.trim()));
  if (start === -1) {
    const base = md.trimEnd();
    return `${base}\n\n## 速记\n\n${line}\n`;
  }
  // 区块结束于下一个标题行或文末
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    if (/^#{1,6}\s/.test(lines[i])) {
      end = i;
      break;
    }
  }
  // 沉底插入：回退过区块尾部空行
  let insertAt = end;
  while (insertAt > start + 1 && lines[insertAt - 1].trim() === '') insertAt--;
  lines.splice(insertAt, 0, line);
  return lines.join('\n');
}
