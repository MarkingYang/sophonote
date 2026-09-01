# SophoNote Markdown Style

Use this reference for formatting, restructuring, and template generation.

## Document hierarchy

- Use one H1 for a standalone document title.
- Use H2 for main sections, H3 for subsections, and avoid skipping levels.
- For a selected fragment whose parent heading is unknown, begin at H2 rather than inventing an H1.
- Keep one blank line around headings, lists, block quotes, tables, and fenced blocks.

## Paragraphs and lists

- Keep one idea per paragraph.
- Use `-` for unordered lists and `1.` for ordered steps.
- Use `- [ ]` and `- [x]` for tasks; never change checked state during formatting.
- Indent nested list content by two spaces unless existing syntax requires four.
- Do not turn prose into a list unless the items are genuinely parallel.

## Emphasis and links

- Use `**bold**` for strong emphasis and `*italic*` sparingly.
- Do not place whitespace immediately inside emphasis markers.
- Preserve URLs and link labels. Use `[label](url)` only when both are already present or the user asks to create a link.
- Preserve SophoNote/Obsidian wiki links `[[target|alias]]`, block references, tags, and embeds exactly.

## Code, math, and diagrams

- Preserve inline code and fenced-code contents exactly during format-only operations.
- Keep a blank line before and after fenced blocks.
- Preserve language identifiers on fences.
- Keep KaTeX delimiters and Mermaid bodies unchanged unless the user explicitly asks to fix them.

## Tables

- Use tables only for repeated fields that benefit from column comparison.
- Include a header separator row.
- Keep cell text concise; use lists instead when cells would contain long paragraphs.

## Quotes, callouts, and metadata

- Preserve YAML frontmatter keys and values unless the user explicitly asks to edit metadata.
- Use `>` for quotations and do not rewrite quoted text.
- Preserve footnote identifiers and definitions.
- Keep source links, citations, dates, owners, and status fields visible.

## Output quality

- Prefer clear hierarchy over decorative formatting.
- Do not add emojis unless the document already uses them or the user asks.
- Do not add generic sections such as “总结” or “下一步” without source content.
- For missing template data, use `[待填写]` or a more specific placeholder such as `[负责人]`.
