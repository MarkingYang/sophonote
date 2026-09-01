//! TextAnchor 解析与 Markdown 结构校验（AG-25）。
//!
//! 设计基线：docs/architecture.md（持久化位置 = 选中文本 hash +
//! 前后文 hash + 唯一匹配；零/多候选 → 冲突，绝不猜测位置）。
//!
//! 解析规则（`resolve_anchor`）：
//! 1. selected_text 为空 → 非法；
//! 2. 携带 hash 时先验 hash（捕获侧与提交侧不一致 = 锚点失效）；
//! 3. 全量查找 selected_text 出现位置；
//! 4. 多于 1 处时用前后文过滤消歧——过滤后恰 1 处 = 命中；0 处 = NotFound；
//!    仍多处 = Ambiguous。**不做就近匹配等任何形式的猜测。**
//!
//! 结构校验（`validate_patch_structure`，Markdown round-trip 的可单测结构不变量子集）：
//! CommonMark 解析不会失败，「parse 通过」无鉴别力；真正会坏的是结构——
//! ① 锚点范围不得跨越代码围栏内外边界（否则替换会劈开/截断代码块）；
//! ② 围栏内的替换不得引入新围栏标记（保持代码上下文的「纯」）；
//! ③ 围栏外的替换自身围栏必须配对；
//! ④ 正文开头不得被替换出伪 frontmatter。
//! 完整的 parse↔serialize 回环在编辑器刷新浪口天然发生（AG-26 Diff Overlay 接线）。

use super::repository;

// ------------------- 锚点 -------------------

/// 锚点错误语义（措辞稳定：经 ServiceError 进模型回填与用户可见错误）
#[derive(Debug, PartialEq)]
pub enum AnchorError {
    /// 锚点文本为空
    Empty,
    /// selected_text 与 selected_text_hash 不一致（捕获失效或被篡改）
    HashMismatch,
    /// 0 匹配，或文本存在但前后文过滤后无一幸存（目标已变化）
    NotFound,
    /// 前后文过滤后仍有 n>1 处候选：唯一性不足，禁止猜测
    Ambiguous(usize),
}

/// 持久化文本锚点（SelectionSnapshot 的 Rust 侧对应物，camelCase 对齐工具参数）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextAnchor {
    /// 选区原文（逐字存在于正文中）
    pub selected_text: String,
    /// selected_text 的 FNV-1a 64（16 位 hex；空串 = 不校验 hash）
    #[serde(default)]
    pub selected_text_hash: String,
    /// 前文（正文中命中位置之前的后缀；空 = 不参与消歧）
    #[serde(default)]
    pub before_context: String,
    /// 后文（命中位置之后的前缀；空 = 不参与消歧）
    #[serde(default)]
    pub after_context: String,
}

/// 把锚点解析为正文中唯一字节范围 `(start, end)`。
/// 0 匹配 / 消歧失败 / 多候选 = 错误返回，绝不猜测位置（docs/architecture.md）。
pub fn resolve_anchor(body: &str, anchor: &TextAnchor) -> Result<(usize, usize), AnchorError> {
    if anchor.selected_text.is_empty() {
        return Err(AnchorError::Empty);
    }
    if !anchor.selected_text_hash.is_empty()
        && repository::content_hash(&anchor.selected_text) != anchor.selected_text_hash
    {
        return Err(AnchorError::HashMismatch);
    }
    let needle = anchor.selected_text.as_str();
    let mut candidates: Vec<usize> = body.match_indices(needle).map(|(i, _)| i).collect();
    if candidates.is_empty() {
        return Err(AnchorError::NotFound);
    }
    // 多候选 → 前后文消歧（恰 1 处才命中；宁可冲突也不猜测）
    if candidates.len() > 1
        && (!anchor.before_context.is_empty() || !anchor.after_context.is_empty())
    {
        let needle_len = needle.len();
        candidates.retain(|&i| {
            let before_ok = anchor.before_context.is_empty()
                || body
                    .get(..i)
                    .is_some_and(|s| s.ends_with(anchor.before_context.as_str()));
            let after_ok = anchor.after_context.is_empty()
                || body
                    .get(i + needle_len..)
                    .is_some_and(|s| s.starts_with(anchor.after_context.as_str()));
            before_ok && after_ok
        });
    }
    match candidates.len() {
        0 => Err(AnchorError::NotFound),
        1 => Ok((candidates[0], candidates[0] + needle.len())),
        n => Err(AnchorError::Ambiguous(n)),
    }
}

// ------------------- Markdown 结构校验 -------------------

/// 代码围栏 span（字节范围，含围栏行本身，半开区间 `[start, end)`）。
/// 行扫描：≤3 个前导空格后以 ``` 或 ~~~ 开头的行切换开/闭；未闭合围栏延伸到文末。
pub fn fence_spans(body: &str) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<(usize, u8)> = None; // (span 起点, 围栏字符)
    let mut offset = 0usize;
    for line in body.split_inclusive('\n') {
        let line_end = offset + line.len();
        let trimmed = line.trim_start_matches(' ');
        let leading_spaces = line.len() - trimmed.len();
        if leading_spaces <= 3 {
            let fence_char = if trimmed.starts_with("```") {
                b'`'
            } else if trimmed.starts_with("~~~") {
                b'~'
            } else {
                0
            };
            if fence_char != 0 {
                match open {
                    None => open = Some((offset, fence_char)),
                    Some((start, ch)) if ch == fence_char => {
                        spans.push((start, line_end));
                        open = None;
                    }
                    Some(_) => {} // 异种标记在围栏内是普通文本
                }
            }
        }
        offset = line_end;
    }
    if let Some((start, _)) = open {
        spans.push((start, body.len())); // 未闭合 → 延伸到文末
    }
    spans
}

/// 范围是否跨越围栏内外边界（与某 span 相交但未被其完整包含 = 跨越）。
/// 边界恰好落在范围端点上不算跨越（选区贴着围栏起止是安全的）。
pub fn range_crosses_fence(spans: &[(usize, usize)], start: usize, end: usize) -> bool {
    spans.iter().any(|&(s, e)| {
        let intersects = s < end && e > start;
        let contains = s <= start && end <= e;
        intersects && !contains
    })
}

/// 范围完整落在某个围栏 span 内（代码上下文中的替换走更严规则）
fn range_inside_fence(spans: &[(usize, usize)], start: usize, end: usize) -> bool {
    spans.iter().any(|&(s, e)| s <= start && end <= e)
}

/// 文本自身围栏标记是否配对（扫描结束时无悬挂的开启围栏）
pub fn fences_balanced(text: &str) -> bool {
    let mut open: Option<u8> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        let ch = if trimmed.starts_with("```") {
            b'`'
        } else if trimmed.starts_with("~~~") {
            b'~'
        } else {
            0
        };
        if ch == 0 {
            continue;
        }
        match open {
            None => open = Some(ch),
            Some(c) if c == ch => open = None,
            Some(_) => {}
        }
    }
    open.is_none()
}

/// 结构校验统一入口：返回 Err(原因) 表示替换会破坏文档结构。
pub fn validate_patch_structure(
    body: &str,
    range: (usize, usize),
    replacement: &str,
) -> Result<(), String> {
    let (start, end) = range;
    if start > end || end > body.len() {
        return Err("锚点范围越界".to_string());
    }
    let spans = fence_spans(body);
    if range_crosses_fence(&spans, start, end) {
        return Err("选区跨越代码围栏边界，替换会劈开代码块——请重新圈选".to_string());
    }
    if range_inside_fence(&spans, start, end) {
        // 代码上下文内的替换必须是「纯」的：自身围栏配对且不含任何围栏标记
        if !fences_balanced(replacement)
            || replacement.contains("```")
            || replacement.contains("~~~")
        {
            return Err("代码块内的替换不得包含围栏标记（保持代码上下文完整）".to_string());
        }
    } else if !fences_balanced(replacement) {
        return Err("替换内容的代码围栏不成对，会破坏文档结构".to_string());
    }
    // 伪 frontmatter 防护：正文开头被替换出 `---` 块会干扰 .md 的 frontmatter 剥除
    if start == 0 && (replacement == "---" || replacement.starts_with("---\n")) {
        return Err("替换不得在正文开头注入 frontmatter 分隔符".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(text: &str, hash: &str, before: &str, after: &str) -> TextAnchor {
        TextAnchor {
            selected_text: text.to_string(),
            selected_text_hash: hash.to_string(),
            before_context: before.to_string(),
            after_context: after.to_string(),
        }
    }

    // ---- resolve_anchor ----

    #[test]
    fn resolve_unique_match() {
        let body = "第一段文字。\n第二段文字。";
        let (s, e) = resolve_anchor(body, &anchor("第二段文字。", "", "", "")).unwrap();
        assert_eq!(&body[s..e], "第二段文字。");
    }

    #[test]
    fn resolve_disambiguates_with_context() {
        // 「重复句。」出现两次：靠后文消歧命中第二处
        let body = "重复句。\n甲段收尾。\n重复句。\n乙段收尾。";
        let hit = resolve_anchor(body, &anchor("重复句。", "", "", "\n乙段收尾。")).unwrap();
        assert_eq!(&body[hit.0..hit.1], "重复句。");
        assert!(body[hit.1..].starts_with("\n乙段收尾。"));
        // 靠前文消歧命中第一处
        let hit = resolve_anchor(body, &anchor("重复句。", "", "", "\n甲段收尾。")).unwrap();
        assert!(body[hit.1..].starts_with("\n甲段收尾。"));
    }

    #[test]
    fn resolve_zero_multi_and_empty_are_conflicts() {
        let body = "重复句。\n重复句。";
        // 0 匹配
        assert_eq!(
            resolve_anchor(body, &anchor("不存在", "", "", "")).unwrap_err(),
            AnchorError::NotFound
        );
        // 多候选且无上下文可消歧 → Ambiguous，不猜测
        assert_eq!(
            resolve_anchor(body, &anchor("重复句。", "", "", "")).unwrap_err(),
            AnchorError::Ambiguous(2)
        );
        // 上下文过滤后 0 幸存（目标已变化）→ NotFound
        assert_eq!(
            resolve_anchor(body, &anchor("重复句。", "", "", "\n不存在的后文")).unwrap_err(),
            AnchorError::NotFound
        );
        // 空锚点
        assert_eq!(
            resolve_anchor(body, &anchor("", "", "", "")).unwrap_err(),
            AnchorError::Empty
        );
    }

    #[test]
    fn resolve_hash_mismatch_is_conflict() {
        let body = "目标文本在这里。";
        assert_eq!(
            resolve_anchor(
                body,
                &anchor("目标文本在这里。", "0000000000000000", "", "")
            )
            .unwrap_err(),
            AnchorError::HashMismatch
        );
        // 正确 hash 通过（与 repository::content_hash 同口径）
        let hash = repository::content_hash("目标文本在这里。");
        assert!(resolve_anchor(body, &anchor("目标文本在这里。", &hash, "", "")).is_ok());
    }

    // ---- 结构校验 ----

    #[test]
    fn fence_spans_detect_code_blocks() {
        let body = "前言\n```rust\nlet a = 1;\n```\n后记";
        let spans = fence_spans(body);
        assert_eq!(spans.len(), 1);
        assert_eq!(&body[spans[0].0..spans[0].1], "```rust\nlet a = 1;\n```\n");
        // 未闭合 → 延伸到文末（"前言\n" = 2×3+1 = 7 字节，围栏行从 7 开始；全文 7+3+1+4 = 15 字节）
        let spans = fence_spans("前言\n```\ncode");
        assert_eq!(spans, vec![(7, 15)]);
    }

    #[test]
    fn patch_structure_guards() {
        let body = "前言文字。\n```python\nprint(1)\n```\n后记文字。";
        let code_start = body.find("print").unwrap();
        // 围栏内纯文本替换：允许
        let range = (code_start, code_start + "print(1)".len());
        assert!(validate_patch_structure(body, range, "print(2)").is_ok());
        // 围栏内引入围栏标记：拒绝
        assert!(validate_patch_structure(body, range, "```\n注入").is_err());
        // 跨越围栏边界的选区：拒绝
        let cross_start = body.find("前言文字。").unwrap();
        let cross_end = code_start + 3;
        assert!(validate_patch_structure(body, (cross_start, cross_end), "x").is_err());
        // 围栏外替换自身围栏不成对：拒绝
        let plain_start = body.find("后记文字。").unwrap();
        let range = (plain_start, plain_start + "后记文字。".len());
        assert!(validate_patch_structure(body, range, "```\n悬空围栏").is_err());
        assert!(validate_patch_structure(body, range, "```\n成对围栏\n```").is_ok());
        // 正文开头注入伪 frontmatter：拒绝
        assert!(validate_patch_structure(body, (0, 3), "---\nid: x").is_err());
    }

    #[test]
    fn fences_balanced_pairs() {
        assert!(fences_balanced("无围栏"));
        assert!(fences_balanced("```\ncode\n```"));
        assert!(!fences_balanced("```\n悬空"));
        assert!(fences_balanced("~~~\ncode\n~~~"));
        // 异种标记不互相闭合
        assert!(!fences_balanced("```\ncode\n~~~"));
    }
}
