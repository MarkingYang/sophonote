//! OpenViking 配置投影：Hermes 拥有非密钥配置，Host 拥有凭据与网络探测。
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
mod run_memory;
pub(crate) use run_memory::RunMemory;

use reqwest::{header::HeaderMap, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::commands::ApiResponse;

pub const KEYCHAIN_PROVIDER: &str = "openviking-memory";
const CONFIG_PATH: &str = "/api/memory/providers/openviking/config";
pub const DEFAULT_ENDPOINT: &str = "https://api.vikingdb.cn-beijing.volces.com/openviking";
const MEMORY_ROOT: &str = "viking://~/memories";
const HERMES_MEMORY_ROOT: &str = "viking://~/peers/hermes/memories";
const HOST_ENGINES: [&str; 3] = ["pi", "claude_code", "opencode"];
const MEMORY_ROOTS: [&str; 5] = [
    MEMORY_ROOT,
    HERMES_MEMORY_ROOT,
    "viking://~/peers/pi/memories",
    "viking://~/peers/claude_code/memories",
    "viking://~/peers/opencode/memories",
];
static CONFIG_GENERATION: AtomicU64 = AtomicU64::new(0);
const CONTENT_LIMIT: usize = 256 * 1024;
const PAGE_SIZE: usize = 100;
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
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
    #[serde(default)]
    pub all_engines: bool,
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
    pub host_engines_enabled: bool,
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
    pub engines: Vec<EngineProbe>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineProbe {
    pub engine: String,
    pub success: bool,
    pub message: String,
}

fn cloud_config() -> OpenVikingConfig {
    OpenVikingConfig {
        endpoint: DEFAULT_ENDPOINT.into(),
        account: String::new(),
        user: String::new(),
        agent: "hermes".into(),
    }
}

fn normalized_config(config: OpenVikingConfig) -> Result<OpenVikingConfig, String> {
    if config.endpoint.trim().trim_end_matches('/') != DEFAULT_ENDPOINT
        || !config.account.is_empty()
        || !config.user.is_empty()
        || config.agent != "hermes"
    {
        return Err("记忆仅支持火山 OpenViking 云端服务，请重新读取配置后保存 API Key".into());
    }
    Ok(cloud_config())
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
fn check_managed_home() -> Result<Value, String> {
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
    Ok(config)
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

fn host_engine_enabled(memory: &Value, engine: &str) -> bool {
    HOST_ENGINES.contains(&engine)
        && memory.get("provider").and_then(Value::as_str) == Some("openviking")
        && memory
            .pointer("/openviking/endpoint")
            .and_then(Value::as_str)
            == Some(DEFAULT_ENDPOINT)
        && memory
            .get("sophonote_engines")
            .and_then(Value::as_array)
            .is_some_and(|engines| engines.iter().any(|e| e.as_str() == Some(engine)))
}

fn project_status(memory: &Value, key_configured: bool) -> OpenVikingStatus {
    OpenVikingStatus {
        config: cloud_config(),
        active_provider: if memory.get("provider").and_then(Value::as_str) == Some("openviking")
            && memory
                .pointer("/openviking/endpoint")
                .and_then(Value::as_str)
                == Some(DEFAULT_ENDPOINT)
        {
            "openviking".into()
        } else {
            String::new()
        },
        available: true,
        key_configured,
        host_engines_enabled: HOST_ENGINES
            .iter()
            .all(|engine| host_engine_enabled(memory, engine)),
    }
}

#[tauri::command]
pub async fn agent_openviking_status(app: AppHandle) -> ApiResponse<OpenVikingStatus> {
    let result = async {
        let config = check_managed_home()?;
        let configured = crate::commands::has_api_key(&app, KEYCHAIN_PROVIDER)?;
        Ok(project_status(&config["memory"], configured))
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
        if key.is_none() && !crate::commands::has_api_key(&app, KEYCHAIN_PROVIDER)? {
            return Err("请填写火山 OpenViking API Key".into());
        }
        check_managed_home()?;
        let schema = dashboard(Method::GET, CONFIG_PATH, None).await?;
        if !schema
            .get("fields")
            .and_then(Value::as_array)
            .is_some_and(|fields| {
                fields
                    .iter()
                    .any(|f| f.get("key").and_then(Value::as_str) == Some("endpoint"))
            })
        {
            return Err("当前 Hermes 未提供 OpenViking 插件，请先更新 Hermes Sidecar".into());
        }
        let mut credential_storage = None;
        CONFIG_GENERATION.fetch_add(1, Ordering::SeqCst);
        if let Some(key) = key {
            let saved =
                crate::commands::keychain_save_api_key(app.clone(), KEYCHAIN_PROVIDER.into(), key)
                    .await;
            if !saved.success {
                return Err(saved.error.unwrap_or("凭据保存失败".into()));
            }
            credential_storage = saved.data;
        }
        dashboard(
            Method::PUT,
            "/api/config",
            Some(json!({"config": {"memory": {
                "provider": "openviking", "memory_enabled": false, "user_profile_enabled": false,
                "openviking": config,
                "sophonote_engines": if request.all_engines { HOST_ENGINES.to_vec() } else { vec![] }
            }}})),
        )
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
        let config = check_managed_home()?;
        CONFIG_GENERATION.fetch_add(1, Ordering::SeqCst);
        let provider = config
            .pointer("/memory/provider")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !provider.is_empty() && provider != "openviking" {
            return Err("当前使用其它记忆服务，请先在 Hermes 中停用该服务".into());
        }
        dashboard(
            Method::PUT,
            "/api/config",
            Some(json!({"config": {"memory": {
                "provider": "", "memory_enabled": false, "user_profile_enabled": false, "sophonote_engines": []
            }}})),
        )
        .await?;
        Ok(OpenVikingSaveResult {
            credential_storage: None,
            restart_required: true,
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
    if key.is_empty() {
        return Err("请先保存火山 OpenViking API Key".into());
    }
    insert("x-api-key", key)?;
    insert("authorization", &format!("Bearer {key}"))?;
    insert("x-openviking-actor-peer", &config.agent)?;
    Ok(headers)
}

async fn response_json(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "OpenViking 连接超时，请检查服务地址与网络"
        } else {
            "无法连接火山 OpenViking，请检查网络"
        }
        .to_string()
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "OpenViking 请求失败（HTTP {}）{}",
            status.as_u16(),
            if status.as_u16() == 402 {
                "，云端套餐额度或付费状态不可用，请到火山控制台检查套餐"
            } else if matches!(status.as_u16(), 401 | 403) {
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

fn cloud_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "无法创建 OpenViking 连接".into())
}

fn api_result(value: Value) -> Result<Value, String> {
    if value.get("status").and_then(Value::as_str) != Some("ok") {
        return Err("OpenViking 未确认操作成功，请刷新后重试".into());
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "OpenViking 响应缺少结果".into())
}

async fn probe(config: &OpenVikingConfig, key: &str) -> Result<OpenVikingProbe, String> {
    if key.is_empty() {
        return Err("请填写火山 OpenViking API Key".into());
    }
    api_result(
        response_json(
            cloud_client()?
                .get(format!("{}/api/v1/fs/ls", config.endpoint))
                .headers(auth_headers(config, key)?)
                .query(&[("uri", "viking://~"), ("limit", "1")]),
        )
        .await?,
    )?;
    Ok(OpenVikingProbe {
        version: "火山云端服务".into(),
        engines: vec![],
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
        let mut result = probe(&config, &key).await?;
        for engine in ["hermes", "pi", "claude_code", "opencode"] {
            let mut scoped = config.clone();
            scoped.agent = engine.into();
            let checked = probe(&scoped, &key).await;
            result.engines.push(EngineProbe {
                engine: engine.into(),
                success: checked.is_ok(),
                message: checked
                    .err()
                    .unwrap_or_else(|| "云端鉴权与用户目录访问通过".into()),
            });
        }
        Ok(result)
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub uri: String,
    pub name: String,
    pub is_directory: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPage {
    pub entries: Vec<MemoryEntry>,
    pub has_more: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDocument {
    pub uri: String,
    pub content: String,
    pub revision: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryWrite {
    pub uri: String,
    pub content: String,
    pub base_revision: String,
}

fn memory_uri(uri: &str, file: bool) -> Result<&str, String> {
    let suffix = MEMORY_ROOTS
        .iter()
        .find_map(|root| uri.strip_prefix(root))
        .ok_or("只允许访问当前用户的云端 memories 目录")?;
    if (suffix.is_empty() && file)
        || (!suffix.is_empty() && !suffix.starts_with('/'))
        || uri.len() > 2048
        || uri.contains(['%', '?', '#', '\\'])
        || uri.chars().any(char::is_control)
        || suffix
            .trim_start_matches('/')
            .split('/')
            .any(|s| s == "." || s == ".." || s.starts_with('.'))
        || (!suffix.is_empty() && suffix[1..].split('/').any(str::is_empty))
    {
        return Err("记忆路径无效".into());
    }
    if file && !uri.ends_with(".md") && !uri.ends_with(".txt") {
        return Err("仅支持读取和编辑 Markdown 或文本记忆".into());
    }
    Ok(uri)
}

/// Cloud writes append managed MEMORY_FIELDS. The editor sends only visible body;
/// optimistic concurrency still hashes the entire stored record, including metadata.
fn visible_memory(content: &str) -> &str {
    let mut body = content.trim_end();
    loop {
        let Some(start) = body.rfind("<!-- MEMORY_FIELDS") else {
            return body;
        };
        let Some(metadata) = body[start + "<!-- MEMORY_FIELDS".len()..].strip_suffix("-->") else {
            return body;
        };
        if !serde_json::from_str::<Value>(metadata.trim()).is_ok_and(|v| v.is_object()) {
            return body;
        }
        body = body[..start].trim_end();
    }
}

fn revision(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

struct MemoryClient {
    client: reqwest::Client,
    config: OpenVikingConfig,
    key: String,
}
impl MemoryClient {
    fn from_app(app: &AppHandle) -> Result<Self, String> {
        let key = crate::commands::get_cached_api_key(app, KEYCHAIN_PROVIDER)?;
        if key.is_empty() {
            return Err("请先保存火山 OpenViking API Key".into());
        }
        Ok(Self {
            client: cloud_client()?,
            config: cloud_config(),
            key,
        })
    }
    fn for_uri(mut self, uri: &str) -> Result<Self, String> {
        memory_uri(uri, false)?;
        if let Some(peer) = uri
            .strip_prefix("viking://~/peers/")
            .and_then(|s| s.split('/').next())
        {
            self.config.agent = peer.into();
        }
        Ok(self)
    }
    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, String> {
        Ok(self
            .client
            .request(method, format!("{}{path}", self.config.endpoint))
            .headers(auth_headers(&self.config, &self.key)?))
    }
    async fn list(&self, uri: &str, offset: usize) -> Result<MemoryPage, String> {
        memory_uri(uri, false)?;
        if offset > 100_000 {
            return Err("目录页数超过限制".into());
        }
        let result = api_result(
            response_json(self.request(Method::GET, "/api/v1/fs/ls")?.query(&[
                ("uri", uri),
                ("output", "original"),
                ("offset", &offset.to_string()),
                ("limit", &PAGE_SIZE.to_string()),
                ("sort_by", "name"),
            ]))
            .await?,
        )?;
        project_entries(uri, result)
    }
    async fn read(&self, uri: &str) -> Result<MemoryDocument, String> {
        memory_uri(uri, true)?;
        let result = api_result(
            response_json(
                self.request(Method::GET, "/api/v1/content/read")?
                    .query(&[("uri", uri), ("raw", "true")]),
            )
            .await?,
        )?;
        let content = result
            .as_str()
            .ok_or("OpenViking 正文格式无效")?
            .to_string();
        if content.len() > CONTENT_LIMIT {
            return Err("记忆正文超过 256 KiB 编辑限制".into());
        }
        Ok(MemoryDocument {
            uri: uri.into(),
            revision: revision(&content),
            content: visible_memory(&content).into(),
        })
    }
    async fn write(&self, request: MemoryWrite) -> Result<MemoryDocument, String> {
        memory_uri(&request.uri, true)?;
        if request.content.len() > CONTENT_LIMIT {
            return Err("记忆正文超过 256 KiB 编辑限制".into());
        }
        let latest = self.read(&request.uri).await?;
        if latest.revision != request.base_revision {
            return Err("云端记忆已被修改，本次未写入；请保留草稿并重新读取后合并".into());
        }
        // Upstream has no conditional-write/CAS. This detects intervening edits,
        // but cannot lock out another cloud client between this read and write.
        api_result(
            response_json(self.request(Method::POST, "/api/v1/content/write")?.json(
                &json!({"uri":request.uri,"content":request.content,"mode":"replace","wait":false}),
            ))
            .await
            .map_err(|error| format!("{error}；本次写入是否完成尚未确认，请保留草稿并刷新核对"))?,
        )?;
        // A failed verification must not report a clean failure or retry the write.
        let saved = self
            .read(&request.uri)
            .await
            .map_err(|_| "写入已提交，但回读确认失败；请保留草稿并刷新确认，不要重复提交")?;
        if saved.content != request.content.trim_end() {
            return Err("写入已提交，但云端内容又发生变化；请保留草稿并刷新确认".into());
        }
        Ok(saved)
    }
}

fn project_entries(uri: &str, result: Value) -> Result<MemoryPage, String> {
    let items = result.as_array().ok_or("OpenViking 目录格式无效")?;
    let mut entries = vec![];
    for item in items.iter().take(PAGE_SIZE) {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or("OpenViking 目录缺少名称")?;
        if name.contains('/') || name.is_empty() {
            return Err("OpenViking 返回无效目录名称".into());
        }
        if name.starts_with('.') {
            continue;
        }
        let entry_uri = format!("{uri}/{name}");
        memory_uri(&entry_uri, false)?;
        let is_directory = item
            .get("isDir")
            .or_else(|| item.get("is_dir"))
            .and_then(Value::as_bool)
            .ok_or("OpenViking 目录缺少类型")?;
        entries.push(MemoryEntry {
            uri: entry_uri,
            name: name.into(),
            is_directory,
        });
    }
    Ok(MemoryPage {
        entries,
        has_more: items.len() >= PAGE_SIZE,
    })
}

#[tauri::command]
pub async fn agent_openviking_list(
    app: AppHandle,
    uri: String,
    offset: usize,
) -> ApiResponse<MemoryPage> {
    let _lock = SAVE_LOCK.lock().await;
    let result = async {
        MemoryClient::from_app(&app)?
            .for_uri(&uri)?
            .list(&uri, offset)
            .await
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}
#[tauri::command]
pub async fn agent_openviking_read(app: AppHandle, uri: String) -> ApiResponse<MemoryDocument> {
    let _lock = SAVE_LOCK.lock().await;
    let result = async {
        MemoryClient::from_app(&app)?
            .for_uri(&uri)?
            .read(&uri)
            .await
    }
    .await;
    match result {
        Ok(value) => ApiResponse::ok(value),
        Err(error) => ApiResponse::err(error),
    }
}
#[tauri::command]
pub async fn agent_openviking_write(
    app: AppHandle,
    request: MemoryWrite,
) -> ApiResponse<MemoryDocument> {
    let _lock = SAVE_LOCK.lock().await;
    let result = async {
        MemoryClient::from_app(&app)?
            .for_uri(&request.uri)?
            .write(request)
            .await
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

    #[test]
    fn only_fixed_cloud_endpoint_and_identity_are_accepted() {
        for endpoint in [
            "http://127.0.0.1:1933",
            "https://user:secret@example.com",
            "https://api.vikingdb.cn-beijing.volces.com.evil/openviking",
            "file:///etc/passwd",
        ] {
            let mut config = cloud_config();
            config.endpoint = endpoint.into();
            let error = normalized_config(config).err().unwrap();
            assert!(!error.contains("secret"));
        }
        assert_eq!(
            normalized_config(cloud_config()).unwrap().endpoint,
            DEFAULT_ENDPOINT
        );
        assert!(normalized_key(Some("a\nb".into())).is_err());
    }
    #[test]
    fn status_hides_secrets_and_never_projects_local_connection() {
        let memory = json!({"provider":"openviking", "openviking":{"endpoint":"http://localhost:1933", "api_key":"private-key"}});
        let status = project_status(&memory, true);
        assert_eq!(status.active_provider, "");
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("private-key") && !serialized.contains("localhost"));
        assert!(status.key_configured);
    }
    #[test]
    fn memory_paths_cannot_escape_to_other_users_or_managed_files() {
        for uri in [
            "viking://~/privacy/secret.md",
            "viking://user/other/memories/a.md",
            "viking://~/memories/../privacy/a.md",
            "viking://~/memories/%2e%2e/a.md",
            "viking://~/memories//a.md",
            "viking://~/memories2/a.md",
            "viking://~/peers/other/memories/a.md",
            "viking://~/peers/hermes/privacy/key.md",
            "viking://~/memories/.abstract.md",
        ] {
            assert!(memory_uri(uri, true).is_err(), "{uri}");
        }
        assert!(memory_uri("viking://~/peers/hermes/memories/preferences/a.md", true).is_ok());
        assert!(memory_uri(MEMORY_ROOT, true).is_err());
        assert!(memory_uri(MEMORY_ROOT, false).is_ok());
        assert!(memory_uri("viking://~/memories/preferences/偏好.md", true).is_ok());
    }
    #[test]
    fn directory_projection_uses_safe_child_names_not_untrusted_uris() {
        let page = project_entries(
            MEMORY_ROOT,
            json!([
                {"name":"preferences", "isDir":true, "uri":"viking://user/other/privacy"},
                {"name":"note.md", "isDir":false}, {"name":".abstract.md", "isDir":false}
            ]),
        )
        .unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].uri, "viking://~/memories/preferences");
        assert!(
            project_entries(MEMORY_ROOT, json!([{"name":"../private.md","isDir":false}])).is_err()
        );
    }

    pub(super) async fn fixture(
        responses: Vec<(u16, Value)>,
    ) -> (MemoryClient, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = cloud_config();
        config.endpoint = format!("http://{}", listener.local_addr().unwrap());
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
                    let mut buffer = [0; 4096];
                    let n = stream.read(&mut buffer).await.unwrap();
                    data.extend_from_slice(&buffer[..n]);
                    if let Some(end) = data.windows(4).position(|s| s == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..end]).to_lowercase();
                        let size = headers
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("content-length: ")
                                    .and_then(|s| s.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + size {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                requests.push(String::from_utf8_lossy(&data).to_string());
                let body = body.to_string();
                let response=format!("HTTP/1.1 {code} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nLocation: http://127.0.0.1:1/leak\r\n\r\n{body}",body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (
            MemoryClient {
                client: reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                config,
                key: "fixture-key".into(),
            },
            handle,
        )
    }
    fn ok(result: Value) -> Value {
        json!({"status":"ok","result":result})
    }
    #[tokio::test]
    async fn cloud_metadata_is_not_resubmitted_or_mistaken_for_a_conflict() {
        let raw = "original\n\n<!-- MEMORY_FIELDS\n{\"version\":1,\"scope\":\"test\"}\n-->";
        let edited = "edited\n\n<!-- MEMORY_FIELDS\n{\"version\":2,\"scope\":\"test\"}\n-->";
        let (client, requests) = fixture(vec![
            (200, ok(json!(raw))),
            (200, ok(json!({}))),
            (200, ok(json!(edited))),
        ])
        .await;
        let saved = client
            .write(MemoryWrite {
                uri: format!("{MEMORY_ROOT}/note.md"),
                content: "edited\n".into(),
                base_revision: revision(raw),
            })
            .await
            .unwrap();
        assert_eq!(saved.content, "edited");
        assert_eq!(saved.revision, revision(edited));
        let requests = requests.await.unwrap();
        assert!(!requests[1].contains("MEMORY_FIELDS"));
        assert_eq!(
            visible_memory("body\n<!-- MEMORY_FIELDS invalid -->"),
            "body\n<!-- MEMORY_FIELDS invalid -->"
        );
    }

    #[tokio::test]
    async fn stale_revision_does_not_issue_write() {
        let (client, requests) = fixture(vec![(200, ok(json!("newer cloud content")))]).await;
        let error = client
            .write(MemoryWrite {
                uri: format!("{MEMORY_ROOT}/note.md"),
                content: "edited".into(),
                base_revision: revision("old"),
            })
            .await
            .err()
            .unwrap();
        assert!(error.contains("已被修改"));
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /api/v1/content/read?"));
        assert!(requests[0].contains("raw=true"));
    }
    #[tokio::test]
    async fn write_uses_full_original_version_and_verifies_saved_text() {
        let (client, requests) = fixture(vec![
            (200, ok(json!("original"))),
            (200, ok(json!({}))),
            (200, ok(json!("edited"))),
        ])
        .await;
        let saved = client
            .write(MemoryWrite {
                uri: format!("{MEMORY_ROOT}/note.md"),
                content: "edited".into(),
                base_revision: revision("original"),
            })
            .await
            .unwrap();
        assert_eq!(saved.revision, revision("edited"));
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].starts_with("POST /api/v1/content/write "));
        assert!(requests[1].contains("\"mode\":\"replace\""));
    }
    #[tokio::test]
    async fn quota_auth_redirect_and_oversize_fail_without_echoing_secrets() {
        for (code, body) in [
            (402, json!({"secret":"fixture-key"})),
            (401, json!({"secret":"fixture-key"})),
            (302, json!({})),
            (200, ok(json!("x".repeat(RESPONSE_LIMIT)))),
        ] {
            let (client, requests) = fixture(vec![(code, body)]).await;
            let error = client
                .read(&format!("{MEMORY_ROOT}/note.md"))
                .await
                .err()
                .unwrap();
            assert!(!error.contains("fixture-key"));
            if code == 402 {
                assert!(error.contains("套餐"));
            }
            assert_eq!(requests.await.unwrap().len(), 1);
        }
    }
    #[tokio::test]
    async fn write_verification_failure_is_not_reported_as_no_write() {
        let (client, requests) = fixture(vec![
            (200, ok(json!("original"))),
            (200, ok(json!({}))),
            (503, json!({})),
        ])
        .await;
        let error = client
            .write(MemoryWrite {
                uri: format!("{MEMORY_ROOT}/note.md"),
                content: "edited".into(),
                base_revision: revision("original"),
            })
            .await
            .err()
            .unwrap();
        assert!(error.contains("写入已提交"));
        assert_eq!(requests.await.unwrap().len(), 3);
    }
}
