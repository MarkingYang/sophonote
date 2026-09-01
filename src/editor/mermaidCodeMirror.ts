import {
  LanguageDescription,
  LanguageSupport,
  StreamLanguage,
  type StreamParser,
} from '@codemirror/language';
import { languages as codeMirrorLanguages } from '@codemirror/language-data';

type MermaidParserState = Record<string, never>;

/**
 * Mermaid does not ship an official CodeMirror 6 grammar. This lightweight mode
 * keeps editing responsive while giving declarations, comments, strings and
 * connectors useful visual distinction. Rendering/validation remains Mermaid's job.
 */
const mermaidParser: StreamParser<MermaidParserState> = {
  name: 'mermaid',
  startState: () => ({}),
  token(stream) {
    if (stream.match(/\s+/)) return null;
    if (stream.match(/%%.*$/)) return 'comment';
    if (stream.match(/"(?:[^"\\]|\\.)*"/)) return 'string';
    if (stream.match(/(?:<-->|-->|<--|---|-\.->|==>|~~>|--x|--o|&)/)) return 'operator';
    if (
      stream.match(
        /\b(?:flowchart|graph|sequenceDiagram|classDiagram(?:-v2)?|stateDiagram(?:-v2)?|erDiagram|journey|gantt|pie|quadrantChart|requirementDiagram|gitGraph|C4Context|C4Container|C4Component|C4Dynamic|C4Deployment|mindmap|timeline|subgraph|end|direction|participant|actor|activate|deactivate|loop|alt|else|opt|par|and|critical|break|rect|note|autonumber|title|section|style|classDef|class|linkStyle|click)\b/
      )
    ) return 'keyword';
    if (stream.match(/\b\d+(?:\.\d+)?\b/)) return 'number';
    stream.next();
    return null;
  },
};

const mermaidSupport = new LanguageSupport(StreamLanguage.define(mermaidParser));

/** Crepe's default language catalogue plus the missing Mermaid entry. */
export const editorCodeLanguages = [
  LanguageDescription.of({
    name: 'Mermaid',
    alias: ['mermaid', 'mmd', 'diagram'],
    extensions: ['mmd', 'mermaid'],
    support: mermaidSupport,
  }),
  ...codeMirrorLanguages,
];
