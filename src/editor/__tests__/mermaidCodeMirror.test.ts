import { describe, expect, it } from 'vitest';
import { editorCodeLanguages } from '../mermaidCodeMirror';

describe('Crepe Mermaid language catalogue', () => {
  it('exposes a searchable, immediately loadable Mermaid language', async () => {
    const mermaid = editorCodeLanguages.find((language) => language.alias.includes('mermaid'));

    expect(mermaid?.name).toBe('Mermaid');
    expect(mermaid?.alias).toEqual(expect.arrayContaining(['mermaid', 'mmd', 'diagram']));
    expect(await mermaid?.load()).toBeTruthy();
  });
});
