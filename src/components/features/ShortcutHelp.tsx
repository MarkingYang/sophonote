import { useState } from 'react';
import { Keyboard, X } from 'lucide-react';

/**
 * NB-04：编辑器快捷键参考卡。
 * Milkdown preset 自带成体系的键位（⌘⌥1-6 标题 / ⌘⌥7-8 列表 / ⌘⌥C 代码块等）但无处可查，
 * 工具栏仅覆盖字符级格式——此面板把全集分组可见化，状态栏一键唤起。
 * 键位清单与 preset-commonmark/preset-gfm 内置 keymap 及 NB-04 新增 ⌘⌥9 保持一致，
 * 新增/调整键位时同步更新本表。
 */
const SHORTCUT_GROUPS: { title: string; items: { keys: string; label: string }[] }[] = [
  {
    title: '通用',
    items: [
      { keys: '⌘S', label: '保存' },
      { keys: '⌘E', label: '编辑 / 预览切换' },
      { keys: '⌘K', label: '快速切换文档' },
      { keys: '⌘Z / ⇧⌘Z', label: '撤销 / 重做' },
    ],
  },
  {
    title: '文字格式',
    items: [
      { keys: '⌘B', label: '加粗' },
      { keys: '⌘I', label: '斜体' },
      { keys: '⌘⌥X', label: '删除线' },
      { keys: '⌘⇧B', label: '引用' },
    ],
  },
  {
    title: '块与列表',
    items: [
      { keys: '⌘⌥1 ~ 6', label: '标题 1–6 级' },
      { keys: '⌘⌥0', label: '正文段落' },
      { keys: '⌘⌥7 / ⌘⌥8', label: '有序 / 无序列表' },
      { keys: '⌘⌥9', label: '任务清单' },
      { keys: '⌘⌥C', label: '代码块' },
      { keys: 'Tab / ⇧Tab', label: '列表缩进 / 外移' },
      { keys: '/', label: '斜杠菜单（插入任意块）' },
    ],
  },
];

/** 状态栏唤起的悬浮参考卡：点击外部或 ✕ 关闭 */
export default function ShortcutHelp() {
  const [open, setOpen] = useState(false);
  return (
    <span className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex items-center gap-1 text-[var(--text-tertiary)] hover:text-[var(--accent)] transition-colors"
        title="编辑器快捷键一览"
      >
        <Keyboard size={11} /> 快捷键
      </button>
      {open && (
        <>
          {/* 透明遮罩：点击面板外任意处关闭 */}
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute bottom-full left-0 mb-2 z-50 w-64 rounded-lg border border-border bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] p-3">
            <div className="flex items-center justify-between mb-2">
              <p className="text-[12px] font-semibold text-[var(--text-primary)] uppercase tracking-wider">
                键盘快捷键
              </p>
              <button
                onClick={() => setOpen(false)}
                className="p-0.5 rounded text-[var(--text-tertiary)] hover:text-[var(--text-primary)] transition-colors"
              >
                <X size={12} />
              </button>
            </div>
            <div className="space-y-2.5">
              {SHORTCUT_GROUPS.map((g) => (
                <div key={g.title}>
                  <p className="text-[12px] font-semibold text-[var(--text-tertiary)] uppercase tracking-wider mb-1">{g.title}</p>
                  <div className="space-y-1">
                    {g.items.map((it) => (
                      <div key={it.keys} className="flex items-center justify-between gap-2">
                        <span className="text-[12px] text-[var(--text-secondary)]">{it.label}</span>
                        <kbd className="font-mono text-[13px] px-1.5 py-0.5 rounded-[6px] bg-[var(--bg-sunken)] text-[var(--text-tertiary)] whitespace-nowrap">
                          {it.keys}
                        </kbd>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </span>
  );
}
