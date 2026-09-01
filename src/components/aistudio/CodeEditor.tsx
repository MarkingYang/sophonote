import { useEffect, useRef } from 'react';
import { Compartment, EditorState } from '@codemirror/state';
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { highlightSelectionMatches, search, searchKeymap } from '@codemirror/search';
import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from '@codemirror/autocomplete';
import {
  HighlightStyle,
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
} from '@codemirror/language';
import type { LanguageDescription } from '@codemirror/language';
import { languages } from '@codemirror/language-data';
import { tags } from '@lezer/highlight';

const MONO_FONT = 'SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace';

/** 按文件名匹配 language-data 语言：先按扩展名，无扩展名文件再按整名/别名（Dockerfile、Makefile）。 */
export function matchWorkspaceLanguage(fileName: string | null | undefined): LanguageDescription | null {
  if (!fileName) return null;
  const base = fileName.split('/').pop() ?? fileName;
  const dot = base.lastIndexOf('.');
  const ext = dot > 0 ? base.slice(dot + 1).toLowerCase() : '';
  if (ext) {
    const byExt = languages.find((desc) => desc.extensions.some((candidate) => candidate.toLowerCase() === ext));
    return byExt ?? null;
  }
  const lower = base.toLowerCase();
  return languages.find((desc) => desc.name.toLowerCase() === lower || desc.alias.some((alias) => alias.toLowerCase() === lower)) ?? null;
}

/** 状态栏语言标签：language-data 名称优先，其次扩展名大写。 */
export function codeLanguageLabel(path: string | null | undefined): string {
  if (!path) return 'Plain Text';
  const matched = matchWorkspaceLanguage(path);
  if (matched) return matched.name;
  const base = path.split('/').pop() ?? '';
  const dot = base.lastIndexOf('.');
  if (dot > 0) return base.slice(dot + 1).toUpperCase();
  return 'Plain Text';
}

const syntaxTheme = HighlightStyle.define([
  { tag: tags.comment, color: 'var(--code-comment)', fontStyle: 'italic' },
  { tag: [tags.keyword, tags.operatorKeyword, tags.modifier, tags.self], color: 'var(--code-keyword)' },
  { tag: [tags.string, tags.special(tags.string)], color: 'var(--code-string)' },
  { tag: [tags.number, tags.integer, tags.float, tags.bool, tags.null, tags.atom], color: 'var(--code-number)' },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName], color: 'var(--code-function)' },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.tagName], color: 'var(--code-type)' },
  { tag: [tags.propertyName, tags.variableName, tags.attributeName], color: 'var(--code-variable)' },
  { tag: [tags.meta, tags.annotation, tags.processingInstruction, tags.regexp, tags.escape], color: 'var(--code-meta)' },
  { tag: [tags.heading, tags.link], color: 'var(--code-keyword)' },
]);

const chromeTheme = EditorView.theme({
  '&': { backgroundColor: 'var(--bg-canvas)', color: 'var(--text-primary)', fontSize: '12px', height: '100%' },
  '.cm-scroller': { fontFamily: MONO_FONT, lineHeight: '20px', overflow: 'auto' },
  '.cm-content': { caretColor: 'var(--text-primary)', padding: '12px 0' },
  '.cm-gutters': {
    backgroundColor: 'var(--bg-canvas)',
    color: 'var(--text-disabled)',
    border: 'none',
    borderRight: '1px solid var(--border-default)',
    paddingLeft: '6px',
  },
  '.cm-lineNumbers .cm-gutterElement': { minWidth: '36px', padding: '0 8px 0 4px' },
  '.cm-activeLine': { backgroundColor: 'var(--code-active-line)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-tertiary)' },
  '.cm-cursor': { borderLeftColor: 'var(--text-primary)' },
  '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { backgroundColor: 'var(--accent-subtle) !important' },
  '.cm-selectionMatch': { backgroundColor: 'var(--accent-subtle)' },
  '.cm-matchingBracket': { backgroundColor: 'var(--accent-subtle)', outline: '1px solid var(--accent-border)' },
  '.cm-foldGutter .cm-gutterElement': { color: 'var(--text-disabled)', cursor: 'pointer', padding: '0 4px' },
  '.cm-foldPlaceholder': {
    backgroundColor: 'var(--bg-sunken)',
    border: '1px solid var(--border-default)',
    borderRadius: '4px',
    color: 'var(--text-tertiary)',
    margin: '0 2px',
  },
  '.cm-panels': {
    backgroundColor: 'var(--bg-surface)',
    borderBottom: '1px solid var(--border-default)',
    color: 'var(--text-secondary)',
    fontFamily: MONO_FONT,
    fontSize: '12px',
  },
  '.cm-panel.cm-search': { padding: '6px 8px' },
  '.cm-panel.cm-search input[type=text]': {
    backgroundColor: 'var(--bg-canvas)',
    border: '1px solid var(--border-default)',
    borderRadius: '4px',
    color: 'var(--text-primary)',
    outline: 'none',
    padding: '2px 6px',
  },
  '.cm-panel.cm-search input[type=text]:focus': { borderColor: 'var(--accent)' },
  '.cm-panel.cm-search button': {
    backgroundColor: 'var(--bg-sunken)',
    border: '1px solid var(--border-default)',
    borderRadius: '4px',
    color: 'var(--text-secondary)',
    cursor: 'pointer',
    padding: '2px 8px',
  },
  '.cm-panel.cm-search label': { color: 'var(--text-tertiary)' },
  '.cm-panel.cm-search [name=close]': { color: 'var(--text-tertiary)', fontSize: '14px' },
  '.cm-tooltip': {
    backgroundColor: 'var(--bg-surface)',
    border: '1px solid var(--border-default)',
    borderRadius: '6px',
    boxShadow: 'var(--shadow-md)',
  },
  '.cm-tooltip.cm-tooltip-autocomplete > ul': { fontFamily: MONO_FONT, fontSize: '12px', maxHeight: '12em' },
  '.cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]': {
    backgroundColor: 'var(--accent-subtle)',
    color: 'var(--text-primary)',
  },
  '&.cm-focused': { outline: 'none' },
  '&.cm-read-only .cm-content': { cursor: 'default' },
});

export interface CodeEditorProps {
  value: string;
  path: string;
  readOnly?: boolean;
  onChange?: (value: string) => void;
  onSave?: () => void;
  onCursorChange?: (cursor: { line: number; column: number }) => void;
}

export default function CodeEditor({ value, path, readOnly = false, onChange, onSave, onCursorChange }: CodeEditorProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const languageCompartment = useRef(new Compartment());
  const editableCompartment = useRef(new Compartment());
  const readOnlyCompartment = useRef(new Compartment());
  const onChangeRef = useRef(onChange);
  const onSaveRef = useRef(onSave);
  const onCursorRef = useRef(onCursorChange);

  useEffect(() => { onChangeRef.current = onChange; }, [onChange]);
  useEffect(() => { onSaveRef.current = onSave; }, [onSave]);
  useEffect(() => { onCursorRef.current = onCursorChange; }, [onCursorChange]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const reportCursor = (state: EditorState) => {
      const callback = onCursorRef.current;
      if (!callback) return;
      const head = state.selection.main.head;
      const line = state.doc.lineAt(head);
      callback({ line: line.number, column: head - line.from + 1 });
    };

    const state = EditorState.create({
      doc: value,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        drawSelection(),
        history(),
        indentUnit.of('  '),
        indentOnInput(),
        bracketMatching(),
        closeBrackets(),
        autocompletion(),
        foldGutter(),
        highlightSelectionMatches(),
        search({ top: true }),
        syntaxHighlighting(syntaxTheme),
        chromeTheme,
        languageCompartment.current.of([]),
        editableCompartment.current.of(EditorView.editable.of(!readOnly)),
        readOnlyCompartment.current.of(EditorState.readOnly.of(readOnly)),
        keymap.of([
          { key: 'Mod-s', preventDefault: true, run: () => { onSaveRef.current?.(); return true; } },
          ...closeBracketsKeymap,
          ...defaultKeymap,
          ...searchKeymap,
          ...historyKeymap,
          ...foldKeymap,
          ...completionKeymap,
          indentWithTab,
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) onChangeRef.current?.(update.state.doc.toString());
          if (update.docChanged || update.selectionSet) reportCursor(update.state);
        }),
      ],
    });
    const view = new EditorView({ state, parent: host });
    viewRef.current = view;

    let disposed = false;
    const description = matchWorkspaceLanguage(path);
    if (description) {
      description.load()
        .then((support) => {
          if (disposed || viewRef.current !== view) return;
          view.dispatch({ effects: languageCompartment.current.reconfigure(support) });
        })
        .catch(() => { /* 语言加载失败保持纯文本 */ });
    }

    return () => {
      disposed = true;
      viewRef.current = null;
      view.destroy();
    };
    // 组件由调用方以 key=path 挂载，path/value 只用于初始化；外部更新走下方同步 effect。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current === value) return;
    view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
  }, [value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: [
        editableCompartment.current.reconfigure(EditorView.editable.of(!readOnly)),
        readOnlyCompartment.current.reconfigure(EditorState.readOnly.of(readOnly)),
      ],
    });
    view.dom.classList.toggle('cm-read-only', readOnly);
  }, [readOnly]);

  return <div ref={hostRef} className="cm-code-editor h-full min-h-0 overflow-hidden" aria-label="代码编辑器" />;
}
