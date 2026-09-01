//! sophonote-bridge 门面：校验 Lease；H6 起可经 ToolRegistry 执行（如 propose_document_patch）。

use std::sync::Arc;

use crate::model::messages::ModelToolCall;
use crate::tools::ToolRegistry;

use super::lease::{LeaseError, LeaseRegistry, SidecarLease};
use super::policy::authorize_tool;
use super::tools::{BridgeInvokeRequest, BridgeInvokeResult};
use super::BRIDGE_MCP_NAME;

/// H6：经 Bridge 允许的文档提案工具（仍 dry-run，落盘走 DocumentService）
pub const BRIDGE_PATCH_TOOL: &str = "propose_document_patch";

#[derive(Debug, Clone)]
pub struct SophonoteBridge {
    pub registry: LeaseRegistry,
}

impl SophonoteBridge {
    pub fn new(registry: LeaseRegistry) -> Self {
        Self { registry }
    }

    pub fn name(&self) -> &'static str {
        BRIDGE_MCP_NAME
    }

    pub fn register_lease(&self, lease: SidecarLease) {
        self.registry.insert(lease);
    }

    pub fn revoke_lease(&self, lease_id: &str) -> bool {
        self.registry.revoke(lease_id)
    }

    /// 仅授权（H5）；不执行工具。
    pub fn invoke(&self, req: BridgeInvokeRequest) -> Result<BridgeInvokeResult, LeaseError> {
        let lease = self.authorize(&req)?;
        Ok(BridgeInvokeResult {
            ok: true,
            tool_name: req.tool_name,
            project_id: lease.project_id,
            run_id: lease.run_id,
            model_provider_id: lease.model_route.provider_id,
            model: lease.model_route.model,
            error: None,
            output_text: format!(
                "authorized via {BRIDGE_MCP_NAME}; execution deferred to ToolRegistry"
            ),
            structured: serde_json::Value::Null,
        })
    }

    /// H6：授权后经 ToolRegistry 执行。工具内已绑定 Lease 同源 project/run，
    /// 模型声称的 projectId/runId 只做越权检测，不传入执行。
    pub async fn invoke_with_tools(
        &self,
        req: BridgeInvokeRequest,
        tools: &ToolRegistry,
    ) -> Result<BridgeInvokeResult, LeaseError> {
        let lease = self.authorize(&req)?;
        let call = ModelToolCall {
            id: format!("bridge-{}", req.tool_name),
            name: req.tool_name.clone(),
            arguments: req.arguments,
        };
        match tools.execute(&call).await {
            Ok(out) => Ok(BridgeInvokeResult {
                ok: true,
                tool_name: req.tool_name,
                project_id: lease.project_id,
                run_id: lease.run_id,
                model_provider_id: lease.model_route.provider_id,
                model: lease.model_route.model,
                error: None,
                output_text: out.model_text,
                structured: out.structured,
            }),
            Err(e) => Ok(BridgeInvokeResult {
                ok: false,
                tool_name: req.tool_name,
                project_id: lease.project_id,
                run_id: lease.run_id,
                model_provider_id: lease.model_route.provider_id,
                model: lease.model_route.model,
                error: Some(e.to_string()),
                output_text: String::new(),
                structured: serde_json::Value::Null,
            }),
        }
    }

    fn authorize(&self, req: &BridgeInvokeRequest) -> Result<SidecarLease, LeaseError> {
        let lease = self.registry.require_active(&req.lease_id)?;
        authorize_tool(
            &lease,
            &req.tool_name,
            req.claimed_project_id.as_deref(),
            req.claimed_run_id.as_deref(),
        )?;
        Ok(lease)
    }
}

/// 构造仅含 Bridge 可调用工具的 registry（测试/Hermes 路径）
pub fn bridge_patch_registry(
    db_path: std::path::PathBuf,
    notes_dir: std::path::PathBuf,
    project_id: &str,
    run_id: &str,
) -> ToolRegistry {
    use crate::tools::documents::{ProposeDocumentPatchTool, RenameArticleTool};
    use crate::tools::project::{ListProjectDocumentsTool, ReadDocumentTool};

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ListProjectDocumentsTool::new(
        db_path.clone(),
        project_id.into(),
    )));
    reg.register(Arc::new(ReadDocumentTool::new(
        db_path.clone(),
        notes_dir.clone(),
        project_id.into(),
    )));
    reg.register(Arc::new(ProposeDocumentPatchTool::new(
        db_path.clone(),
        notes_dir.clone(),
        project_id.into(),
        run_id.into(),
    )));
    reg.register(Arc::new(RenameArticleTool::new(
        db_path.clone(),
        notes_dir,
        project_id.into(),
        run_id.into(),
    )));
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sophonote_mcp::lease::{issue_lease, ModelRoute};

    #[test]
    fn bridge_name_is_sophonote_bridge() {
        let b = SophonoteBridge::new(LeaseRegistry::new());
        assert_eq!(b.name(), "sophonote-bridge");
    }

    #[test]
    fn invoke_uses_lease_project_not_claimed() {
        let b = SophonoteBridge::new(LeaseRegistry::new());
        let lease = issue_lease(
            "r1",
            "p-real",
            ["list_project_documents"],
            ModelRoute::deepseek_default(),
            60_000,
        );
        let id = lease.lease_id.clone();
        b.register_lease(lease);
        let res = b
            .invoke(BridgeInvokeRequest {
                lease_id: id,
                tool_name: "list_project_documents".into(),
                arguments: serde_json::json!({}),
                claimed_project_id: Some("p-real".into()),
                claimed_run_id: Some("r1".into()),
            })
            .unwrap();
        assert_eq!(res.project_id, "p-real");
        assert_eq!(res.model_provider_id, "deepseek");
        assert!(res.structured.is_null());
    }
}
