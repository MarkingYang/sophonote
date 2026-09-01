import { describe, expect, it } from 'vitest';
import { isMermaidFence, isMermaidSource, parseFencedCode, shouldRenderMermaid } from '../mermaidSource';

describe('Mermaid source classification', () => {
  it('recognizes supported diagram declarations at the first real statement', () => {
    expect(isMermaidSource('flowchart TB\nA --> B')).toBe(true);
    expect(isMermaidSource('%%{init: {"theme": "base"}}%%\nsequenceDiagram\nA->>B: hello')).toBe(true);
    expect(isMermaidSource('---\ntitle: Context\n---\nC4Context\nPerson(user, "User")')).toBe(true);
  });

  it('does not infer Mermaid from a later mention inside ordinary text', () => {
    expect(isMermaidSource('This document explains flowchart TB')).toBe(false);
    expect(isMermaidSource('ordinary text\nflowchart TB\nA --> B')).toBe(false);
  });

  it('renders explicit Mermaid and conservatively upgrades legacy text fences', () => {
    expect(shouldRenderMermaid('Mermaid', 'not parsed yet')).toBe(true);
    expect(shouldRenderMermaid('text', 'flowchart LR\nA --> B')).toBe(true);
    expect(shouldRenderMermaid('typescript', 'flowchart LR\nA --> B')).toBe(false);
    expect(isMermaidFence('~~~txt\nmindmap\n  root((SophoNote))\n~~~')).toBe(true);
  });

  it('parses both backtick and tilde fences without including the closer', () => {
    expect(parseFencedCode('```text\nflowchart TB\n```')).toEqual({
      language: 'text',
      body: 'flowchart TB',
    });
    expect(parseFencedCode('~~~~Mermaid\nsequenceDiagram\n~~~~')).toEqual({
      language: 'mermaid',
      body: 'sequenceDiagram',
    });
  });
});
