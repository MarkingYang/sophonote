---
name: sophonote-note-persistence
description: Create or persist a Hermes writing deliverable in the SophoNote project or note attached by the user. Use both when the user initially asks to research, summarize, or write an article/brief/note for SophoNote and when they later say save, write into the note, add to the current document, “需要”, “写入文章”, or “补充到当前笔记”. Resolve destination and form before doing long-form work, then write directly instead of leaving the artifact only in Chat.
---

# SophoNote Note Persistence

Turn a completed Session result into a SophoNote document with the fewest possible user actions. SophoNote, not Hermes, owns real documents: edit only the bounded Session working copy and let the Host validate the terminal change.

## Plan Destination Before Long-form Work

For an initial request such as “总结最近的 DeepSeek Harness 并写一篇文章”, decide destination and artifact form before spending a full turn producing a long Chat-only draft:

1. If exactly one `sophonote-document` or `sophonote-selection` is attached, treat it as the destination and write there in the same turn.
2. If exactly one `sophonote-project` is attached, create the requested article in that project in the same turn.
3. If no writable SophoNote target is attached and the user clearly wants a persistent article, ask one compact combined question, for example: `写到哪个项目或文档？按研究文章（含摘要、章节和来源）来写可以吗？请从左侧点“加入会话”。`
4. If the user asked only for an answer or summary and did not imply a SophoNote artifact, answer normally; do not force persistence.

Do not generate the full article first, ask `需要我保存吗？`, and then prescribe setup steps. When the destination and form are already clear, research, draft, and update the bounded working copy within the same Run. Chat should show progress and a concise completion result; the left editor/project tree is the artifact surface.

## Target Resolution

Resolve one target in this strict order:

1. `source: sophonote-selection`: update only the selected range in the current note.
2. `source: sophonote-document`: update the visible current note, including its bounded title when requested.
3. `source: sophonote-project`: create a new note in the explicitly attached project by writing one `create_document` action.
4. An ordinary file explicitly attached or named by the user.

The attachment filename may have a numeric suffix in a long Session. Identify it by frontmatter `source`, not by an exact filename.

Never infer a writable target merely because an old Session message mentions a project or title. If there is no SophoNote attachment and no explicit ordinary path, ask one short question: `写到哪个项目或文档？请从左侧点“加入会话”。` Do not provide a multi-step tutorial.

## Execute Strong Intent Immediately

Treat these as an instruction to write, not as a request for instructions:

- `需要`
- `保存`
- `写入文章`
- `写进当前笔记`
- `补充到左侧文档`
- `把刚才的内容放到这个项目`
- `总结一下写一篇文章`
- `整理成笔记/项目简报/研究文章`

If the target is unique and the content form is already clear from the previous turn, execute immediately. Do not ask the user to create a note, attach it, and send another message.

Ask exactly one concise clarification only when one of these is genuinely ambiguous:

- both a project and a document are attached but the user did not choose whether to create or update;
- the requested artifact form is unclear and materially changes the result;
- more than one named existing document could be the target.

When clarification is needed, combine destination and form in one question, for example: `写入当前《调研草稿》并保留研究文章结构，还是在“Agent Harness”项目中新建一篇？`

## Current Document or Selection

`sophonote-document` and `sophonote-selection` are Session working copies. Their mutable content is bounded by:

```text
<!-- SOPHONOTE_EDITABLE_START -->
...
<!-- SOPHONOTE_EDITABLE_END -->
```

Replace only the content between those markers. Keep frontmatter and every marker exactly once and in place. A selection target must never expand to the full document.

For a full current document, an optional title is bounded by:

```text
<!-- SOPHONOTE_TITLE_START -->
...
<!-- SOPHONOTE_TITLE_END -->
```

Edit the title only when the user requested a title or a clear title is part of creating the deliverable. Keep it one non-empty line of at most 200 characters. Never edit YAML identity fields.

Use the best complete Markdown artifact already present in the Session as the source. Do not regenerate a shorter approximation. If the user asked to append, merge with the current editable content; otherwise replace an empty placeholder document with the finished artifact.

At turn end, SophoNote converts the difference into a left-editor review surface. Return only a short status; never print a duplicate full document in Chat.

## Project Target

A `sophonote-project` working copy contains a read-only manifest and one writable project-action array:

```text
<!-- SOPHONOTE_PROJECT_MANIFEST_START -->
[
  {"articleId":"existing-id","title":"Existing note","parentArticleId":null}
]
<!-- SOPHONOTE_PROJECT_MANIFEST_END -->

<!-- SOPHONOTE_PROJECT_ACTIONS_START -->
[]
<!-- SOPHONOTE_PROJECT_ACTIONS_END -->
```

Do not edit the manifest, project metadata, or markers. To save the finished Session artifact as a new project note, replace only the action array:

```json
[
  {
    "type": "create_document",
    "client_id": "session-article",
    "title": "DeepSeek Harness 发布解读",
    "content": "# DeepSeek Harness 发布解读\n\n...",
    "parent_article_id": null
  }
]
```

Rules:

- Use the complete Markdown artifact from the Session as `content`.
- Choose a concise title from the artifact when the user did not provide one.
- If the manifest already has an unambiguous same-purpose document, do not overwrite it from project scope because its body is not attached. Ask the user to attach that document, or ask whether to create a new note.
- Never invent an existing `articleId` and never use a legacy SophoNote MCP or `lease_id`.
- Use `parent_article_id` only when the user explicitly selected or named a known manifest parent.
- Create at most one article for an ordinary “save this article” request. Use nested actions only when the user explicitly requested a hierarchy.

SophoNote applies validated project actions at turn end and refreshes the left project tree. Do not claim success before the Host result appears.

## Writing Form

Preserve the form already agreed in the Session. If none was agreed, infer the smallest conventional form:

- research/news request → research article with title, summary, sections, and sources;
- meeting content → meeting notes;
- daily capture → daily note;
- decision discussion → decision record;
- implementation discussion → project brief.

Do not add unsupported facts. Keep citations, links, code, names, and dates intact.

## Response Contract

- Current document or selection: `已把内容提交到左侧原文审阅。`
- Project creation: wait for the Host project-action result, then report the created title and location.
- Ambiguous target: ask one short destination/form question.
- Missing target: ask the user to click the left project or document’s `加入会话`; do not prescribe creating a placeholder note first.

## Forbidden Patterns

- Telling the user to create a note, then Add to Chat, then send another phrase.
- Asking `需要我保存吗？` after the user already said `需要` or `保存`.
- Returning the full article only in the right Chat when a writable SophoNote target is attached.
- Searching local files for a synthetic `@file:notes/draft.md`.
- Falling back to a legacy MCP or asking for `lease_id`.
- Claiming a project or note was updated before SophoNote Host confirms it.
