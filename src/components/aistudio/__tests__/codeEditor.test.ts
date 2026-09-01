import { describe, expect, it } from 'vitest';
import { codeLanguageLabel, matchWorkspaceLanguage } from '../CodeEditor';

describe('matchWorkspaceLanguage', () => {
  it('按扩展名匹配主力开发语言', () => {
    expect(matchWorkspaceLanguage('src/main.ts')?.name).toBe('TypeScript');
    expect(matchWorkspaceLanguage('App.tsx')?.name).toBe('TSX');
    expect(matchWorkspaceLanguage('crates/core/src/lib.rs')?.name).toBe('Rust');
    expect(matchWorkspaceLanguage('scripts/build.py')?.name).toBe('Python');
    expect(matchWorkspaceLanguage('package.json')?.name).toBe('JSON');
    expect(matchWorkspaceLanguage('config.toml')?.name).toBe('TOML');
    expect(matchWorkspaceLanguage('compose.yaml')?.name).toBe('YAML');
    expect(matchWorkspaceLanguage('run.sh')?.name).toBe('Shell');
  });

  it('扩展名大小写不敏感', () => {
    expect(matchWorkspaceLanguage('README.MD')?.name).toBe('Markdown');
    expect(matchWorkspaceLanguage('Main.TS')?.name).toBe('TypeScript');
  });

  it('无扩展名文件按整名/别名匹配（Dockerfile）', () => {
    expect(matchWorkspaceLanguage('Dockerfile')?.name).toBe('Dockerfile');
    expect(matchWorkspaceLanguage('docker/dockerfile')?.name).toBe('Dockerfile');
  });

  it('未知扩展名与空输入返回 null（回退纯文本）', () => {
    expect(matchWorkspaceLanguage('notes.unknownext')).toBeNull();
    expect(matchWorkspaceLanguage('README')).toBeNull();
    expect(matchWorkspaceLanguage('')).toBeNull();
    expect(matchWorkspaceLanguage(null)).toBeNull();
    expect(matchWorkspaceLanguage(undefined)).toBeNull();
  });

  it('匹配到的语言可异步加载为 LanguageSupport', async () => {
    const ts = matchWorkspaceLanguage('main.ts');
    expect(await ts?.load()).toBeTruthy();
    const rs = matchWorkspaceLanguage('lib.rs');
    expect(await rs?.load()).toBeTruthy();
  });
});

describe('codeLanguageLabel', () => {
  it('language-data 名称优先', () => {
    expect(codeLanguageLabel('src/main.ts')).toBe('TypeScript');
    expect(codeLanguageLabel('lib.rs')).toBe('Rust');
  });

  it('未收录扩展名回退为大写扩展名', () => {
    expect(codeLanguageLabel('data.unknownext')).toBe('UNKNOWNEXT');
  });

  it('无匹配且无扩展名回退 Plain Text', () => {
    expect(codeLanguageLabel('README')).toBe('Plain Text');
    expect(codeLanguageLabel(null)).toBe('Plain Text');
    expect(codeLanguageLabel(undefined)).toBe('Plain Text');
  });
});
