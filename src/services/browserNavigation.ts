export function normalizeBrowserUrl(value: string): string | null {
  const text = value.trim();
  if (!text) return null;
  const isLocalhost = /^(localhost|127\.0\.0\.1|0\.0\.0\.0)(:\d+)?(?:\/|$)/i.test(text);
  if (!isLocalhost && /^[a-z][a-z\d+.-]*:/i.test(text) && !/^https?:\/\//i.test(text)) return null;
  const candidate = /^https?:\/\//i.test(text)
    ? text
    : isLocalhost
      ? `http://${text}`
      : `https://${text}`;
  try {
    const url = new URL(candidate);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.toString() : null;
  } catch {
    return null;
  }
}

export function isPdfUrl(value: string): boolean {
  try {
    return new URL(value).pathname.toLocaleLowerCase().endsWith('.pdf');
  } catch {
    return false;
  }
}

export type BrowserFileKind = 'pdf' | 'image' | 'text' | 'unsupported';

const IMAGE_EXTENSIONS = new Set(['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg']);
const TEXT_EXTENSIONS = new Set(['txt', 'md', 'markdown', 'json', 'xml', 'csv']);

export function fileExtension(path: string): string {
  const name = fileNameFromPath(path);
  const separator = name.lastIndexOf('.');
  return separator > 0 ? name.slice(separator + 1).toLocaleLowerCase() : '';
}

export function browserFileKind(path: string): BrowserFileKind {
  const extension = fileExtension(path);
  if (extension === 'pdf') return 'pdf';
  if (IMAGE_EXTENSIONS.has(extension)) return 'image';
  if (TEXT_EXTENSIONS.has(extension)) return 'text';
  return 'unsupported';
}

export function isBrowserPreviewFile(path: string): boolean {
  return browserFileKind(path) !== 'unsupported';
}

export function shouldOpenProjectFileInBrowser(path: string): boolean {
  const kind = browserFileKind(path);
  return kind === 'pdf' || kind === 'image';
}

export function fileNameFromPath(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || '文件';
}
