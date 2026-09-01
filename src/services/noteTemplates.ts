/**
 * NB-07 笔记模板体系（对标 Obsidian Templates 核心插件）。
 *
 * 约定（零新存储，纯笔记即模板）：
 * - 带 `#template` 标签的 manual 笔记就是模板，像普通笔记一样随时编辑；
 * - 变量：{{title}}=新笔记标题（创建时刻的值，同 Obsidian 插入语义）、
 *   {{date}}=YYYY-MM-DD、{{time}}=HH:MM；
 * - 实例化时剥离模板正文里的 `#template` 标签——否则新笔记会带上模板标签、
 *   自己也变成模板（Obsidian 靠模板文件夹隔离，本应用标签内联需显式剥离）。
 */

/** 判断笔记内容是否为模板（#标签 提取规则与 DocWorkspace collectTags 一致） */
export function isTemplateContent(md: string): boolean {
  return Array.from(md.matchAll(/#[\p{L}\p{N}_-]+/gu)).some((m) => m[0] === '#template');
}

/** 剥离正文中的 #template 标签本体（不误伤 #templateX 等更长的标签） */
export function stripTemplateTag(md: string): string {
  return md.replace(/#template(?![\p{L}\p{N}_-])/gu, '').replace(/[ \t]+\n/g, '\n');
}

/** 变量替换：{{title}} / {{date}} / {{time}}（split/join 免正则特殊字符问题） */
export function applyTemplate(
  tpl: string,
  vars: { title: string; date: string; time: string }
): string {
  return tpl
    .split('{{title}}')
    .join(vars.title)
    .split('{{date}}')
    .join(vars.date)
    .split('{{time}}')
    .join(vars.time);
}

/** 从模板笔记生成新笔记正文：先剥离 #template 标签，再替换变量 */
export function instantiateTemplate(
  tpl: string,
  vars: { title: string; date: string; time: string }
): string {
  return applyTemplate(stripTemplateTag(tpl), vars);
}
