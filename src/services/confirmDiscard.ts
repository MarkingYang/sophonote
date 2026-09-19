/** WebView-owned modal: Tauri's window.confirm shim is not synchronous. */
export function confirmDiscard(message = '记忆有未保存修改，放弃修改并继续？'): Promise<boolean> {
  return new Promise((resolve) => {
    const dialog = document.createElement('dialog');
    dialog.className = 'rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5 text-[var(--text-primary)] shadow-xl backdrop:bg-black/40';
    dialog.style.maxWidth = 'min(420px, calc(100vw - 40px))';
    dialog.setAttribute('aria-label', '未保存的记忆修改');
    const text = document.createElement('p');
    text.textContent = message;
    const actions = document.createElement('div');
    actions.className = 'mt-5 flex justify-end gap-3';
    const finish = (discard: boolean) => { dialog.close(); dialog.remove(); resolve(discard); };
    for (const [label, discard] of [['继续编辑', false], ['放弃修改', true]] as const) {
      const button = document.createElement('button');
      button.textContent = label;
      button.className = 'rounded-lg border border-[var(--border-default)] px-3 py-2 text-sm';
      button.autofocus = !discard;
      button.onclick = () => finish(discard);
      actions.append(button);
    }
    dialog.append(text, actions);
    dialog.addEventListener('cancel', (event) => { event.preventDefault(); finish(false); });
    document.body.append(dialog);
    dialog.showModal();
  });
}
