//! SidecarLease：短租约 + modelRoute（DEC-012：路由来自 SophoNote settings，不含 API Key）。

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::openai_compat::ProviderSnapshot;

/// Host 注入的模型路由（无 Key；Key 不进 Lease / 不进 sidecar 持久化）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoute {
    pub provider_id: String,
    pub base_url: String,
    pub model: String,
}

impl ModelRoute {
    pub fn from_provider_snapshot(snapshot: &ProviderSnapshot) -> Self {
        Self {
            provider_id: snapshot.id.clone(),
            base_url: snapshot.base_url.clone(),
            model: snapshot.model.clone(),
        }
    }

    /// 测试/无 AppHandle 时的 DeepSeek 默认（与 openai_compat 默认对齐）
    pub fn deepseek_default() -> Self {
        Self {
            provider_id: "deepseek".into(),
            base_url: "https://api.deepseek.com".into(),
            model: "deepseek-v4-pro".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarLease {
    pub lease_id: String,
    pub run_id: String,
    pub project_id: String,
    pub allowed_tools: BTreeSet<String>,
    pub model_route: ModelRoute,
    /// Unix 毫秒；过期后 Bridge 拒绝
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseError {
    NotFound,
    Expired,
    ToolNotAllowed(String),
    ProjectMismatch { expected: String, claimed: String },
    RunMismatch { expected: String, claimed: String },
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "lease 不存在或已撤销"),
            Self::Expired => write!(f, "lease 已过期"),
            Self::ToolNotAllowed(t) => write!(f, "工具不在 lease 白名单: {t}"),
            Self::ProjectMismatch { expected, claimed } => {
                write!(f, "projectId 越权：lease={expected} claimed={claimed}")
            }
            Self::RunMismatch { expected, claimed } => {
                write!(f, "runId 越权：lease={expected} claimed={claimed}")
            }
        }
    }
}

impl std::error::Error for LeaseError {}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 签发短租约（TTL 默认 15 分钟）
pub fn issue_lease(
    run_id: impl Into<String>,
    project_id: impl Into<String>,
    allowed_tools: impl IntoIterator<Item = impl Into<String>>,
    model_route: ModelRoute,
    ttl_ms: u64,
) -> SidecarLease {
    let ttl = if ttl_ms == 0 { 15 * 60 * 1000 } else { ttl_ms };
    SidecarLease {
        lease_id: format!("lease-{}", Uuid::new_v4().simple()),
        run_id: run_id.into(),
        project_id: project_id.into(),
        allowed_tools: allowed_tools.into_iter().map(|s| s.into()).collect(),
        model_route,
        expires_at_ms: now_ms().saturating_add(ttl),
    }
}

/// 进程内 Lease 注册表（H5；真实 Bridge 可同口径）
#[derive(Debug, Default, Clone)]
pub struct LeaseRegistry {
    inner: Arc<Mutex<HashMap<String, SidecarLease>>>,
}

impl LeaseRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, lease: SidecarLease) {
        let id = lease.lease_id.clone();
        self.inner.lock().unwrap().insert(id, lease);
    }

    pub fn revoke(&self, lease_id: &str) -> bool {
        self.inner.lock().unwrap().remove(lease_id).is_some()
    }

    pub fn get(&self, lease_id: &str) -> Option<SidecarLease> {
        self.inner.lock().unwrap().get(lease_id).cloned()
    }

    /// 取活租约；过期则删除并返回 Expired
    pub fn require_active(&self, lease_id: &str) -> Result<SidecarLease, LeaseError> {
        let mut map = self.inner.lock().unwrap();
        let Some(lease) = map.get(lease_id).cloned() else {
            return Err(LeaseError::NotFound);
        };
        if now_ms() >= lease.expires_at_ms {
            map.remove(lease_id);
            return Err(LeaseError::Expired);
        }
        Ok(lease)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_route_has_no_api_key_field() {
        let json = serde_json::to_value(ModelRoute::deepseek_default()).unwrap();
        assert!(json.get("apiKey").is_none());
        assert!(json.get("api_key").is_none());
        assert_eq!(json["providerId"], "deepseek");
    }

    #[test]
    fn expired_lease_rejected() {
        let reg = LeaseRegistry::new();
        let mut lease = issue_lease(
            "r1",
            "p1",
            ["list_project_documents"],
            ModelRoute::deepseek_default(),
            1,
        );
        lease.expires_at_ms = 1; // 早已过期
        reg.insert(lease.clone());
        assert!(matches!(
            reg.require_active(&lease.lease_id),
            Err(LeaseError::Expired)
        ));
    }
}
