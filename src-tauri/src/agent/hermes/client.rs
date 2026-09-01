//! Hermes API Server health HTTP 客户端（自有错误类型，不泄漏到 Tauri DTO）。

use serde::Deserialize;

/// Client 侧错误
#[derive(Debug)]
pub enum HermesClientError {
    Transport(String),
    Unauthorized,
    UnexpectedStatus(u16, String),
    Parse(String),
}

impl std::fmt::Display for HermesClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(m) => write!(f, "Hermes 网络错误: {m}"),
            Self::Unauthorized => write!(f, "Hermes 未授权（Bearer 无效）"),
            Self::UnexpectedStatus(code, body) => {
                write!(f, "Hermes 非预期状态 {code}: {body}")
            }
            Self::Parse(m) => write!(f, "Hermes 响应解析失败: {m}"),
        }
    }
}

impl std::error::Error for HermesClientError {}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HealthStatus {
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailedHealth {
    pub status: String,
    #[serde(default)]
    pub readiness: Option<serde_json::Value>,
}

impl DetailedHealth {
    pub fn is_ready(&self) -> bool {
        self.status.eq_ignore_ascii_case("ok") || self.status.eq_ignore_ascii_case("ready")
    }
}

#[derive(Debug, Clone)]
pub struct HermesHttpClient {
    base_url: String,
    bearer: String,
    http: reqwest::Client,
}

impl HermesHttpClient {
    pub fn new(base_url: impl Into<String>, bearer: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bearer: bearer.into(),
            http: reqwest::Client::new(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /health`（无认证）
    pub async fn health(&self) -> Result<HealthStatus, HermesClientError> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), body));
        }
        serde_json::from_str(&body).map_err(|e| HermesClientError::Parse(e.to_string()))
    }

    /// `GET /health/detailed`（Bearer 必填）
    pub async fn health_detailed(&self) -> Result<DetailedHealth, HermesClientError> {
        let url = format!("{}/health/detailed", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), body));
        }
        serde_json::from_str(&body).map_err(|e| HermesClientError::Parse(e.to_string()))
    }

    /// 故意使用错误 Bearer，用于契约测试
    pub async fn health_detailed_with_bearer(
        &self,
        bearer: &str,
    ) -> Result<DetailedHealth, HermesClientError> {
        let url = format!("{}/health/detailed", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {bearer}"))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), body));
        }
        serde_json::from_str(&body).map_err(|e| HermesClientError::Parse(e.to_string()))
    }
}
