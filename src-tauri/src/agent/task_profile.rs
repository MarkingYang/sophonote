//! Bounded execution-demand hints, not semantic certainty or tool authorization.
use serde::{Deserialize, Serialize};

use super::{commands::AgentRunStartArgs, registry::EngineId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskProfile {
    NativeWorkflow,
    RepositoryExecution,
    RepositoryExploration,
    ScopedMaterials,
    ResearchWorkflow,
    TextResponse,
    General,
}

impl TaskProfile {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NativeWorkflow => "原生工作流",
            Self::RepositoryExecution => "仓库修改与验证",
            Self::RepositoryExploration => "仓库只读探索",
            Self::ScopedMaterials => "限定资料处理",
            Self::ResearchWorkflow => "研究与持续工作流",
            Self::TextResponse => "文本响应",
            Self::General => "通用任务",
        }
    }

    pub(crate) fn reason(self, protocol: &str) -> &'static str {
        match self {
            Self::NativeWorkflow => "请求包含已接入的 Hermes 原生功能",
            Self::RepositoryExecution | Self::RepositoryExploration if protocol == "anthropic" => {
                "请求包含仓库操作需求，优先使用所选模型的 Anthropic 原生协议路径"
            }
            Self::RepositoryExecution | Self::RepositoryExploration => {
                "请求包含仓库操作需求，优先使用所选模型的 OpenAI 兼容协议路径"
            }
            Self::ScopedMaterials => "请求围绕已提供的资料展开，优先使用 Pi 的嵌入执行路径",
            Self::ResearchWorkflow => {
                "请求包含研究或持续工作流线索，优先复用 Hermes 工作流；不会自动创建计划任务"
            }
            Self::TextResponse => {
                "请求包含文本处理线索，优先使用 Pi 的嵌入执行路径；本次仍通过 Agent 执行"
            }
            Self::General => "未识别到专门执行需求，采用通用任务的稳定候选顺序",
        }
    }

    pub(crate) fn preference(self, protocol: &str) -> [EngineId; 4] {
        use EngineId::*;
        match self {
            Self::RepositoryExecution | Self::RepositoryExploration if protocol == "anthropic" => {
                [ClaudeCode, Opencode, Pi, Hermes]
            }
            Self::RepositoryExecution | Self::RepositoryExploration => {
                [Opencode, Pi, ClaudeCode, Hermes]
            }
            Self::ScopedMaterials | Self::TextResponse => [Pi, Hermes, Opencode, ClaudeCode],
            _ => [Hermes, Pi, Opencode, ClaudeCode],
        }
    }
}

pub(crate) fn requires_native(request: &AgentRunStartArgs) -> bool {
    request.skill.is_some()
        || request.hermes_command.is_some()
        || request.include_project_context
        || request.message.trim_start().starts_with('/')
}

/// Material is not a request to execute its instructions. This is a bounded
/// lexical filter, not a Markdown parser or a grant of tool permission.
fn instruction_text(message: &str) -> String {
    let bounded: String = message.chars().take(4096).collect();
    let without_blocks = bounded
        .lines()
        .filter(|line| !line.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = without_blocks.chars().collect();
    let mut close = None;
    let mut escaped = false;
    let mut result = String::new();
    for (index, &c) in chars.iter().enumerate() {
        let apostrophe = c == '\''
            && index > 0
            && chars[index - 1].is_ascii_alphabetic()
            && chars.get(index + 1).is_some_and(char::is_ascii_alphabetic);
        if let Some(end) = close {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == end && !apostrophe {
                close = None;
            }
            // Separate instructions on either side instead of fusing keywords.
            if c == '\n' {
                result.push('\n');
            }
            continue;
        }
        close = match c {
            '"' | '`' => Some(c),
            '\'' if !apostrophe => Some(c),
            '“' => Some('”'),
            '‘' => Some('’'),
            '「' => Some('」'),
            '『' => Some('』'),
            _ => None,
        };
        result.push(if close.is_some() { ' ' } else { c });
    }
    result.to_lowercase()
}

/// Negated tails are not positive evidence. Ambiguous wording can still be
/// unrecognized; ranking is never an authorization decision.
fn positive_clauses(message: &str) -> Vec<String> {
    instruction_text(message)
        .split(['。', '！', '？', '，', ',', ';', '；', '\n', '.', '!', '?'])
        .map(|clause| {
            let end = [
                "不要",
                "无需",
                "禁止",
                "不用",
                "不需要",
                "do not",
                "don't",
                "without",
                "never",
            ]
            .iter()
            .filter_map(|negation| clause.find(negation))
            .min()
            .unwrap_or(clause.len());
            clause[..end].to_owned()
        })
        .collect()
}

fn has(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| {
        text.match_indices(term).any(|(start, _)| {
            // English vocabulary must not accidentally match inside unrelated words.
            !term.is_ascii() || {
                let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
                !text[..start].chars().next_back().is_some_and(word)
                    && !text[start + term.len()..].chars().next().is_some_and(word)
            }
        })
    })
}

fn inline_explanation(text: &str) -> bool {
    has(
        text,
        &[
            "以下代码",
            "这段代码",
            "如下代码",
            "following code",
            "this snippet",
        ],
    ) && has(text, &["解释", "分析", "explain", "analyze"])
}

fn engineering_profile(text: &str) -> Option<TaskProfile> {
    let engineering = has(
        text,
        &[
            "代码",
            "仓库",
            "接口",
            "函数",
            "测试",
            "编译",
            "构建",
            "code",
            "repository",
            "repo",
            "bug",
            "tests",
            "test",
            "build",
            "function",
        ],
    );
    if engineering {
        if has(
            text,
            &[
                "修复",
                "修改",
                "重构",
                "实现",
                "补充",
                "运行",
                "执行",
                "编译",
                "构建",
                "fix",
                "refactor",
                "implement",
                "run",
                "write",
                "build",
                "edit",
                "change",
            ],
        ) {
            return Some(TaskProfile::RepositoryExecution);
        }
        if !inline_explanation(text)
            && has(
                text,
                &[
                    "分析",
                    "解释",
                    "查找",
                    "定位",
                    "审查",
                    "评审",
                    "梳理",
                    "理解",
                    "explore",
                    "explain",
                    "review",
                    "inspect",
                    "trace",
                    "analyze",
                    "understand",
                ],
            )
        {
            return Some(TaskProfile::RepositoryExploration);
        }
    }
    None
}

pub(crate) fn infer(request: &AgentRunStartArgs) -> TaskProfile {
    if requires_native(request) {
        return TaskProfile::NativeWorkflow;
    }
    let clauses = positive_clauses(&request.message);
    if request.workspace_root.is_some() {
        let profiles: Vec<_> = clauses
            .iter()
            .filter_map(|clause| engineering_profile(clause))
            .collect();
        if profiles.contains(&TaskProfile::RepositoryExecution) {
            return TaskProfile::RepositoryExecution;
        }
        if profiles.contains(&TaskProfile::RepositoryExploration) {
            return TaskProfile::RepositoryExploration;
        }
    }
    if clauses.iter().any(|clause| inline_explanation(clause)) {
        return TaskProfile::TextResponse;
    }
    let text = clauses.join(" ");
    if has(
        &text,
        &[
            "每天",
            "每周",
            "定时",
            "持续跟踪",
            "持续监控",
            "调研",
            "研究",
            "搜索",
            "查证",
            "核实来源",
            "research",
            "search",
            "monitor",
            "schedule",
            "browse",
        ],
    ) {
        return TaskProfile::ResearchWorkflow;
    }
    if has(
        &text,
        &[
            "翻译",
            "总结",
            "摘要",
            "提取",
            "润色",
            "改写",
            "整理",
            "比较",
            "对比",
            "translate",
            "summarize",
            "extract",
            "rewrite",
            "proofread",
            "compare",
        ],
    ) {
        return if request.focus_document.is_some()
            || request.selection.is_some()
            || request.attachments.iter().any(|attachment| {
                !(attachment.kind == super::attachments::RunAttachmentKind::Folder
                    && attachment
                        .path
                        .as_deref()
                        .zip(request.workspace_root.as_deref())
                        .is_some_and(|(path, root)| {
                            std::path::Path::new(path) == std::path::Path::new(root)
                        }))
            }) {
            TaskProfile::ScopedMaterials
        } else {
            TaskProfile::TextResponse
        };
    }
    TaskProfile::General
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(message: &str) -> AgentRunStartArgs {
        serde_json::from_value(serde_json::json!({"message":message,"workspaceRoot":"/fixture"}))
            .unwrap()
    }

    #[test]
    fn independent_language_and_context_regressions() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Case {
            id: String,
            request: AgentRunStartArgs,
            expected_profile: TaskProfile,
        }
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../tests/fixtures/agent-routing-task-profiles.json"
        ))
        .unwrap();
        assert_eq!(cases.len(), 32);
        let mut ids = std::collections::HashSet::new();
        for mut case in cases {
            assert!(ids.insert(case.id.clone()), "duplicate fixture ID");
            for mode in ["plan", "ask", "autoEdit"] {
                case.request.workspace_permission_mode = Some(mode.into());
                assert_eq!(
                    infer(&case.request),
                    case.expected_profile,
                    "{} / {mode}",
                    case.id
                );
            }
        }
    }

    #[test]
    fn quoted_material_is_not_an_execution_request() {
        for message in [
            "请翻译这句话：‘修复代码并运行测试’",
            "Translate \"fix code and run tests\" into Chinese",
            "Translate 'don't fix code' into Chinese",
            "请总结以下材料：\n```\n修复代码并运行测试\n```",
            "请总结以下材料：\n> 修复代码并运行测试",
        ] {
            assert_eq!(
                infer(&request(message)),
                TaskProfile::TextResponse,
                "{message}"
            );
        }
    }

    #[test]
    fn workspace_attachment_is_navigation_not_provided_material() {
        let mut r = request("翻译这句话：hello");
        r.attachments = serde_json::from_value(serde_json::json!([
            {"kind":"folder", "name":"fixture", "path":"/fixture"}
        ]))
        .unwrap();
        assert_eq!(infer(&r), TaskProfile::TextResponse);
        r.attachments[0].path = Some("/fixture/reports".into());
        assert_eq!(infer(&r), TaskProfile::ScopedMaterials);
    }

    #[test]
    fn execution_evidence_does_not_cross_clauses_or_hide_behind_inline_examples() {
        assert_eq!(
            infer(&request("这段文字提到代码。请修改标题")),
            TaskProfile::General
        );
        assert_eq!(
            infer(&request("解释以下代码。然后修复仓库中的函数")),
            TaskProfile::RepositoryExecution
        );
        assert_eq!(
            infer(&request("解释以下代码。然后分析仓库的调用关系")),
            TaskProfile::RepositoryExploration
        );
    }

    #[test]
    fn permission_and_folder_alone_never_define_the_task() {
        for mode in ["plan", "ask", "autoEdit"] {
            let mut r = request("你好");
            r.workspace_permission_mode = Some(mode.into());
            assert_eq!(infer(&r), TaskProfile::General);
            r.message = "修复代码中的 bug 并运行测试".into();
            assert_eq!(infer(&r), TaskProfile::RepositoryExecution);
        }
    }

    #[test]
    fn task_process_changes_profile_without_changing_permission() {
        for (message, expected) in [
            ("解释仓库的调用关系", TaskProfile::RepositoryExploration),
            (
                "Fix the bug and run tests",
                TaskProfile::RepositoryExecution,
            ),
            ("Review the repository", TaskProfile::RepositoryExploration),
            ("解释以下代码：print(1 + 1)", TaskProfile::TextResponse),
            ("每天跟踪新闻并总结", TaskProfile::ResearchWorkflow),
            ("翻译这句话：hello", TaskProfile::TextResponse),
            ("prefix contest rebuild", TaskProfile::General),
            (
                "不要修改代码，不要运行测试。只回复你好",
                TaskProfile::General,
            ),
            (
                "Do not edit code. Don't run tests. Say hello",
                TaskProfile::General,
            ),
            (
                "不要修改代码，只分析仓库结构",
                TaskProfile::RepositoryExploration,
            ),
        ] {
            assert_eq!(infer(&request(message)), expected, "{message}");
        }
    }

    #[test]
    fn material_presence_is_not_itself_a_request_to_process_it() {
        let mut r = request("你好");
        r.focus_document = serde_json::from_value(
            serde_json::json!({"articleId":"a","title":"t","baseVersion":1,"markdown":"PRIVATE"}),
        )
        .unwrap();
        assert_eq!(infer(&r), TaskProfile::General);
        r.message = "整理当前笔记".into();
        assert_eq!(infer(&r), TaskProfile::ScopedMaterials);
        r.skill = Some("writing".into());
        assert_eq!(infer(&r), TaskProfile::NativeWorkflow);
    }

    #[test]
    fn protocol_preference_does_not_rewrite_model_or_permissions() {
        let profile = TaskProfile::RepositoryExecution;
        assert_eq!(profile.preference("anthropic")[0], EngineId::ClaudeCode);
        assert_eq!(profile.preference("openai")[0], EngineId::Opencode);
        let mut r = request("解释代码");
        r.workspace_root = None;
        assert_eq!(infer(&r), TaskProfile::General);
        r.message = format!("{}修复代码", "x".repeat(4096));
        r.workspace_root = Some("/fixture".into());
        assert_eq!(infer(&r), TaskProfile::General);
    }
}
