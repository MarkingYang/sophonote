//! Official Pi releases staged without touching the signed bundle or running processes.
use super::runtime::{self, Manifest, Runtime};
use crate::agent::runtime_updates::report;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tauri::AppHandle;
use tokio::io::{AsyncWriteExt, BufReader};

const API: &str = "https://api.github.com/repos/earendil-works/pi/releases/latest";
const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_UNPACKED: u64 = 512 * 1024 * 1024;
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
#[derive(Clone)]
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
}
fn version_tuple(v: &str) -> Result<(u32, u32, u32), String> {
    let parts = v
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Pi 官方版本格式无效")?;
    if parts.len() != 3 {
        return Err("Pi 官方版本格式无效".into());
    }
    Ok((parts[0], parts[1], parts[2]))
}
fn valid_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn asset_name() -> &'static str {
    match runtime::target() {
        "aarch64-apple-darwin" => "pi-darwin-arm64.tar.gz",
        "x86_64-apple-darwin" => "pi-darwin-x64.tar.gz",
        _ => "pi-windows-x64.zip",
    }
}
fn select_release(release: Release) -> Result<Available, String> {
    if release.draft || release.prerelease {
        return Err("Pi 更新仅接受官方稳定版".into());
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .ok_or("Pi 官方 tag 格式无效")?
        .to_string();
    if version_tuple(&version)? < version_tuple(runtime::VERSION)? {
        return Err("Pi 官方版本低于宿主最低兼容版本".into());
    }
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == asset_name())
        .ok_or("官方稳定版未提供当前平台 Pi 资产")?;
    let expected_url = format!(
        "https://github.com/earendil-works/pi/releases/download/v{version}/{}",
        asset_name()
    );
    let digest = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| valid_digest(d))
        .ok_or("官方资产缺少可信 SHA-256，已停止更新")?
        .to_lowercase();
    if asset.browser_download_url != expected_url || asset.size == 0 || asset.size > MAX_DOWNLOAD {
        return Err("Pi 官方资产来源或大小不符合更新要求".into());
    }
    Ok(Available {
        version,
        url: expected_url,
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
        .map_err(|_| "无法连接 Pi 官方 GitHub Release，请检查网络后重试")?
        .error_for_status()
        .map_err(|e| {
            format!(
                "Pi 版本检查失败：HTTP {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Pi 版本信息下载中断")?;
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("Pi 版本信息过大".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    select_release(serde_json::from_slice(&bytes).map_err(|_| "Pi 官方 Release 信息无效")?)
}
pub(crate) fn root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::storage_layout::StorageLayout::resolve(app)?
        .runtime
        .join("pi-sidecar"))
}
fn slot(root: &Path, p: &Pointer) -> Result<PathBuf, String> {
    version_tuple(&p.version)?;
    if !valid_digest(&p.digest) {
        return Err("Pi 更新指针校验无效".into());
    }
    Ok(root
        .join("versions")
        .join(format!("{}-{}", p.version, p.digest)))
}
pub(crate) fn resolve(app: &AppHandle) -> Result<(Runtime, Option<String>), String> {
    let root = root(app)?;
    let mut warning = None;
    if root.join("active.json").exists() {
        let candidate = (|| {
            let raw =
                std::fs::read(root.join("active.json")).map_err(|_| "无法读取 Pi 更新指针")?;
            if raw.len() > 4096 {
                return Err("Pi 更新指针过大".into());
            }
            let p: Pointer = serde_json::from_slice(&raw).map_err(|_| "Pi 更新指针损坏")?;
            if version_tuple(&p.version)? < version_tuple(runtime::VERSION)? {
                return Err("Pi 私有版本低于随包版本".into());
            }
            let path = slot(&root, &p)?;
            let runtime = runtime::verify_version(&path, &p.version, &p.digest)?;
            let manifest: Manifest = serde_json::from_slice(
                &std::fs::read(path.join("manifest.json")).map_err(|_| "Pi 清单不可读")?,
            )
            .map_err(|_| "Pi 清单无效")?;
            if hashes(&path)? != manifest.files {
                return Err("Pi 更新槽包含未登记或被修改的文件".into());
            }
            Ok::<Runtime, String>(runtime)
        })();
        match candidate {
            Ok(r) => return Ok((r, None)),
            Err(e) => warning = Some(format!("{e}；已回退随包 Pi。")),
        }
    }
    Ok((runtime::verify(&runtime::locate(app)?)?, warning))
}
fn safe_relative(path: &Path) -> bool {
    path.components().count() <= 128
        && path.as_os_str().len() <= 4096
        && !path.as_os_str().is_empty()
        && !path.to_string_lossy().contains(['\\', ':'])
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
fn unpack(data: &[u8], destination: &Path, zip: bool) -> Result<(), String> {
    let mut total = 0u64;
    if zip {
        let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| "Pi ZIP 无效")?;
        if archive.len() > 20_000 {
            return Err("Pi 文件数量过多".into());
        }
        for i in 0..archive.len() {
            let entry = archive.by_index(i).map_err(|_| "Pi ZIP 条目无效")?;
            let path = entry.enclosed_name().ok_or("Pi ZIP 路径越界")?;
            if !safe_relative(&path) || entry.is_symlink() {
                return Err("Pi ZIP 包含不安全路径或链接".into());
            }
            total = total.checked_add(entry.size()).ok_or("Pi ZIP 过大")?;
            if total > MAX_UNPACKED {
                return Err("Pi ZIP 解包超过大小限制".into());
            }
            let output = destination.join(path);
            if entry.is_dir() {
                std::fs::create_dir_all(output).map_err(|_| "Pi 目录创建失败")?;
            } else {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent).map_err(|_| "Pi 目录创建失败")?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)
                    .map_err(|_| "Pi ZIP 文件重复或不可写")?;
                let size = entry.size();
                let copied = std::io::copy(&mut entry.take(size + 1), &mut file)
                    .map_err(|_| "Pi ZIP 解包失败")?;
                if copied != size {
                    return Err("Pi ZIP 文件大小不匹配".into());
                }
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
        for (i, entry) in archive.entries().map_err(|_| "Pi 归档无效")?.enumerate() {
            if i >= 20_000 {
                return Err("Pi 文件数量过多".into());
            }
            let mut entry = entry.map_err(|_| "Pi 归档条目无效")?;
            let path = entry.path().map_err(|_| "Pi 归档路径无效")?.into_owned();
            if !safe_relative(&path)
                || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
            {
                return Err("Pi 归档包含不安全路径或链接".into());
            }
            total = total.checked_add(entry.size()).ok_or("Pi 归档过大")?;
            if total > MAX_UNPACKED {
                return Err("Pi 解包超过大小限制".into());
            }
            let output = destination.join(&path);
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(output).map_err(|_| "Pi 目录创建失败")?;
            } else {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent).map_err(|_| "Pi 目录创建失败")?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)
                    .map_err(|_| "Pi 文件重复或不可写")?;
                std::io::copy(&mut entry, &mut file).map_err(|_| "Pi 解包失败")?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        output,
                        std::fs::Permissions::from_mode(
                            entry.header().mode().unwrap_or(0o644) & 0o777,
                        ),
                    )
                    .map_err(|_| "Pi 权限设置失败")?;
                }
            }
        }
    }
    Ok(())
}
fn hashes(root: &Path) -> Result<BTreeMap<String, String>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|_| "Pi 文件列表不可读")? {
            let entry = entry.map_err(|_| "Pi 文件不可读")?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|_| "Pi 文件类型不可读")?;
            if kind.is_symlink() {
                return Err("Pi 更新槽不允许链接".into());
            }
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() && path != root.join("manifest.json") {
                out.insert(
                    path.strip_prefix(root)
                        .map_err(|_| "Pi 路径越界")?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    crate::agent::hermes::file_sha256_hex(&path).map_err(|_| "Pi 哈希失败")?,
                );
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result)?;
    Ok(result)
}
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn health(runtime: &Runtime, home: &Path) -> Result<(), String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&runtime.executable)
            .arg("--version")
            .env_clear()
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Pi 版本检测超时")?
    .map_err(|_| "Pi 无法启动")?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != runtime.version
    {
        return Err("Pi 实际版本与官方 Release 不一致".into());
    }
    std::fs::create_dir_all(home).map_err(|_| "无法创建 Pi 校验目录")?;
    let mut command = tokio::process::Command::new(&runtime.executable);
    command
        .args([
            "--mode",
            "rpc",
            "--offline",
            "--no-builtin-tools",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
            "--no-context-files",
            "--no-approve",
            "--no-session",
            "--extension",
        ])
        .arg(&runtime.extension)
        .args(["--provider", "sophonote", "--model", "probe"])
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .env("PI_CODING_AGENT_DIR", home)
        .env("PI_OFFLINE", "1")
        .env(
            "SOPHONOTE_PI_MODEL",
            r#"{"baseUrl":"http://127.0.0.1:1/v1","id":"probe","api":"openai-completions"}"#,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for name in ["SystemRoot", "WINDIR", "PATH"] {
        if let Some(v) = std::env::var_os(name) {
            command.env(name, v);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child =
        super::tools::ManagedChild::new(command.spawn().map_err(|_| "Pi 更新版本不能启动")?);
    let mut input = child.0.stdin.take().ok_or("Pi 校验输入缺失")?;
    let mut output = BufReader::new(child.0.stdout.take().ok_or("Pi 校验输出缺失")?);
    tokio::time::timeout(Duration::from_secs(20), async {
        input
            .write_all(b"{\"id\":\"health\",\"type\":\"get_commands\"}\n")
            .await
            .map_err(|_| "Pi 校验写入失败")?;
        while let Some(frame) = super::transport::read_frame(&mut output).await? {
            if frame["id"] == "health" {
                return if frame["success"] == true
                    && frame["data"]["commands"]
                        .as_array()
                        .is_some_and(|commands| {
                            commands.iter().any(|c| c["name"] == "sophonote_host_ready")
                        })
                {
                    Ok(())
                } else {
                    Err("Pi 新版与宿主权限扩展不兼容，保留原版本".into())
                };
            }
        }
        Err("Pi 离线校验提前退出".into())
    })
    .await
    .map_err(|_| "Pi 离线兼容性检查超时")?
}
async fn stage(data: Vec<u8>, available: &Available, root: &Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    if data.len() as u64 != available.size
        || format!("{:x}", Sha256::digest(&data)) != available.digest
    {
        return Err("Pi 官方 SHA-256 或下载大小不匹配".into());
    }
    std::fs::create_dir_all(root.join("versions")).map_err(|_| "无法创建 Pi 私有更新目录")?;
    let staging = Staging(root.join(format!("staging-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&staging.0).map_err(|_| "无法创建 Pi 暂存目录")?;
    let directory = staging.0.clone();
    tokio::task::spawn_blocking(move || unpack(&data, &directory, asset_name().ends_with(".zip")))
        .await
        .map_err(|_| "Pi 解包任务异常")??;
    let executable = staging
        .0
        .join("pi")
        .join(if cfg!(windows) { "pi.exe" } else { "pi" });
    if !executable.is_file() {
        return Err("Pi 官方归档缺少预期执行文件".into());
    }
    std::fs::write(
        staging.0.join("sophonote-policy.ts"),
        include_bytes!("../../../../scripts/assets/pi-policy.ts"),
    )
    .map_err(|_| "无法写入 Pi 权限扩展")?;
    #[cfg(target_os = "macos")]
    {
        let entitlements = staging.0.join("entitlements.plist");
        std::fs::write(
            &entitlements,
            include_bytes!("../../../../scripts/assets/pi-entitlements.plist"),
        )
        .map_err(|_| "Pi 签名配置写入失败")?;
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
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await
        .map_err(|_| "Pi 本地签名超时")?
        .map_err(|_| "Pi 本地签名不可用")?;
        if !status.success() {
            return Err("Pi 本地签名失败".into());
        }
    }
    let runtime = Runtime {
        executable,
        extension: staging.0.join("sophonote-policy.ts"),
        version: available.version.clone(),
    };
    let probe = Staging(root.join(format!("probe-{}", uuid::Uuid::new_v4())));
    health(&runtime, &probe.0).await?;
    let manifest = Manifest {
        version: available.version.clone(),
        target: runtime::target().into(),
        archive_sha256: available.digest.clone(),
        executable: format!("pi/{}", if cfg!(windows) { "pi.exe" } else { "pi" }),
        files: hashes(&staging.0)?,
    };
    std::fs::write(
        staging.0.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|_| "Pi 清单生成失败")?,
    )
    .map_err(|_| "Pi 清单保存失败")?;
    runtime::verify_version(&staging.0, &available.version, &available.digest)?;
    let p = Pointer {
        version: available.version.clone(),
        digest: available.digest.clone(),
    };
    let destination = slot(root, &p)?;
    if destination.exists() {
        runtime::verify_version(&destination, &p.version, &p.digest)?;
    } else {
        std::fs::rename(&staging.0, &destination).map_err(|_| "Pi 更新槽提交失败")?;
    }
    Ok(destination)
}
pub(crate) async fn pull(app: &AppHandle) -> Result<(), String> {
    report(
        app,
        "pi",
        "checking",
        "running",
        "正在检查 Pi 官方稳定版…",
        None,
        None,
    );
    let available = latest().await?;
    if let Ok((current, warning)) = resolve(app) {
        if warning.is_none()
            && version_tuple(&current.version)? >= version_tuple(&available.version)?
        {
            report(
                app,
                "pi",
                "ready",
                "completed",
                &format!("当前 Pi {} 已是官方稳定版。", current.version),
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
        .map_err(|_| "Pi 官方下载连接失败，请检查网络后重试")?
        .error_for_status()
        .map_err(|e| {
            format!(
                "Pi 下载失败：HTTP {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let mut stream = response.bytes_stream();
    let mut data = Vec::new();
    let mut reported = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Pi 下载中断，请重试")?;
        if (data.len() + chunk.len()) as u64 > available.size.min(MAX_DOWNLOAD) {
            return Err("Pi 下载超过官方资产大小".into());
        }
        data.extend_from_slice(&chunk);
        if data.len() - reported >= 256 * 1024 {
            reported = data.len();
            report(
                app,
                "pi",
                "downloading",
                "running",
                "正在下载 Pi 官方分发…",
                Some(data.len() as u64),
                Some(available.size),
            );
        }
    }
    report(
        app,
        "pi",
        "verifying",
        "running",
        "正在解包、校验 SHA-256、签名和离线 RPC 兼容性…",
        Some(data.len() as u64),
        Some(available.size),
    );
    let root = root(app)?;
    stage(data, &available, &root).await?;
    let pointer = Pointer {
        version: available.version,
        digest: available.digest,
    };
    let temporary = root.join(format!("active-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(
        &temporary,
        serde_json::to_vec(&pointer).map_err(|_| "Pi 指针序列化失败")?,
    )
    .map_err(|_| "Pi 更新指针写入失败")?;
    std::fs::rename(&temporary, root.join("active.json"))
        .map_err(|_| "Pi 更新指针提交失败，原版本仍保留")?;
    report(
        app,
        "pi",
        "ready",
        "completed",
        &format!(
            "Pi {} 已就绪，下一轮使用；当前会话不中断。",
            pointer.version
        ),
        None,
        None,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_release_rejects_unsigned_redirected_and_prerelease_assets() {
        let make = || Release {
            tag_name: "v0.85.1".into(),
            draft: false,
            prerelease: false,
            assets: vec![Asset {
                name: asset_name().into(),
                browser_download_url: format!(
                    "https://github.com/earendil-works/pi/releases/download/v0.85.1/{}",
                    asset_name()
                ),
                digest: Some(format!("sha256:{}", "a".repeat(64))),
                size: 100,
            }],
        };
        assert!(select_release(make()).is_ok());
        let mut release = make();
        release.assets[0].digest = None;
        assert!(select_release(release).is_err());
        let mut release = make();
        release.assets[0].browser_download_url = "https://mirror.example/pi".into();
        assert!(select_release(release).is_err());
        let mut release = make();
        release.prerelease = true;
        assert!(select_release(release).is_err());
        assert!(!safe_relative(Path::new("../../bad")));
        assert!(!safe_relative(Path::new("C:\\bad")));
        assert!(!safe_relative(Path::new("/bad")));
    }
    #[tokio::test]
    #[ignore = "validates cached official Pi archive through signing and native offline RPC"]
    async fn official_archive_stages_and_passes_native_health() {
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = Staging(
            project
                .join("target")
                .join(format!("pi-update-test-{}", uuid::Uuid::new_v4())),
        );
        let data = std::fs::read(project.join("target/pi-downloads").join(format!(
            "{}-{}",
            runtime::VERSION,
            asset_name()
        )))
        .unwrap();
        use sha2::{Digest, Sha256};
        let available = Available {
            version: runtime::VERSION.into(),
            url: String::new(),
            digest: format!("{:x}", Sha256::digest(&data)),
            size: data.len() as u64,
        };
        let slot = stage(data, &available, &root.0).await.unwrap();
        let runtime =
            runtime::verify_version(&slot, &available.version, &available.digest).unwrap();
        assert_eq!(runtime.version, available.version);
        std::fs::write(runtime.extension, "tampered").unwrap();
        assert!(runtime::verify_version(&slot, &available.version, &available.digest).is_err());
    }
}
