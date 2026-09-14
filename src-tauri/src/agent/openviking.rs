//! OpenViking 配置投影：Hermes 拥有非密钥配置，Host 拥有凭据与网络探测。
use std::time::Duration;

use reqwest::{header::HeaderMap, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::commands::ApiResponse;

pub const KEYCHAIN_PROVIDER: &str = "openviking-memory";
const CONFIG_PATH: &str = "/api/memory/providers/openviking/config";
const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:1933";
const RESPONSE_LIMIT: usize = 64 * 1024;
static SAVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenVikingConfig {
    pub endpoint: String,
    pub account: String,
    pub user: String,
    pub agent: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenVikingSaveRequest {
    pub config: OpenVikingConfig,
    /// 空值保留已有 Key；从不回传或转发给 Hermes 配置 API。
    pub api_key: Option<String>,
    pub consent: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenVikingTestRequest {
    pub config: OpenVikingConfig,
    pub api_key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenVikingStatus {
    pub config: OpenVikingConfig,
    pub active_provider: String,
    pub available: bool,
    pub key_configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenVikingSaveResult {
    pub credential_storage: Option<String>,
    pub restart_required: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenVikingProbe {
    pub version: String,
}

fn normalized_config(mut config: OpenVikingConfig) -> Result<OpenVikingConfig, String> {
    let url = Url::parse(config.endpoint.trim())
        .map_err(|_| "请输入有效的 OpenViking HTTP(S) 服务地址")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("服务地址仅支持 HTTP(S)，不能包含用户名、密码、查询参数或片段".into());
    }
    if let Some(host) = url.host_str() {
        let ip = host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .ok();
        if host.eq_ignore_ascii_case("metadata.google.internal")
            || ip.is_some_and(|ip| match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_unspecified() || ip.is_link_local() || ip.is_multicast()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_unspecified() || ip.is_unicast_link_local() || ip.is_multicast()
                }
            })
        {
            return Err("此地址不能用作 OpenViking 服务".into());
        }
    }
    config.endpoint = url.as_str().trim_end_matches('/').to_string();
    for value in [&mut config.account, &mut config.user, &mut config.agent] {
        *value = value.trim().to_string();
        if value.len() > 256 || value.chars().any(char::is_control) || !value.is_ascii() {
            return Err(
                "账户、用户和 Agent 标识需为不超过 256 字节的 ASCII 文本，不能含控制字符".into(),
            );
        }
    }
    if config.agent.is_empty() {
        config.agent = "hermes".into();
    }
    Ok(config)
}

fn normalized_key(key: Option<String>) -> Result<Option<String>, String> {
    let key = key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    if key
        .as_ref()
        .is_some_and(|key| key.len() > 8192 || !key.is_ascii() || key.chars().any(char::is_control))
    {
        return Err("API Key 格式无效".into());
    }
    Ok(key)
}

/// 拒绝优先级高于 config.yaml 的旧配置，避免保存成功却连接另一服务。
fn check_managed_home() -> Result<(), String> {
    if super::hermes::bundled_runtime::should_use_external_debug_gateway() {
        return Err(
            "OpenViking 设置仅支持 SophoNote 管理的 Hermes Sidecar，请先关闭外部 Gateway 附着"
                .into(),
        );
    }
    let home = super::hermes::bridge_mount::hermes_home()
        .ok_or("Hermes Sidecar 尚未连接，请先启动或重连 Hermes")?;
    let raw = std::fs::read_to_string(home.join("config.yaml"))
        .map_err(|_| "无法读取 Hermes 私有配置")?;
    let config: Value = serde_yaml::from_str(&raw).map_err(|_| "Hermes 私有配置格式无效")?;
    if config
        .pointer("/memory/openviking/use_ovcli_config")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err("Hermes 正在链接 ovcli 配置，请先在 Hermes 中解除链接后使用此设置页".into());
    }
    match std::fs::read_to_string(home.join(".env")) {
        Ok(env) if env.lines().any(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim()).trim();
            line.split_once('=').is_some_and(|(key, _)| matches!(key.trim(),
                "OPENVIKING_ENDPOINT" | "OPENVIKING_API_KEY" | "OPENVIKING_ACCOUNT" | "OPENVIKING_USER" | "OPENVIKING_AGENT" | "OPENVIKING_CLI_CONFIG_FILE"))
        }) => return Err("Hermes 私有 .env 中存在 OpenViking 覆盖项，请先迁移或移除这些覆盖项，再使用此设置页".into()),
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(_) => return Err("无法检查 Hermes 私有环境配置".into()),
    }
    Ok(())
}

async fn dashboard(method: Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    super::commands::hermes_dashboard_request(method, path, body, Duration::from_secs(15))
        .await
        .map_err(|error| {
            // 上游异常可能包含原始配置。只保留 HTTP 状态，不回传任意正文。
            let status = ["400", "401", "403", "404", "409", "422", "500", "503"]
                .into_iter()
                .find(|status| error.contains(status));
            match status {
                Some("404") => {
                    "当前 Hermes 不支持 OpenViking 配置接口，请在 Hermes 更新页升级".into()
                }
                Some(status) => {
                    format!("Hermes OpenViking 配置请求失败（HTTP {status}），请检查 Runtime 状态")
                }
                None => "无法连接 Hermes OpenViking 配置接口，请重连后重试".into(),
            }
        })
}

fn project_status(memory: &Value, payload: &Value, key_configured: bool) -> OpenVikingStatus {
    let field = |key: &str, fallback: &str| {
        payload
            .get("fields")
            .and_then(Value::as_array)
            .and_then(|fields| {
                fields
                    .iter()
                    .find(|field| field.get("key").and_then(Value::as_str) == Some(key))
            })
            .and_then(|field| field.get("value"))
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string()
    };
    OpenVikingStatus {
        config: OpenVikingConfig {
            endpoint: field("endpoint", DEFAULT_ENDPOINT),
            account: field("account", ""),
            user: field("user", ""),
            agent: field("agent", "hermes"),
        },
        active_provider: memory
            .get("active")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        available: payload
            .get("fields")
            .and_then(Value::as_array)
            .is_some_and(|fields| {
                fields
                    .iter()
                    .any(|f| f.get("key").and_then(Value::as_str) == Some("endpoint"))
            }),
        key_configured,
    }
}

#[tauri::command]
pub async fn agent_openviking_status(app: AppHandle) -> ApiResponse<OpenVikingStatus> {
    let result = async {
        check_managed_home()?;
        let (memory, config) = tokio::try_join!(
            dashboard(Method::GET, "/api/memory", None),
            dashboard(Method::GET, CONFIG_PATH, None)
        )?;
        let key = crate::commands::get_cached_api_key(&app, KEYCHAIN_PROVIDER)?;
        Ok(project_status(&memory, &config, !key.is_empty()))
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_openviking_save(
    app: AppHandle,
    request: OpenVikingSaveRequest,
) -> ApiResponse<OpenVikingSaveResult> {
    let _lock = SAVE_LOCK.lock().await;
    let result = async {
        if !request.consent {
            return Err("请先确认启用后的会话同步与共享记忆行为".into());
        }
        let config = normalized_config(request.config)?;
        let key = normalized_key(request.api_key)?;
        check_managed_home()?;
        let schema = dashboard(Method::GET, CONFIG_PATH, None).await?;
        if !project_status(&json!({}), &schema, false).available {
            return Err("当前 Hermes 未提供 OpenViking 插件，请先更新 Hermes Sidecar".into());
        }
        let mut credential_storage = None;
        if let Some(key) = key {
            let saved =
                crate::commands::keychain_save_api_key(app.clone(), KEYCHAIN_PROVIDER.into(), key)
                    .await;
            if !saved.success {
                return Err(saved.error.unwrap_or("凭据保存失败".into()));
            }
            credential_storage = saved.data;
        }
        dashboard(Method::PUT, CONFIG_PATH, Some(json!({"values": config})))
            .await
            .map_err(|error| {
                if credential_storage.is_some() {
                    format!("凭据已保存；连接配置未保存：{error}")
                } else {
                    error
                }
            })?;
        Ok(OpenVikingSaveResult {
            credential_storage,
            restart_required: true,
        })
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[tauri::command]
pub async fn agent_openviking_disable() -> ApiResponse<OpenVikingSaveResult> {
    let _lock = SAVE_LOCK.lock().await;
    let result = async {
        check_managed_home()?;
        let status = dashboard(Method::GET, "/api/memory", None).await?;
        // 不覆盖用户在 Hermes 中选择的其它 Memory Provider。
        let changed = status.get("active").and_then(Value::as_str) == Some("openviking");
        if changed {
            dashboard(
                Method::PUT,
                "/api/memory/provider",
                Some(json!({"provider": ""})),
            )
            .await?;
        }
        Ok(OpenVikingSaveResult {
            credential_storage: None,
            restart_required: changed,
        })
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

fn auth_headers(config: &OpenVikingConfig, key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let mut insert = |name: &'static str, value: &str| -> Result<(), String> {
        headers.insert(
            name,
            value.parse().map_err(|_| "OpenViking 鉴权字段格式无效")?,
        );
        Ok(())
    };
    if !key.is_empty() {
        insert("x-api-key", key)?;
        insert("authorization", &format!("Bearer {key}"))?;
    } else {
        insert(
            "x-openviking-account",
            if config.account.is_empty() {
                "default"
            } else {
                &config.account
            },
        )?;
        insert(
            "x-openviking-user",
            if config.user.is_empty() {
                "default"
            } else {
                &config.user
            },
        )?;
    }
    insert("x-openviking-actor-peer", &config.agent)?;
    Ok(headers)
}

async fn response_json(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "OpenViking 连接超时，请检查服务地址与网络"
        } else {
            "无法连接 OpenViking，请检查服务是否已启动及网络配置"
        }
        .to_string()
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "OpenViking 请求失败（HTTP {}）{}",
            status.as_u16(),
            if matches!(status.as_u16(), 401 | 403) {
                "，请检查 API Key 与账户权限"
            } else {
                ""
            }
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "读取 OpenViking 响应失败")?
    {
        if bytes.len() + chunk.len() > RESPONSE_LIMIT {
            return Err("OpenViking 响应超过大小限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "OpenViking 返回了无效的 JSON 响应".into())
}

async fn probe(config: &OpenVikingConfig, key: &str) -> Result<OpenVikingProbe, String> {
    let url = Url::parse(&config.endpoint).map_err(|_| "服务地址无效")?;
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8));
    if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|_| "无法创建 OpenViking 连接")?;
    // 在服务身份确认前绝不发送 Key。
    let health = response_json(client.get(format!("{}/health", config.endpoint))).await?;
    let version = health
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty());
    if health.get("status").and_then(Value::as_str) != Some("ok")
        || health.get("healthy").and_then(Value::as_bool) != Some(true)
        || version.is_none()
    {
        return Err(
            "服务未返回健康的 OpenViking 响应，请确认地址并使用 OpenViking 0.2.10 或更新版本"
                .into(),
        );
    }
    let status = response_json(
        client
            .get(format!("{}/api/v1/system/status", config.endpoint))
            .headers(auth_headers(config, key)?),
    )
    .await?;
    if status.get("status").and_then(Value::as_str) != Some("ok")
        || status
            .pointer("/result/initialized")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("OpenViking 已响应，但服务尚未完成初始化".into());
    }
    Ok(OpenVikingProbe {
        version: version.unwrap_or_default().chars().take(80).collect(),
    })
}

#[tauri::command]
pub async fn agent_openviking_test(
    app: AppHandle,
    request: OpenVikingTestRequest,
) -> ApiResponse<OpenVikingProbe> {
    let result = async {
        let config = normalized_config(request.config)?;
        let key = match normalized_key(request.api_key)? {
            Some(key) => key,
            None => crate::commands::get_cached_api_key(&app, KEYCHAIN_PROVIDER)?,
        };
        probe(&config, &key).await
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config(endpoint: &str) -> OpenVikingConfig {
        OpenVikingConfig {
            endpoint: endpoint.into(),
            account: "".into(),
            user: "".into(),
            agent: "hermes".into(),
        }
    }

    #[test]
    fn validates_addresses_without_echoing_credentials() {
        for endpoint in [
            "file:///etc/passwd",
            "https://user:secret@example.com",
            "http://localhost?token=secret",
            "http://169.254.169.254",
            "http://[::]",
        ] {
            let error = normalized_config(config(endpoint)).err().unwrap();
            assert!(!error.contains("secret"));
        }
        assert_eq!(
            normalized_config(config(" https://example.com/viking/ "))
                .unwrap()
                .endpoint,
            "https://example.com/viking"
        );
        assert_eq!(
            normalized_config(config("http://[::1]:1933"))
                .unwrap()
                .endpoint,
            "http://[::1]:1933"
        );
        assert!(normalized_key(Some("a\nb".into())).is_err());
        assert!(normalized_key(Some("  ".into())).unwrap().is_none());
    }

    #[test]
    fn status_never_projects_secret_fields_or_unknown_config() {
        let status = project_status(
            &json!({"active":"openviking"}),
            &json!({"fields":[
                {"key":"endpoint","value":"http://localhost:1933"},
                {"key":"api_key","value":"never-expose-this"},
                {"key":"future_secret","value":"never-expose-this"}
            ]}),
            true,
        );
        let text = serde_json::to_string(&status).unwrap();
        assert!(!text.contains("never-expose"));
        assert!(status.key_configured && status.available);
        assert_eq!(status.config.agent, "hermes");
    }

    #[test]
    fn headers_match_hermes_key_and_local_identity_modes() {
        let headers = auth_headers(&config(DEFAULT_ENDPOINT), "test-key").unwrap();
        assert_eq!(headers["x-api-key"], "test-key");
        assert_eq!(headers["authorization"], "Bearer test-key");
        assert_eq!(headers["x-openviking-actor-peer"], "hermes");
        assert!(!headers.contains_key("x-openviking-user"));
        let headers = auth_headers(&config(DEFAULT_ENDPOINT), "").unwrap();
        assert!(!headers.contains_key("x-api-key"));
        assert_eq!(headers["x-openviking-account"], "default");
        assert_eq!(headers["x-openviking-user"], "default");
    }

    async fn fixture(
        responses: Vec<(u16, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let mut requests = vec![];
            for (code, body) in responses {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(3), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut data = vec![];
                loop {
                    let mut buffer = [0; 1024];
                    let size = stream.read(&mut buffer).await.unwrap();
                    data.extend_from_slice(&buffer[..size]);
                    if size == 0 || data.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8_lossy(&data).to_lowercase());
                let response = format!("HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nLocation: http://127.0.0.1:1/leak\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes()).await;
            }
            requests
        });
        (endpoint, handle)
    }

    fn health() -> String {
        json!({"status":"ok","healthy":true,"version":"0.2.10"}).to_string()
    }

    #[tokio::test]
    async fn probe_identifies_server_before_sending_key_and_preserves_base_path() {
        let (endpoint, requests) = fixture(vec![
            (200, health()),
            (
                200,
                json!({"status":"ok","result":{"initialized":true}}).to_string(),
            ),
        ])
        .await;
        assert_eq!(
            probe(&config(&format!("{endpoint}/viking")), "fixture-key")
                .await
                .unwrap()
                .version,
            "0.2.10"
        );
        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("get /viking/health "));
        assert!(!requests[0].contains("fixture-key"));
        assert!(requests[1].starts_with("get /viking/api/v1/system/status "));
        assert!(requests[1].contains("x-api-key: fixture-key"));
    }

    #[tokio::test]
    async fn probe_rejects_false_health_redirects_and_oversize_without_leaking_key() {
        for response in [
            (200, "{\"status\":\"ok\"}".into()),
            (302, "redirect".into()),
            (200, "x".repeat(RESPONSE_LIMIT + 1)),
        ] {
            let (endpoint, requests) = fixture(vec![response]).await;
            assert!(probe(&config(&endpoint), "fixture-key").await.is_err());
            assert!(!requests.await.unwrap()[0].contains("fixture-key"));
        }
    }

    #[tokio::test]
    async fn probe_reports_auth_failure_without_echoing_response() {
        let (endpoint, requests) = fixture(vec![
            (200, health()),
            (401, "server echoed fixture-key".into()),
        ])
        .await;
        let error = probe(&config(&endpoint), "fixture-key")
            .await
            .err()
            .unwrap();
        assert!(error.contains("401"));
        assert!(!error.contains("fixture-key"));
        assert_eq!(requests.await.unwrap().len(), 2);
    }
}
