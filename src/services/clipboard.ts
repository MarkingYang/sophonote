/**
 * 把原始 Markdown 写入纯文本剪贴板。WebView 没有 Clipboard 权限时回退到
 * execCommand，保证粘贴到 Markdown 编辑器得到源码而不是预览 DOM 文本。
 */
export async function copyMarkdownSource(markdown: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(markdown);
      return true;
    }
  } catch {
    // Tauri WebView 的剪贴板权限可能暂时不可用，继续使用同步回退。
  }

  try {
    const textarea = document.createElement('textarea');
    textarea.value = markdown;
    textarea.readOnly = true;
    textarea.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0;';
    document.body.append(textarea);
    textarea.select();
    const copied = document.execCommand('copy');
    textarea.remove();
    return copied;
  } catch {
    return false;
  }
}
