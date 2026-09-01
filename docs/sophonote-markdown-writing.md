# SophoNote Markdown writing guide

**English** | [简体中文](./sophonote-markdown-writing.zh-CN.md)

`sophonote-markdown-writing` and `sophonote-note-persistence` are shared Hermes Skills for Notes and Studio. The first handles writing and structure. The second persists finished session work into an explicit project, current document, or selection. Body edits enter left-side hunk review. Title edits produce a confirm card. New project documents are created by the SophoNote Host. The right-side chat shows process and a short result only. Do not copy full Markdown back into the editor.

## 1. Getting started

1. Click “加入会话” on a project or document row in Studio. Project scope is for new articles or folders. Document scope is for reading, extending, or rewriting the current article.
2. To work on a fragment, select text and add it to Chat. Otherwise use the “当前文档” or “项目” scope shown above the input.
3. Send natural language directly. To force the writing rules, type `/` and pick `sophonote-markdown-writing`.
4. Hermes edits the session working copy of the current document. SophoNote opens review from a real working-copy change. Explicitly picking the Skill is not required.
5. Accept or reject body hunks in the left original. Confirm title changes on the rename card.

When the article is already finished in this session and the user says “需要”, “保存”, or “写入文章”, `sophonote-note-persistence` uses the bound target: current document → left-side review; project → create a new document. Ask once only if the target or article shape is actually unclear. Do not require “create empty note → attach → send a sentence”.

The same rule applies to “research and write an article” on the first turn: if a project or document chip is already shown, Hermes researches, writes, and persists in that turn. If there is no writable target, ask once for destination and article shape, then start the long write. Do not first dump a draft on the right for a second copy-paste.

Do not invent `@file:notes/draft.md`. SophoNote attaches the real editor draft at send time as the Hermes session working copy.

If the right-side reply is done but the left side did not change, this turn produced no reviewable diff. Do not treat the right-side prose as “already written”. Body edits should show accept/reject on the left immediately. Only accept writes the original.

## 2. Writing operations

| Command | Example | Default |
|---|---|---|
| `format` | “Format Markdown only, change no words” | Structure only: headings, lists, tables, indent, blank lines. Do not change words, numbers, punctuation, links, code, or paragraph order |
| `proofread` | “Proofread this note, list issues first” | List typos, grammar, ambiguity, and proper-name risks first. Do not rewrite the body unless asked |
| `structure` | “Organize into three chapters, keep meaning” | Change heading hierarchy and organization. Do not polish by default |
| `outline` | “Make a research outline from this” | Extract a hierarchical outline from existing material. Do not invent facts |
| `rewrite` | “Make it shorter and more formal” | Polish in the requested tone. Wording may change; facts and terms stay |
| `continue` | “Continue the next section of the selection” | Continue from visible context only. Mark missing facts as `[待确认]` |
| `template` | “Generate a meeting-notes template” | Meeting notes, research notes, project brief, daily note, decision record |
| `actions` | “Extract todos from this note” | Extract todos, owners, and due dates. Mark uncertain items as `[待确认]` |

Commands can take constraints, for example: `rewrite 更克制，不改专有名词`, `proofread 只给建议`, `template 决策记录`.

## 3. Titles and body

- Title: ask to rename the current note. Hermes edits only the constrained title region. SophoNote checks the old title, length, and project-name collision, then shows a rename card.
- Body: ask to `format` the body. Hermes edits only the constrained body. SophoNote turns a real diff into a left-side reviewable patch.
- Both: title and body use separate confirm paths.
- Selection: select first, then ask. The Skill must not silently expand the selection to the whole document.

## 4. Project folders

A project folder is a document-as-folder node, not an arbitrary filesystem directory. Click “加入会话” on the project row, then ask, for example:

```text
创建“研究”目录，下面建立“资料”“实验”“结论”三个子文档；
在“实验”下面再创建“实验记录”和“结果分析”。
```

The Host creates documents parent-to-child, sets parents, rejects cross-project parents and cycles, then refreshes the left tree. Existing documents move only when identity is clear. If names collide or the target is unclear, the Skill should ask first.

## 5. Templates

- `template 会议纪要`
- `template 研究笔记`
- `template 项目简报`
- `template 每日笔记`
- `template 决策记录`

Templates use placeholders such as `[待填写]`. They must not invent times, attendees, conclusions, evidence, or owners.

## 6. Images as context

Pick an image from `+` at the bottom-left of the input, or paste a screenshot. SophoNote uses the same `image.attach_bytes` protocol as Hermes Desktop and puts the raw image into the current Hermes session. It does not treat a local image path as text, and it does not ask the Agent to call a nonexistent `vision_analyze`.

Whether the image is understood still depends on the selected model/provider. If the provider returns “images/vision not supported”, switch to a configured vision-capable model and retry. Upload failure, unsupported extension, or Hermes attachment-limit errors must show a clear attachment error instead of a guessed answer.

## 7. Hermes CLI

The CLI can use the same Skill on ordinary files, but it does not have SophoNote title confirmation, left-side patches, or project-folder permissions:

```bash
ARTICLE_MD='/absolute/path/to/article.md'
hermes --skills sophonote-markdown-writing -z "format @file:${ARTICLE_MD}"
hermes --skills sophonote-markdown-writing -z "proofread @file:${ARTICLE_MD}，先给建议"
hermes --skills sophonote-markdown-writing --in notes
```

The CLI requires a real absolute path. To change a SophoNote title, current document, or project tree, go back to Notes/Studio Chat.

## 8. FAQ

- **The right side dumped a full article**: Confirm a real current document is open, keep the document-scope chip, and use this Skill. Do not replace the current document with a sample path.
- **Missing `lease_id`**: That is the old SophoNote MCP path. Current Skills use the session working copy and Host action area. For the bundled runtime, run `pnpm hermes:bundle` and restart SophoNote. Run `./scripts/sophonote.sh skills` only when attaching external Hermes.
- **`Auxiliary title generation failed: HTTP 400 response_format`**: That is the old Hermes auxiliary session-title job, not the body Skill. SophoNote’s private runtime has turned that duplicate off. Confirm the process uses the bundled/private Hermes Home.
- **Folder did not appear**: Confirm the “项目” scope chip above the input. Global sessions and ordinary CLI do not have that Host permission.
- **Still gets a how-to after “需要”**: Confirm `sophonote-note-persistence` is bundled and enabled. The new behavior should write the unique bound target. If there is no target, only prompt to click “加入会话” on the left.
- **Format changed words**: Reject the left-side patch. A `format` that fails word conservation must block apply. Use `proofread` or explicitly choose `rewrite`.
