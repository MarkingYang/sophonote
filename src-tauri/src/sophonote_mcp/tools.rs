//! Bridge 工具调用（H5 授权；H6 起可接 ToolRegistry 执行 dry-run 工具）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInvokeRequest {
    pub lease_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub arguments: Value,
    /// 模型可能夹带；Bridge **不信任**，仅用于越权检测
    #[serde(default)]
    pub claimed_project_id: Option<String>,
    #[serde(default)]
    pub claimed_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeInvokeResult {
    pub ok: bool,
    pub tool_name: String,
    /// 授权通过后回显 Lease 绑定的 projectId（不以模型声称值为准）
    pub project_id: String,
    pub run_id: String,
    pub model_provider_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub output_text: String,
    /// H6：dry-run 结构化结果（如 PatchPreview JSON）；无执行时为 Null
    #[serde(default)]
    pub structured: Value,
}
