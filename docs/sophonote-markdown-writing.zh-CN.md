# SophoNote Markdown 写作 Skill 使用指南

[English](./sophonote-markdown-writing.md) | **简体中文**

`sophonote-markdown-writing` 与 `sophonote-note-persistence` 是笔记本和工作室共用的 Hermes Skills。前者负责写作与结构变换，后者负责把已经完成的会话成果落到明确的项目、当前文档或选区。正文修改进入左侧逐块审阅，标题修改生成确认卡，项目新文档由 SophoNote Host 创建；右侧 Chat 只展示过程和简短结果，不再要求复制完整 Markdown 回编辑器。

## 1. 开始使用

1. 在工作室左侧项目或文档行点击“加入会话”。项目范围适合创建新文章或目录；文档范围适合阅读、补充和改写当前文章。
2. 需要处理局部内容时，先选中文本并加入 Chat；不选时使用输入框上方显示的“当前文档”或“项目”范围。
3. 直接输入自然语言并发送；需要严格使用写作规则时，也可以先输入 `/`，搜索并选择 `sophonote-markdown-writing`。
4. Hermes 修改的是当前文档的 Session 工作副本；SophoNote 以工作副本是否真实变化为准生成审阅，不要求必须显式选择 Skill。
5. 正文变更在左侧原文逐块接受或拒绝；标题变更在改名卡中确认。

当文章已经在当前 Session 中生成完毕，用户再说“需要”“保存”“写入文章”时，`sophonote-note-persistence` 会直接使用已经绑定的目标：当前文档就提交左侧审阅，项目就创建一篇新文档。只有目标或文章形态确实不明确时才问一次，不再要求用户先新建空笔记、再附加、再发送一句话。

首次提出“调研并写一篇文章”时也遵循同一规则：如果输入框已经显示项目或文档 chip，Hermes 会在同一轮完成调研、写作和写入；如果没有可写目标，只合并询问一次写入位置与文章形式，再开始长文生成，不先在右侧生成一份等待二次搬运。

不要手写虚构的 `@file:notes/draft.md`。SophoNote 会把发送时刻的真实编辑器草稿作为 Hermes Session 工作副本附加到本轮。

如果右侧回复已经完成但左侧没有出现任何变化，表示本轮没有生成可审阅 Diff；不要按右侧自然语言结论判断“已经写入”。正常情况下，正文修改会立即在左侧显示逐块接受/拒绝入口，只有接受后才写入原文。

## 2. 支持的写作操作

| 命令 | 自然语言示例 | 默认行为 |
|---|---|---|
| `format` | “只整理 Markdown 格式，不改任何文字” | 仅调整标题、列表、表格、缩进、空行等结构；不改变文字、数字、标点、链接、代码和段落顺序 |
| `proofread` | “校对这篇笔记，先列问题” | 先给错别字、语病、歧义和专名风险建议；未明确要求时不直接改正文 |
| `structure` | “把内容整理成三章，保留原意” | 调整章节层级与内容组织；不默认润色 |
| `outline` | “从当前内容生成研究大纲” | 从现有材料提取层级大纲，不编造事实 |
| `rewrite` | “改得更简洁、正式” | 按指定语气润色，允许改变措辞但保留事实和术语 |
| `continue` | “续写选中内容的下一节” | 只基于可见上下文续写，缺失事实标为 `[待确认]` |
| `template` | “生成会议纪要模板” | 支持会议纪要、研究笔记、项目简报、每日笔记和决策记录模板 |
| `actions` | “从这篇笔记提取待办” | 提取待办、负责人和截止时间；不确定项标为 `[待确认]` |

命令可以继续带约束，例如：`rewrite 更克制，不改专有名词`、`proofread 只给建议`、`template 决策记录`。

## 3. 修改标题与正文

- 标题：输入“把当前笔记标题改为《检索策略复盘》”。Hermes 只修改受限标题区，SophoNote 校验旧标题、长度和项目重名后显示改名确认卡。
- 正文：输入“正文执行 format”。Hermes 只修改受限正文区，SophoNote 将真实差异转换为左侧可审阅补丁。
- 同时处理：输入“标题改为《检索策略复盘》，正文只做 format”。标题和正文走各自独立的确认路径。
- 局部处理：先选择文本，再输入“proofread 先列问题”或“continue 补充一个例子”。Skill 不会把选区静默扩大到整篇。

## 4. 生成项目目录和子目录

项目目录是“文档作为目录节点”的层级，不是机器上的任意文件夹。点击左侧项目行的“加入会话”后即可发起，例如：

```text
创建“研究”目录，下面建立“资料”“实验”“结论”三个子文档；
在“实验”下面再创建“实验记录”和“结果分析”。
```

SophoNote Host 会按父到子顺序创建文档、设置父级、拒绝跨项目父节点和循环，并在成功后刷新左侧项目树。已有文档只有在身份明确时才会移动；同名或目标不清楚时 Skill 应先询问。

## 5. 模板示例

- `template 会议纪要`
- `template 研究笔记`
- `template 项目简报`
- `template 每日笔记`
- `template 决策记录`

模板使用 `[待填写]` 等占位符，不会虚构时间、参与人、结论、证据或负责人。

## 6. 图片作为上下文

通过输入框左下角 `+` 选择图片，或直接粘贴截图后发送。SophoNote 使用 Hermes Desktop 同源的 `image.attach_bytes` 协议，把原始图片放进当前 Hermes Session；不会把本地图片路径当作文字，也不会要求 Agent 调用一个不存在的 `vision_analyze`。

图片能否被理解仍取决于当前选择模型/Provider 的多模态能力。若 Provider 返回“不支持图片/vision”的错误，请切换到已配置且支持图片的模型后重试。图片上传失败、扩展名不支持或超过 Hermes 附件上限时，Chat 会显示明确的附件错误，而不是继续生成猜测性答案。

## 7. Hermes CLI

CLI 可用同一 Skill 操作普通文件，但没有 SophoNote 的标题确认、左侧补丁和项目目录权限：

```bash
ARTICLE_MD='/absolute/path/to/article.md'
hermes --skills sophonote-markdown-writing -z "format @file:${ARTICLE_MD}"
hermes --skills sophonote-markdown-writing -z "proofread @file:${ARTICLE_MD}，先给建议"
hermes --skills sophonote-markdown-writing --in notes
```

CLI 中必须使用真实绝对路径。需要修改 SophoNote 标题、当前文档或项目树时，请回到笔记本/工作室 Chat。

## 8. 常见问题

- **右侧输出了整篇文章**：确认已打开真实当前文档、保留文档范围 chip，并已选择本 Skill；不要用样例路径代替当前文档。
- **提示缺少 `lease_id`**：这是旧 SophoNote MCP 路径。当前 Skill 应使用 Session 工作副本和 Host 操作区。默认包内 Runtime 请执行 `pnpm hermes:bundle` 后重启 SophoNote；只有显式附着外部 Hermes 时才执行 `./scripts/sophonote.sh skills`。
- **出现 `Auxiliary title generation failed: HTTP 400 response_format`**：这是旧 Hermes 辅助会话标题任务，不是正文 Skill。SophoNote 私有 Runtime 已关闭该重复能力；确认当前进程使用随包/私有 Hermes Home。
- **目录未出现**：确认输入框上方显示“项目”范围 chip；全局会话和普通 CLI 不具备该 Host 权限。
- **说“需要”后仍给出操作教程**：确认 `sophonote-note-persistence` 已随包安装并启用；新行为应直接写入唯一绑定目标，缺少目标时只提示从左侧点击“加入会话”。
- **格式化改变了文字**：拒绝左侧补丁。`format` 的文字守恒失败必须阻断应用，可改用 `proofread` 或明确选择 `rewrite`。
