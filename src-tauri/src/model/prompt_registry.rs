// ============================================================
// Track B · 智能体演进（docs/architecture.md Phase 0）
// PromptRegistry：提示词版本唯一真相源。
// 版本清单存 src-tauri/prompt_versions.json（编译期 include_str! 嵌入），
// 前端经相对路径 import 同一文件——单一文件，零手工同步。
// 规则：改提示词 → 递增此处版本号 → AI 产出入库带版本，回归对比用。
// ============================================================
use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../prompt_versions.json");

static REGISTRY: OnceLock<HashMap<String, String>> = OnceLock::new();

fn registry() -> &'static HashMap<String, String> {
    REGISTRY.get_or_init(|| {
        serde_json::from_str(RAW).expect("prompt_versions.json 必须是合法的字符串字典")
    })
}

/// 取提示词版本号（如 version("enrich") == "enrich@v1"）；未知 key 返回 None
pub fn version(key: &str) -> Option<&'static String> {
    registry().get(key)
}

/// 取指定 key 的版本号，未知 key 时 panic 仅限拼写错误（编译期无法检查，测试兜底）
pub fn expect_version(key: &str) -> &'static String {
    version(key).unwrap_or_else(|| panic!("PromptRegistry 缺少 key: {}", key))
}

/// 全量清单（Tauri 命令/调试用）
pub fn all() -> &'static HashMap<String, String> {
    registry()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前端历史口径（ai.ts PROMPT_VERSIONS）+ scheduler nightly，键值漂移即失败
    #[test]
    fn registry_matches_baseline() {
        assert_eq!(expect_version("enrich"), "enrich@v1");
        assert_eq!(expect_version("deepDive"), "deepdive@v1");
        assert_eq!(expect_version("daily"), "daily@v1");
        assert_eq!(expect_version("weekly"), "weekly@v1");
        assert_eq!(expect_version("summary"), "summary@v1");
        assert_eq!(expect_version("tags"), "tags@v1");
        assert_eq!(expect_version("pick"), "pick@v1");
        assert_eq!(expect_version("manualEdit"), "manual-edit@v1");
        assert_eq!(expect_version("nightly"), "nightly@v1");
        assert_eq!(expect_version("projectAssign"), "project-assign@v1");
        assert_eq!(expect_version("completion"), "completion@v1");
        assert_eq!(expect_version("agentChat"), "agent-chat@v1");
    }
}
