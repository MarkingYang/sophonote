---
name: sophonote-markdown-writing
description: Edit SophoNote note titles and Markdown content, format or proofread writing, generate outlines and templates, continue or rewrite selected text, extract actions, and create nested project document structures. Use for natural-language writing requests or explicit format, proofread, structure, outline, rewrite, continue, template, and actions commands in SophoNote or Hermes CLI.
---

# SophoNote Markdown Writing

Use this skill as the writing layer for SophoNote notes and workspace documents. Treat the user's words as source material, the visible selection or attached document as scope, and Markdown as the delivery format.

## When to Use

Use this skill when the user asks to:

- format Markdown without changing wording;
- proofread typos, grammar, punctuation, or unclear sentences;
- organize rough notes into headings, lists, tables, quotes, or task lists;
- create a reusable Markdown template;
- build an outline, summary, continuation, or rewrite;
- transform selected text in a SophoNote note or workspace document.
- rename the visible current note;
- create project documents and organize them into parent/child subdirectories.

Do not use it for source verification, factual research, or non-writing automation unless the user also asks for a Markdown deliverable.

## Input and Scope

Resolve the target in this order:

1. A SophoNote selection attachment whose frontmatter contains `source: sophonote-selection` (its filename starts with `sophonote-selection` and may gain a numeric suffix in a long Session).
2. A visible current-document attachment whose frontmatter contains `source: sophonote-document` (its filename starts with `sophonote-document`).
3. An explicitly attached project whose frontmatter contains `source: sophonote-project`; it supplies only project metadata, a read-only document manifest, and a bounded action region—not member document bodies.
4. A document or folder explicitly attached by the user.
5. Text pasted into the current request.
6. A real path or document explicitly named by the user.

Never silently widen a selection to the whole document. If more than one target is plausible, ask one short clarification question.

`sophonote-selection` and `sophonote-document` attachments are SophoNote Session working copies. Read the YAML frontmatter only for identity and version metadata. Markdown content is mutable only between:

```text
<!-- SOPHONOTE_EDITABLE_START -->
...
<!-- SOPHONOTE_EDITABLE_END -->
```

Never delete, duplicate, move, or rewrite either marker or the YAML frontmatter. Never edit text outside those markers. A selection working copy contains only the selected range, so never widen it to the rest of the article.

A `sophonote-document` working copy also exposes the current note title between:

```text
<!-- SOPHONOTE_TITLE_START -->
...
<!-- SOPHONOTE_TITLE_END -->
```

Edit that title region only when the user explicitly asks to rename the current note. Keep it to one non-empty line of at most 200 characters. Never edit YAML `title:` directly. SophoNote Host validates the base title and returns an approval card; the title is not changed until the user accepts it.

For a mutating request (`format`, an explicitly applied proofread, `structure`, `rewrite`, `continue`, or `actions`), edit that bounded working copy **in place** with Hermes native file tools. Do not search for another copy, substitute a sample path, or call a legacy SophoNote MCP. Do not return a second full copy of the document in Chat. SophoNote Host will read the working-copy difference after the turn, validate its identity and base version, and show a reviewable patch directly in the left editor.

For a non-mutating request such as “先列问题” or “给建议，不修改”, leave the working copy unchanged and answer concisely in Chat.

When the user has already expressed a strong write intent such as “需要”, “保存”, “写入文章”, “补充到当前笔记”, or “直接改左侧”, act on the unique attached target immediately. Do not instruct them to create a placeholder note, attach it, and send another message. If destination or content form is genuinely ambiguous, ask one concise combined question instead of a tutorial.

## Note and Project Operations

Treat these as Host domain operations, not Markdown formatting:

- **Current note title:** edit only the bounded title region in the current `sophonote-document` working copy. Do not edit a title for a selection-only attachment.
- **Current note content:** edit only the bounded Markdown region. SophoNote returns a reviewable patch to the left editor.
- **New note and nested project structure:** when a `sophonote-project` or project-bound `sophonote-document` working copy contains the project-action markers below, write the requested Host actions into that bounded JSON array. Do not call a legacy MCP and do not create local filesystem folders.
- **Existing project structure:** use the project context already supplied by SophoNote. Reuse an existing article only when its `article_id` is known and unambiguous; otherwise ask before moving it.

Title changes require Host approval. Creating documents and setting parents are immediately visible but remain limited to the project bound to the current SophoNote Session. If the project-action markers are absent, do not simulate success by creating local files or using a `lease_id` MCP; ask the user to attach the target project from the left tree.

### Project Action Working Copy

A project-scoped working copy contains exactly one bounded action region. A standalone `sophonote-project` copy also has a read-only manifest between `SOPHONOTE_PROJECT_MANIFEST_START` and `SOPHONOTE_PROJECT_MANIFEST_END`; never edit that manifest.

```text
<!-- SOPHONOTE_PROJECT_ACTIONS_START -->
[]
<!-- SOPHONOTE_PROJECT_ACTIONS_END -->
```

For a requested new note or project hierarchy, replace only the JSON array between these markers. Keep the markers and `projectId` metadata unchanged. Supported actions are:

```json
[
  {
    "type": "create_document",
    "client_id": "research",
    "title": "研究",
    "content": "",
    "parent_client_id": null,
    "parent_article_id": null
  },
  {
    "type": "create_document",
    "client_id": "materials",
    "title": "资料",
    "content": "",
    "parent_client_id": "research"
  },
  {
    "type": "set_document_parent",
    "article_id": "existing-article-id",
    "parent_article_id": "existing-parent-id"
  }
]
```

Rules:

- `client_id` is a short, unique identifier local to this turn.
- Put every parent `create_document` action before its children.
- Use either `parent_client_id` for a node created earlier in this array or `parent_article_id` for a known existing node, never both.
- Omit `parent_article_id` or set it to `null` for project root.
- Never invent an existing `article_id`.
- Do not add more than 64 actions in one turn.
- SophoNote Host validates project scope, duplicates, parent membership and cycles, then refreshes the left project tree.

## Intent Routing

Infer the operation from ordinary writing language. Explicit commands are also supported:

| Intent | Natural requests | Command |
|---|---|---|
| Format only | “排版一下”“规范 Markdown”“整理格式，不改字” | `format` |
| Proofread | “校对”“查错别字”“看看哪里不通顺” | `proofread` |
| Restructure | “整理层级”“变成清单/表格”“拆成章节” | `structure` |
| Outline | “先列提纲”“把这些想法归类” | `outline` |
| Rewrite | “润色”“改得更简洁/正式” | `rewrite` |
| Continue | “续写”“补充下一节” | `continue` |
| Template | “生成模板”“给我一个会议/研究笔记样例” | `template` |
| Extract actions | “提取待办”“变成任务清单” | `actions` |

The command is the first token after selecting this skill; the remainder is the target or constraint. Examples: `format`, `proofread 先给建议`, `rewrite 更简洁`, `template 会议纪要`, `structure 并创建项目子目录`.

If the request mixes operations, apply them in this order: proofread findings → structure/format → rewrite or continue. State when wording will change before doing so.

## Procedure

1. Identify the target and requested operation.
2. Read `references/markdown-style.md` when formatting, restructuring, or creating a template.
3. Preserve the user's language, terminology, links, code, citations, and factual claims unless the chosen operation explicitly permits wording changes.
4. If the target is a bounded SophoNote working copy and the request is mutating, edit only its editable region in place. The working copy is not the real article; the Host owns review and write-back.
5. When the request includes the current note title, edit only the title region and leave the YAML identity metadata intact.
6. When the request creates a project tree, write the bounded project-action JSON in parent-to-child order. Return a short status; SophoNote Host returns the authoritative created tree and article IDs.
7. Re-read the edited working copy and confirm all boundary markers and the YAML frontmatter remain intact.
8. For `format`, verify textual-content conservation before ending the turn. If verification fails, restore the original editable region and report the failure.
9. End with a short status only: what was prepared, whether verification passed, and that the proposed changes are available in the left editor for review. Do not repeat the replacement Markdown in Chat.
10. For ordinary files outside SophoNote, keep the existing CLI behavior: preview by default and write only when the user's file-edit request is explicit.

## Operation Rules

### Format Only

- Keep all words, numbers, punctuation, links, code, citations, and their order unchanged.
- Only change Markdown markers, heading levels, list markers, table delimiters, indentation, spacing, and blank lines.
- Do not summarize, translate, correct typos, or silently finish incomplete sentences.
- Preserve fenced-code contents byte-for-byte.
- When original and formatted text are available as files, run `scripts/check_format_only.py ORIGINAL FORMATTED`. A failed check blocks write-back.
- For a SophoNote working copy, write the formatted result into the editable region and return only a one-line verification/status result. Do not echo the document.

### Proofread

- Classify findings as `错别字`, `语法/标点`, `表达不顺`, or `疑似事实/专名`.
- Do not “correct” names, code identifiers, quotations, URLs, or domain terms without confidence.
- Default to an issue table with location, original, suggestion, and reason.
- Only edit the working copy when the user says to apply or directly fix it. Keep edits minimal. Otherwise return the issue table and leave the working copy untouched.

### Structure and Outline

- Preserve meaning and paragraph wording unless the user also requests rewriting.
- Prefer one H1 for a standalone document; start selected fragments at H2 or below when their parent context is unknown.
- Avoid heading-level jumps. Use lists for parallel items and tables only for repeated fields.
- Keep code fences, math, Mermaid, wiki links, block IDs, footnotes, and task states intact.

### Rewrite and Continue

- These operations may change or add wording only when explicitly requested.
- Match the document's language, tone, terminology, heading depth, and sentence length.
- Separate assumptions from source facts. Use `[待确认]` for missing facts instead of inventing them.
- For a rewrite, summarize the major wording changes after the Markdown preview.

### Templates

- Select the closest asset in `assets/templates/` and adapt its structure.
- Preserve reusable placeholders such as `[待填写]`, dates, owners, sources, and decision status.
- Never fabricate names, dates, conclusions, evidence, or task owners.
- If no template type is given, offer at most three likely choices before generating a long template.

### Actions

- Convert only explicit commitments or clearly actionable statements into `- [ ]` tasks.
- Preserve owner, due date, source, and dependencies when present.
- Mark inferred metadata as `[待确认]`.

### Project Tree

- Interpret “目录/子目录” as SophoNote document hierarchy, not a local filesystem directory, unless the user explicitly attached a normal filesystem folder.
- Use short index documents as structural nodes only when the user wants them. Give each node a useful title and a minimal purpose/links body instead of inventing project facts.
- Create top-level nodes first, then children. SophoNote Host supplies stable run-scoped idempotency keys.
- Never move an existing document unless the user named it or approved the proposed hierarchy.
- Reject cycles, cross-project parents, and ambiguous duplicate titles; ask one focused question when a match is unclear.

## Quick Reference

### SophoNote

Use the left project/document row’s **加入会话** button, or select text in the editor and choose **Add to Chat**, then write a natural request such as:

- `只整理 Markdown 格式，不改变任何文字。`
- `校对这段内容，先列问题，不要直接改。`
- `把选中内容整理成二级标题和任务清单。`
- `按研究笔记模板重组，缺失信息保留待填写。`
- `把当前笔记标题改为“检索策略复盘”，正文只做 format。`
- `创建“研究”目录，下面建立“资料”“实验”“结论”三个子文档。`
- `把刚才生成的文章保存到这个项目。`

SophoNote passes the explicit project, current document, or selection as a native Hermes file attachment. A project attachment contains titles and hierarchy only; choose a specific document when its body must be read or edited.

To format the whole current article, leave the current-document chip visible, activate this skill, and send `format` or `只整理 Markdown 格式，不改变任何文字。` SophoNote attaches the latest editor draft automatically; do not type a synthetic `@file:` path. The result appears as reviewable changes in the left editor; accept or reject each change there.

### Hermes CLI

Preload the skill for one prompt:

```bash
ARTICLE_MD='/replace/with/a/real/absolute/path/article.md'
hermes --skills sophonote-markdown-writing -z "proofread @file:${ARTICLE_MD} and list issues first"
```

Start an interactive writing session:

```bash
hermes --skills sophonote-markdown-writing --in notes
```

The same natural-language intents and explicit operation names work in both modes.

CLI operates ordinary files and folders only. SophoNote titles, left-editor review, and project hierarchy require a project-scoped SophoNote Chat because those operations depend on Host identity, approval, and project boundaries.

## Output Contract

Use the smallest fitting response:

- **SophoNote mutating request:** edit the bounded working copy; return verification + “已提交到左侧原文审阅”. Never echo the full replacement.
- **SophoNote title request:** edit the bounded title region; return “已生成标题改名提案” until the user accepts it.
- **SophoNote project-tree request:** write only the bounded project-action JSON and wait for the Host result; never claim that local filesystem folders were created.
- **SophoNote advisory request:** leave the working copy unchanged; return the issue list or advice.
- **Ordinary-file format:** formatted Markdown + verification unless direct file editing was explicitly requested.
- **Ordinary-file proofread:** issue table; corrected Markdown only when requested.
- **Template without a SophoNote target:** complete copyable Markdown with visible placeholders.
- **Host patch:** say “proposed” until the user accepts it in the left editor; never claim the real article was already saved.

Do not wrap a full Markdown document in a fenced code block unless the interface requires a copy-only artifact. Fenced wrappers interfere with nested code blocks.

## Pitfalls

- Treating “format” as permission to rewrite.
- Expanding a selected range to the whole note.
- Flattening code, Mermaid, math, footnotes, wiki links, or task states.
- Inventing facts while filling a template.
- Printing the whole replacement in the right Chat instead of editing the SophoNote working copy.
- Deleting or moving `SOPHONOTE_EDITABLE_START` / `SOPHONOTE_EDITABLE_END`.
- Applying a whole-document replacement when a selected-range working copy is present.
- Claiming that SophoNote saved or applied changes without host confirmation.
- Editing YAML frontmatter to rename a note instead of the bounded title region.
- Treating a project document tree as an unrestricted filesystem tree.
- Treating example paths as real files, or falling back to a `lease_id` MCP when a `sophonote-selection` or `sophonote-document` attachment already contains the source.

## Verification

Before returning:

- Confirm the target stayed inside the requested selection or attachment and both editable markers remain exactly once.
- Confirm format-only output preserved textual content and order.
- Confirm proofreading did not alter uncertain proper nouns silently.
- Confirm generated templates contain placeholders instead of invented facts.
- Confirm all code fences and Markdown extensions remain balanced.
- Confirm title edits remained inside the title markers and are still a single valid line.
- Confirm every project child belongs to the same project and no parent cycle was introduced.
- State whether the result is a preview, copyable output, or an applied host patch.
