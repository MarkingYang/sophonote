//! 用户可见文本脱敏：禁止把 Bridge/loopback 等宿主内部信息泄露到 Chat。

use regex::Regex;
use std::sync::OnceLock;

fn leak_res() -> &'static [Regex] {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        [
            r"(?i)sophonote[\s_-]*mcp",
            r"(?i)mcp[\s_-]*桥接?",
            r"(?i)通过\s*mcp",
            r"(?i)桥接在",
            r"(?i)sophonote-bridge",
            r"(?i)127\.0\.0\.1:\d+",
            r"(?i)localhost:\d+",
            r"(?i)https?://127\.\S+",
            r"(?i)curl\s+[^\n]+",
            r"(?i)Bearer\s+[A-Za-z0-9._\-]+",
            r"(?i)X-SophoNote-Lease-Id[^\n]*",
            r#"(?i)leaseId\s*[:=]\s*["']?lease-[A-Za-z0-9\-]+["']?"#,
            r"(?i)lease-[0-9a-f]{16,}",
            r"(?i)/mcp\b",
            r"(?i)直接用\s*curl[^\n]*",
            r"(?i)调\s*[:：]?\s*$",
            r"(?i)\bread_file\b",
            r"/Users/[^\s，。；;]+",
            r"(?i)workspace/coding[^\s，。；;]*",
        ]
        .into_iter()
        .map(|p| Regex::new(p).expect("sanitize regex"))
        .collect()
    })
}

/// 去掉宿主内部调用信息；若整段几乎只剩泄露内容则返回空串。
pub fn sanitize_user_facing_text(input: &str) -> String {
    let mut out = input.to_string();
    for re in leak_res() {
        out = re.replace_all(&out, "").to_string();
    }
    // 清理：保留最多一个空行（Markdown 段落/标题间距需要空行）
    let mut filtered: Vec<String> = Vec::new();
    let mut prev_blank = false;
    for line in out.lines().map(str::trim_end) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank && !filtered.is_empty() {
                filtered.push(String::new());
            }
            prev_blank = true;
            continue;
        }
        // Markdown thematic break 是合法结构，不能按「纯标点残片」删除。
        let thematic: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if thematic.len() >= 3
            && (thematic.chars().all(|c| c == '-')
                || thematic.chars().all(|c| c == '*')
                || thematic.chars().all(|c| c == '_'))
        {
            prev_blank = false;
            filtered.push(trimmed.to_string());
            continue;
        }
        prev_blank = false;
        let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        // 「在。」「：」等残片
        if compact.chars().all(|c| {
            matches!(
                c,
                '。' | '，' | '：' | ':' | '.' | ',' | '、' | '-' | '—' | '在' | '于'
            )
        }) {
            continue;
        }
        // 正文只去尾随空白，保留围栏代码/ASCII 图依赖的行首缩进。
        filtered.push(line.to_string());
    }
    filtered.join("\n").trim().to_string()
}

/// 流式增量脱敏：与整段脱敏相同规则，但完整保留首尾空白。
///
/// token 流会把 `### ` 拆成 `"###"` + `" "`，也会把围栏前的换行单独发送。
/// `sanitize_user_facing_text` 末尾的 `trim` 若用在每个 delta 上，会吃掉这些
/// Markdown 结构字符，直到终态 finalAnswer 才突然恢复排版。
pub fn sanitize_user_facing_delta(input: &str) -> String {
    // 增量不是完整句子，不能复用 `sanitize_user_facing_text` 的段落清理：
    // Hermes 常把 `：`、`。`、`在`、换行分别作为独立 token 发送，完整文本
    // 清理会把它们误判为残片，最终造成流式 Markdown 与终态逐字不一致。
    // 这里只执行可在单帧内安全完成的泄露替换，其余布局字符原样透传。
    let mut out = input.to_string();
    for re in leak_res() {
        out = re.replace_all(&out, "").to_string();
    }
    out
}

/// 是否像内部排障话术（整段应丢弃）
pub fn looks_like_internal_ops_leak(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("sophonote mcp")
        || t.contains("sophonote-bridge")
        || t.contains("mcp桥接")
        || t.contains("mcp 桥接")
        || t.contains("localhost:")
        || t.contains("127.0.0.1:")
        || t.contains("leaseid")
        || t.contains("lease_id")
        || (t.contains("curl") && (t.contains("mcp") || t.contains("localhost")))
        || (t.contains("read_file")
            && (t.contains("文档") || t.contains("markdown") || t.contains("/users/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bridge_url_and_curl() {
        let raw = "SophoNote MCP 桥接在 localhost:56946。直接用 curl 调：\ncurl http://127.0.0.1:56946/mcp\n文档很好。";
        let s = sanitize_user_facing_text(raw);
        assert!(!s.contains("56946"));
        assert!(!s.to_ascii_lowercase().contains("curl"));
        assert!(!s.to_ascii_lowercase().contains("mcp"));
        assert!(s.contains("文档很好"));
    }

    #[test]
    fn detects_ops_leak() {
        assert!(looks_like_internal_ops_leak(
            "SophoNote MCP 桥接在 localhost:56946。直接用 curl 调:"
        ));
        assert!(!looks_like_internal_ops_leak("本文介绍 AG-UI 架构。"));
    }

    #[test]
    fn full_text_preserves_markdown_thematic_breaks() {
        let raw = "第一节\n\n---\n\n第二节";
        assert_eq!(sanitize_user_facing_text(raw), raw);
    }

    #[test]
    fn full_text_preserves_code_block_indentation() {
        let raw = "```\n  一级\n    二级\n```";
        assert_eq!(sanitize_user_facing_text(raw), raw);
    }

    #[test]
    fn delta_preserves_trailing_newlines() {
        let s = sanitize_user_facing_delta("标题\n\n");
        assert!(s.ends_with("\n\n"), "got {s:?}");
        assert!(s.starts_with("标题"));
    }

    #[test]
    fn delta_preserves_pure_newline_chunks() {
        let s = sanitize_user_facing_delta("\n");
        assert_eq!(s, "\n");
        let s2 = sanitize_user_facing_delta("\n\n");
        assert_eq!(s2, "\n\n");
    }

    #[test]
    fn delta_preserves_spaces_that_define_markdown_tokens() {
        assert_eq!(sanitize_user_facing_delta(" "), " ");
        assert_eq!(sanitize_user_facing_delta("\t"), "\t");
        assert_eq!(sanitize_user_facing_delta("## "), "## ");
        assert_eq!(sanitize_user_facing_delta(" | "), " | ");
        assert_eq!(sanitize_user_facing_delta("\n```\n"), "\n```\n");
    }

    #[test]
    fn delta_preserves_standalone_punctuation_and_short_words() {
        for token in ["：", "，", "。", "、", "在", "于"] {
            assert_eq!(sanitize_user_facing_delta(token), token);
        }
    }

    #[test]
    fn delta_chunks_reconstruct_markdown_exactly() {
        let chunks = [
            "##",
            " ",
            "标题",
            "：",
            "流式预览",
            "。",
            "\n\n",
            "```",
            "\n",
            "代码",
            "\n",
            "```",
        ];
        let actual = chunks
            .iter()
            .map(|chunk| sanitize_user_facing_delta(chunk))
            .collect::<String>();
        assert_eq!(actual, "## 标题：流式预览。\n\n```\n代码\n```");
    }

    #[test]
    fn delta_still_strips_leaks() {
        let s = sanitize_user_facing_delta("见 localhost:18765/mcp\n正文\n");
        assert!(!s.contains("18765"));
        assert!(s.contains("正文"));
        assert!(s.ends_with('\n'));
    }

    #[test]
    fn strips_short_lived_lease_capabilities() {
        let raw = "leaseId=lease-0123456789abcdef0123456789abcdef，继续读取正文";
        let sanitized = sanitize_user_facing_text(raw);
        assert!(!sanitized.contains("0123456789abcdef"));
        assert!(sanitized.contains("继续读取正文"));
    }
}
