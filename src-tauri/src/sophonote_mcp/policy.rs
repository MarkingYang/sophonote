//! Lease 策略：工具白名单 + 不信任模型传来的 projectId/runId。

use super::lease::{LeaseError, SidecarLease};

/// 授权一次工具调用：必须以 Lease 为准，忽略/校验模型侧声称的 id。
pub fn authorize_tool(
    lease: &SidecarLease,
    tool_name: &str,
    claimed_project_id: Option<&str>,
    claimed_run_id: Option<&str>,
) -> Result<(), LeaseError> {
    if !lease.allowed_tools.contains(tool_name) {
        return Err(LeaseError::ToolNotAllowed(tool_name.to_string()));
    }
    if let Some(claimed) = claimed_project_id {
        if claimed != lease.project_id {
            return Err(LeaseError::ProjectMismatch {
                expected: lease.project_id.clone(),
                claimed: claimed.to_string(),
            });
        }
    }
    if let Some(claimed) = claimed_run_id {
        if claimed != lease.run_id {
            return Err(LeaseError::RunMismatch {
                expected: lease.run_id.clone(),
                claimed: claimed.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sophonote_mcp::lease::{issue_lease, ModelRoute};

    fn lease() -> SidecarLease {
        issue_lease(
            "run-1",
            "proj-1",
            ["list_project_documents", "read_document"],
            ModelRoute::deepseek_default(),
            60_000,
        )
    }

    #[test]
    fn rejects_tool_outside_allowlist() {
        let err = authorize_tool(&lease(), "create_document", None, None).unwrap_err();
        assert!(matches!(err, LeaseError::ToolNotAllowed(_)));
    }

    #[test]
    fn rejects_spoofed_project_id() {
        let err = authorize_tool(&lease(), "read_document", Some("other"), None).unwrap_err();
        assert!(matches!(err, LeaseError::ProjectMismatch { .. }));
    }

    #[test]
    fn accepts_matching_claims() {
        assert!(authorize_tool(
            &lease(),
            "list_project_documents",
            Some("proj-1"),
            Some("run-1"),
        )
        .is_ok());
    }
}
