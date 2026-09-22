//! Host-managed official releases. Each Run keeps an immutable executable path.
use super::runtime::{self, Manifest, Runtime};
use crate::agent::runtime_updates::report;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::AppHandle;

const API: &str = "https://api.github.com/repos/anomalyco/opencode/releases/latest";
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_BINARY: u64 = 512 * 1024 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}
pub(crate) struct Available {
    pub version: String,
    url: String,
    digest: String,
    size: u64,
}
#[derive(Serialize, Deserialize)]
struct Pointer {
    version: String,
    digest: String,
    slot: String,
}

fn version_tuple(v: &str) -> Result<(u32, u32, u32), String> {
    let parts = v
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "OpenCode 官方版本格式无效")?;
    if parts.len() != 3 {
        return Err("OpenCode 官方版本格式无效".into());
    }
    if format!("{}.{}.{}", parts[0], parts[1], parts[2]) != v {
        return Err("OpenCode 官方版本格式无效".into());
    }
    Ok((parts[0], parts[1], parts[2]))
}
fn valid_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn executable_name() -> &'static str {
    if cfg!(windows) {
        "opencode.exe"
    } else {
        "opencode"
    }
}
fn asset_name() -> &'static str {
    match runtime::target() {
        "aarch64-apple-darwin" => "opencode-darwin-arm64.zip",
        "x86_64-apple-darwin" => "opencode-darwin-x64.zip",
        _ => "opencode-windows-x64.zip",
    }
}
fn select_release(release: Release) -> Result<Available, String> {
    if release.draft || release.prerelease {
        return Err("OpenCode 更新仅接受官方稳定版".into());
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or("OpenCode 官方 tag 格式无效")?
        .to_string();
    if version_tuple(&version)? < version_tuple(runtime::VERSION)? {
        return Err("OpenCode 官方版本低于随包兼容版本".into());
    }
    let mut assets = release
        .assets
        .into_iter()
        .filter(|a| a.name == asset_name());
    let asset = assets
        .next()
        .ok_or("官方稳定版未提供当前平台 OpenCode 资产")?;
    let url = format!(
        "https://github.com/anomalyco/opencode/releases/download/v{version}/{}",
        asset_name()
    );
    if assets.next().is_some()
        || asset.browser_download_url != url
        || asset.size == 0
        || asset.size > MAX_DOWNLOAD
    {
        return Err("OpenCode 官方资产来源或大小不符合更新要求".into());
    }
    let digest = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| valid_digest(d))
        .ok_or("官方资产缺少可信 SHA-256，已停止更新")?
        .to_lowercase();
    Ok(Available {
        version,
        url,
        digest,
        size: asset.size,
    })
}
pub(crate) async fn latest() -> Result<Available, String> {
    let (client, _) = crate::agent::hermes::sidecar_update::build_download_client()?;
    let response = client
        .get(API)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| "无法连接 OpenCode 官方 GitHub Release，请检查网络后重试")?
        .error_for_status()
        .map_err(|e| {
            format!(
                "OpenCode 版本检查失败：HTTP {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "OpenCode 版本信息下载中断")?;
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("OpenCode 版本信息过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    select_release(serde_json::from_slice(&bytes).map_err(|_| "OpenCode 官方 Release 信息无效")?)
}
pub(crate) fn root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::storage_layout::StorageLayout::resolve(app)?
        .runtime
        .join("opencode-sidecar"))
}
fn slot(root: &Path, pointer: &Pointer) -> Result<PathBuf, String> {
    version_tuple(&pointer.version)?;
    uuid::Uuid::parse_str(&pointer.slot).map_err(|_| "OpenCode 更新槽标识无效")?;
    if !valid_digest(&pointer.digest) {
        return Err("OpenCode 更新指针摘要无效".into());
    }
    Ok(root.join("versions").join(format!(
        "{}-{}-{}",
        pointer.version, pointer.digest, pointer.slot
    )))
}
fn resolve_at(
    root: &Path,
    bundled: impl FnOnce() -> Result<Runtime, String>,
) -> Result<(Runtime, Option<String>), String> {
    let active = root.join("active.json");
    let candidate = (|| {
        let mut bytes = Vec::new();
        std::fs::File::open(&active)
            .map_err(|_| "无法读取 OpenCode 更新指针")?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法读取 OpenCode 更新指针")?;
        if bytes.len() > 4096 {
            return Err("OpenCode 更新指针过大".into());
        }
        let p: Pointer = serde_json::from_slice(&bytes).map_err(|_| "OpenCode 更新指针损坏")?;
        if version_tuple(&p.version)? < version_tuple(runtime::VERSION)? {
            return Err("OpenCode 私有版本低于随包版本".into());
        }
        runtime::verify_version(&slot(root, &p)?, &p.version, &p.digest)
    })();
    match candidate {
        Ok(runtime) => Ok((runtime, None)),
        Err(error) => {
            let warning = active
                .exists()
                .then(|| format!("{error}；已回退随包 OpenCode。"));
            Ok((bundled()?, warning))
        }
    }
}
pub(crate) fn resolve(app: &AppHandle) -> Result<(Runtime, Option<String>), String> {
    resolve_at(&root(app)?, || runtime::verify(&runtime::locate(app)?))
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Official standalone ZIPs contain exactly one root executable. Reject any layout change.
fn unpack(data: &[u8], destination: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| "OpenCode ZIP 无效")?;
    if archive.len() != 1 {
        return Err("OpenCode 归档必须只包含官方 standalone 执行文件".into());
    }
    let entry = archive.by_index(0).map_err(|_| "OpenCode ZIP 条目无效")?;
    if entry.name() != executable_name()
        || entry.is_symlink()
        || entry.is_dir()
        || entry
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
        || entry.size() == 0
        || entry.size() > MAX_BINARY
    {
        return Err("OpenCode ZIP 路径、类型或大小不符合更新要求".into());
    }
    let size = entry.size();
    let path = destination.join(executable_name());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| "OpenCode 执行文件不可写")?;
    let copied =
        std::io::copy(&mut entry.take(size + 1), &mut file).map_err(|_| "OpenCode ZIP 解包失败")?;
    if copied != size {
        return Err("OpenCode ZIP 文件大小不匹配".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|_| "OpenCode 权限设置失败")?;
    }
    Ok(())
}

async fn health(runtime: &Runtime, home: &Path) -> Result<(), String> {
    std::fs::create_dir_all(home).map_err(|_| "无法创建 OpenCode 校验目录")?;
    let password = uuid::Uuid::new_v4().to_string();
    let config = json!({"autoupdate":false,"share":"disabled","plugin":[],"permission":{"*":"deny"},"provider":{},"mcp":{}});
    let server = super::transport::start_server(runtime, home, &config, &password).await?;
    let base = &server.base;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "OpenCode 校验连接不可用")?;
    let request = |method, path: &str| {
        client
            .request(method, format!("{base}{path}"))
            .basic_auth("opencode", Some(&password))
    };
    let health: serde_json::Value = request(reqwest::Method::GET, "/global/health")
        .send()
        .await
        .map_err(|_| "OpenCode 健康检查连接失败")?
        .error_for_status()
        .map_err(|_| "OpenCode 健康检查失败")?
        .json()
        .await
        .map_err(|_| "OpenCode 健康检查格式无效")?;
    if health["healthy"] != true || health["version"] != runtime.version {
        return Err("OpenCode 实际版本与官方 Release 不一致".into());
    }
    let unauth = client
        .get(format!("{base}/session"))
        .send()
        .await
        .map_err(|_| "OpenCode 认证校验失败")?;
    if unauth.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err("OpenCode 服务未拒绝未认证请求".into());
    }
    let session: serde_json::Value = request(reqwest::Method::POST, "/session")
        .json(&json!({"title":"SophoNote update probe"}))
        .send()
        .await
        .map_err(|_| "OpenCode 会话探测失败")?
        .error_for_status()
        .map_err(|_| "OpenCode 会话创建不兼容")?
        .json()
        .await
        .map_err(|_| "OpenCode 会话响应无效")?;
    let id = session["id"]
        .as_str()
        .filter(|s| {
            s.starts_with("ses_") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .ok_or("OpenCode 会话标识不兼容")?;
    request(reqwest::Method::GET, &format!("/session/{id}"))
        .send()
        .await
        .map_err(|_| "OpenCode 会话读取失败")?
        .error_for_status()
        .map_err(|_| "OpenCode 会话读取不兼容")?;
    let spec: serde_json::Value = request(reqwest::Method::GET, "/doc")
        .send()
        .await
        .map_err(|_| "OpenCode 协议检查失败")?
        .error_for_status()
        .map_err(|_| "OpenCode 协议接口不可用")?
        .json()
        .await
        .map_err(|_| "OpenCode 协议描述无效")?;
    for (path, method) in [
        ("/session/{sessionID}/prompt_async", "post"),
        ("/session/{sessionID}/abort", "post"),
        ("/event", "get"),
    ] {
        if !spec["paths"][path][method].is_object() {
            return Err(format!("OpenCode 新版缺少宿主必需协议 {path}"));
        }
    }
    let response = request(reqwest::Method::GET, "/event")
        .send()
        .await
        .map_err(|_| "OpenCode SSE 探测失败")?
        .error_for_status()
        .map_err(|_| "OpenCode SSE 不兼容")?;
    if !response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return Err("OpenCode SSE 响应类型不兼容".into());
    }
    let mut events = response.bytes_stream();
    let frame = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .map_err(|_| "OpenCode SSE 就绪超时")?
        .ok_or("OpenCode SSE 提前结束")?
        .map_err(|_| "OpenCode SSE 读取失败")?;
    if frame.is_empty() {
        return Err("OpenCode SSE 就绪事件为空".into());
    }
    Ok(())
}

async fn stage(data: Vec<u8>, available: &Available, root: &Path) -> Result<Pointer, String> {
    if data.len() as u64 != available.size
        || format!("{:x}", Sha256::digest(&data)) != available.digest
    {
        return Err("OpenCode 官方 SHA-256 或下载大小不匹配".into());
    }
    std::fs::create_dir_all(root.join("versions")).map_err(|_| "无法创建 OpenCode 私有更新目录")?;
    let staging = Staging(root.join(format!("staging-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&staging.0).map_err(|_| "无法创建 OpenCode 暂存目录")?;
    let directory = staging.0.clone();
    tokio::task::spawn_blocking(move || unpack(&data, &directory))
        .await
        .map_err(|_| "OpenCode 解包任务异常")??;
    let executable = staging.0.join(executable_name());
    std::fs::write(
        staging.0.join("LICENSE.opencode"),
        include_bytes!("../../../../scripts/assets/LICENSE.opencode"),
    )
    .map_err(|_| "OpenCode 许可写入失败")?;
    #[cfg(target_os = "macos")]
    {
        let entitlements = staging.0.join("entitlements.plist");
        std::fs::write(
            &entitlements,
            include_bytes!("../../../../scripts/assets/pi-entitlements.plist"),
        )
        .map_err(|_| "OpenCode 签名配置写入失败")?;
        let status = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::process::Command::new("/usr/bin/codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--timestamp=none",
                    "--entitlements",
                ])
                .arg(&entitlements)
                .arg(&executable)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await
        .map_err(|_| "OpenCode 本地签名超时")?
        .map_err(|_| "OpenCode 本地签名不可用")?;
        if !status.success() {
            return Err("OpenCode 本地签名失败".into());
        }
    }
    let runtime = Runtime {
        executable,
        version: available.version.clone(),
    };
    let probe = Staging(root.join(format!("probe-{}", uuid::Uuid::new_v4())));
    tokio::time::timeout(Duration::from_secs(55), health(&runtime, &probe.0))
        .await
        .map_err(|_| "OpenCode 兼容性检查超时，保留原版本")??;
    let mut files = BTreeMap::new();
    for entry in std::fs::read_dir(&staging.0).map_err(|_| "OpenCode 文件列表不可读")? {
        let path = entry.map_err(|_| "OpenCode 文件不可读")?.path();
        files.insert(
            path.file_name()
                .ok_or("OpenCode 路径无效")?
                .to_string_lossy()
                .into_owned(),
            crate::agent::hermes::file_sha256_hex(&path).map_err(|_| "OpenCode 哈希失败")?,
        );
    }
    let manifest = Manifest {
        version: available.version.clone(),
        target: runtime::target().into(),
        archive_sha256: available.digest.clone(),
        executable: executable_name().into(),
        files,
    };
    std::fs::write(
        staging.0.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|_| "OpenCode 清单生成失败")?,
    )
    .map_err(|_| "OpenCode 清单保存失败")?;
    runtime::verify_version(&staging.0, &available.version, &available.digest)?;
    let pointer = Pointer {
        version: available.version.clone(),
        digest: available.digest.clone(),
        slot: uuid::Uuid::new_v4().to_string(),
    };
    std::fs::rename(&staging.0, slot(root, &pointer)?).map_err(|_| "OpenCode 更新槽提交失败")?;
    Ok(pointer)
}
fn activate(root: &Path, pointer: &Pointer) -> Result<(), String> {
    runtime::verify_version(&slot(root, pointer)?, &pointer.version, &pointer.digest)?;
    let temporary = root.join(format!("active-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::write(
            &temporary,
            serde_json::to_vec(pointer).map_err(|_| "OpenCode 指针序列化失败")?,
        )
        .map_err(|_| "OpenCode 指针写入失败")?;
        std::fs::rename(&temporary, root.join("active.json"))
            .map_err(|_| "OpenCode 更新指针提交失败，原版本仍保留")
    })();
    let _ = std::fs::remove_file(&temporary);
    result.map_err(str::to_owned)
}
pub(crate) async fn pull(app: &AppHandle) -> Result<(), String> {
    report(
        app,
        "opencode",
        "checking",
        "running",
        "正在检查 OpenCode 官方稳定版…",
        None,
        None,
    );
    let available = latest().await?;
    let handle = app.clone();
    if let Ok(Ok((current, warning))) = tokio::task::spawn_blocking(move || resolve(&handle)).await
    {
        if warning.is_none()
            && version_tuple(&current.version)? >= version_tuple(&available.version)?
        {
            report(
                app,
                "opencode",
                "ready",
                "completed",
                &format!("当前 OpenCode {} 已无需更新。", current.version),
                None,
                None,
            );
            return Ok(());
        }
    }
    let (client, _) = crate::agent::hermes::sidecar_update::build_download_client()?;
    let response = client
        .get(&available.url)
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .map_err(|_| "OpenCode 官方下载连接失败，请重试")?
        .error_for_status()
        .map_err(|e| {
            format!(
                "OpenCode 下载失败：HTTP {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let mut stream = response.bytes_stream();
    let mut data = Vec::new();
    let mut reported = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "OpenCode 下载中断，请重试")?;
        if (data.len() + chunk.len()) as u64 > available.size {
            return Err("OpenCode 下载超过官方资产大小".into());
        }
        data.extend_from_slice(&chunk);
        if data.len() - reported >= 256 * 1024 {
            reported = data.len();
            report(
                app,
                "opencode",
                "downloading",
                "running",
                "正在下载 OpenCode 官方分发…",
                Some(data.len() as u64),
                Some(available.size),
            );
        }
    }
    report(
        app,
        "opencode",
        "verifying",
        "running",
        "正在校验 SHA-256、签名和本机协议兼容性…",
        Some(data.len() as u64),
        Some(available.size),
    );
    let root = root(app)?;
    let pointer = stage(data, &available, &root).await?;
    activate(&root, &pointer)?;
    report(
        app,
        "opencode",
        "ready",
        "completed",
        &format!(
            "OpenCode {} 已就绪，下一轮使用；当前会话不中断。",
            pointer.version
        ),
        None,
        None,
    );
    Ok(())
}

#[cfg(test)]
mod tests;
