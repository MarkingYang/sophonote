import { describe, expect, it } from 'vitest';
import {
  browserFileKind,
  fileExtension,
  fileNameFromPath,
  isBrowserPreviewFile,
  isPdfUrl,
  normalizeBrowserUrl,
  shouldOpenProjectFileInBrowser,
} from '../browserNavigation';

describe('browserNavigation', () => {
  it('normalizes public sites and localhost', () => {
    expect(normalizeBrowserUrl('example.com')).toBe('https://example.com/');
    expect(normalizeBrowserUrl('localhost:3000/app')).toBe('http://localhost:3000/app');
    expect(normalizeBrowserUrl('javascript:alert(1)')).toBeNull();
  });

  it('recognizes remote PDF URLs without relying on the query string', () => {
    expect(isPdfUrl('https://example.com/report.pdf?download=1')).toBe(true);
    expect(isPdfUrl('https://example.com/report')).toBe(false);
  });

  it('extracts PDF names from macOS and Windows paths', () => {
    expect(fileNameFromPath('/Users/demo/report.pdf')).toBe('report.pdf');
    expect(fileNameFromPath('C:\\Users\\demo\\report.pdf')).toBe('report.pdf');
  });

  it('classifies browser-readable local files without treating code as a binary preview', () => {
    expect(browserFileKind('/tmp/report.PDF')).toBe('pdf');
    expect(browserFileKind('/tmp/screenshot.webp')).toBe('image');
    expect(browserFileKind('/tmp/README.md')).toBe('text');
    expect(browserFileKind('/tmp/archive.zip')).toBe('unsupported');
    expect(isBrowserPreviewFile('/tmp/data.csv')).toBe(true);
    expect(isBrowserPreviewFile('/tmp/app.tsx')).toBe(false);
  });

  it('routes project documents and images to Browser while keeping text in the editor', () => {
    expect(shouldOpenProjectFileInBrowser('docs/spec.pdf')).toBe(true);
    expect(shouldOpenProjectFileInBrowser('assets/logo.svg')).toBe(true);
    expect(shouldOpenProjectFileInBrowser('README.md')).toBe(false);
    expect(fileExtension('C:\\demo\\DATA.JSON')).toBe('json');
  });
});
