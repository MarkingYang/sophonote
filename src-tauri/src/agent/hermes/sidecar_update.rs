//! DEC-050: Hermes sidecar is updated into an app-private version slot.
//! The signed application resource is immutable; a successful pull only writes
//! `pending.json`, and the next process promotes it after the health gate.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const OFFICIAL_REPOSITORY: &str = "https://github.com/NousResearch/hermes-agent";
const GITHUB_API_ROOT: &str = "https://api.github.com/repos/NousResearch/hermes-agent";
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/NousResearch/hermes-agent/releases/latest";
const MAX_SOURCE_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const DOWNLOAD_ATTEMPTS: usize = 3;
pub const SIDECAR_UPDATE_PROGRESS_EVENT: &str = "sophonote:hermes-sidecar-update-progress";
const OWNED_SKILLS: &[&str] = &[
    "sophonote-help",
    "sophonote-markdown-writing",
    "sophonote-note-persistence",
    "sophonote-ai-radar",
    "sophonote-openrouter-rankings",
    "archify",
];

#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    tarball_url: String,
    prerelease: bool,
    draft: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubGitObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubTagRef {
    object: GithubGitObject,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubAnnotatedTag {
    sha: String,
    tag: String,
    object: GithubGitObject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimePointer {
    directory: String,
    version: String,
    commit: String,
    tag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunningReceipt {
    version: String,
    commit: String,
    source: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RuntimeManifest {
    hermes_version: String,
    hermes_commit: String,
    target: String,
    launcher: String,
    python: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesSidecarStatus {
    pub current_version: String,
    pub current_commit: String,
    pub current_source: String,
    pub pending_version: Option<String>,
    pub pending_commit: Option<String>,
    pub update_ready: bool,
    pub repository: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesSidecarProgress {
    pub operation_id: String,
    pub phase: String,
    pub state: String,
    pub percent: u8,
    pub message: String,
    pub bytes_downloaded: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[derive(Clone)]
struct ProgressReporter {
    app: AppHandle,
    operation_id: String,
    #[allow(clippy::type_complexity)] // (阶段, 进度, 已下载, 总量) 内部进度快照
    last: Arc<Mutex<(String, u8, Option<u64>, Option<u64>)>>,
}

impl ProgressReporter {
    fn new(app: AppHandle) -> Self {
        Self {
            app,
            operation_id: Uuid::new_v4().to_string(),
            last: Arc::new(Mutex::new(("checking".to_string(), 0, None, None))),
        }
    }

    fn running(&self, phase: &str, percent: u8, message: impl Into<String>) {
        self.emit(phase, "running", percent, message, None, None);
    }

    fn downloading(
        &self,
        percent: u8,
        message: impl Into<String>,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    ) {
        self.emit(
            "downloading",
            "running",
            percent,
            message,
            Some(bytes_downloaded),
            total_bytes,
        );
    }

    fn completed(&self) {
        self.emit(
            "staging",
            "completed",
            100,
            "更新已完整校验并准备就绪，重启 SophoNote 后生效。",
            None,
            None,
        );
    }

    fn failed(&self, error: &str) {
        let (phase, percent, bytes_downloaded, total_bytes) = self
            .last
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| ("checking".to_string(), 0, None, None));
        self.emit(
            &phase,
            "failed",
            percent,
            error,
            bytes_downloaded,
            total_bytes,
        );
    }

    fn emit(
        &self,
        phase: &str,
        state: &str,
        percent: u8,
        message: impl Into<String>,
        bytes_downloaded: Option<u64>,
        total_bytes: Option<u64>,
    ) {
        let percent = percent.min(100);
        if state == "running" {
            if let Ok(mut last) = self.last.lock() {
                *last = (phase.to_string(), percent, bytes_downloaded, total_bytes);
            }
        }
        let _ = self.app.emit(
            SIDECAR_UPDATE_PROGRESS_EVENT,
            HermesSidecarProgress {
                operation_id: self.operation_id.clone(),
                phase: phase.to_string(),
                state: state.to_string(),
                percent,
                message: message.into(),
                bytes_downloaded,
                total_bytes,
            },
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct NetworkContext {
    used_macos_fixed_proxy: bool,
    macos_pac_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ManagedRuntimeCandidate {
    pub root: PathBuf,
    pointer: RuntimePointer,
    pending: bool,
}

fn update_root(app: &AppHandle) -> Result<PathBuf, String> {
    let layout = crate::storage_layout::StorageLayout::resolve(app)?;
    layout.ensure()?;
    let root = layout.runtime.join("hermes-sidecar");
    std::fs::create_dir_all(root.join("versions"))
        .map_err(|error| format!("创建 Hermes 更新目录失败: {error}"))?;
    Ok(root)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|error| format!("解析 {} 失败: {error}", path.display()))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "更新指针缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let tmp = parent.join(format!(".pointer-{}.tmp", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&tmp, path).map_err(|error| error.to_string())
}

fn validate_pointer(
    root: &Path,
    pointer: RuntimePointer,
    pending: bool,
) -> Option<ManagedRuntimeCandidate> {
    if pointer.directory.is_empty()
        || pointer.directory.contains('/')
        || pointer.directory.contains('\\')
        || pointer.directory.contains("..")
    {
        return None;
    }
    let candidate_root = root.join("versions").join(&pointer.directory);
    candidate_root
        .join("MANIFEST.toml")
        .is_file()
        .then_some(ManagedRuntimeCandidate {
            root: candidate_root,
            pointer,
            pending,
        })
}

pub fn managed_candidates(app: &AppHandle) -> Result<Vec<ManagedRuntimeCandidate>, String> {
    let root = update_root(app)?;
    let mut candidates = Vec::new();
    if let Some(pointer) = read_json::<RuntimePointer>(&root.join("pending.json"))? {
        if let Some(candidate) = validate_pointer(&root, pointer, true) {
            candidates.push(candidate);
        }
    }
    if let Some(pointer) = read_json::<RuntimePointer>(&root.join("active.json"))? {
        if let Some(candidate) = validate_pointer(&root, pointer, false) {
            if !candidates.iter().any(|item| item.root == candidate.root) {
                candidates.push(candidate);
            }
        }
    }
    Ok(candidates)
}

pub fn candidate_started(
    app: &AppHandle,
    candidate: &ManagedRuntimeCandidate,
) -> Result<(), String> {
    let root = update_root(app)?;
    if candidate.pending {
        write_json_atomic(&root.join("active.json"), &candidate.pointer)?;
        let _ = std::fs::remove_file(root.join("pending.json"));
    }
    record_running(
        app,
        &candidate.pointer.version,
        &candidate.pointer.commit,
        "official-update",
    )
}

pub fn candidate_failed(app: &AppHandle, candidate: &ManagedRuntimeCandidate, error: &str) {
    let Ok(root) = update_root(app) else {
        return;
    };
    let pointer_path = if candidate.pending {
        root.join("pending.json")
    } else {
        root.join("active.json")
    };
    let _ = std::fs::remove_file(pointer_path);
    let failed = serde_json::json!({
        "version": candidate.pointer.version,
        "commit": candidate.pointer.commit,
        "error": error,
    });
    let _ = write_json_atomic(&root.join("last-failed.json"), &failed);
}

pub fn record_running(
    app: &AppHandle,
    version: &str,
    commit: &str,
    source: &str,
) -> Result<(), String> {
    let receipt = RunningReceipt {
        version: version.to_string(),
        commit: commit.to_string(),
        source: source.to_string(),
    };
    write_json_atomic(&update_root(app)?.join("running.json"), &receipt)
}

pub fn status(app: &AppHandle, bundled_root: &Path) -> Result<HermesSidecarStatus, String> {
    let root = update_root(app)?;
    let bundled = read_manifest(bundled_root)?;
    let running =
        read_json::<RunningReceipt>(&root.join("running.json"))?.unwrap_or(RunningReceipt {
            version: bundled.hermes_version,
            commit: bundled.hermes_commit,
            source: "bundled".to_string(),
        });
    let pending = read_json::<RuntimePointer>(&root.join("pending.json"))?;
    Ok(HermesSidecarStatus {
        current_version: running.version,
        current_commit: running.commit,
        current_source: running.source,
        pending_version: pending.as_ref().map(|value| value.version.clone()),
        pending_commit: pending.as_ref().map(|value| value.commit.clone()),
        update_ready: pending.is_some(),
        repository: OFFICIAL_REPOSITORY.to_string(),
    })
}

fn read_manifest(root: &Path) -> Result<RuntimeManifest, String> {
    let path = root.join("MANIFEST.toml");
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("读取 Hermes Manifest {} 失败: {error}", path.display()))?;
    toml::from_str(&raw).map_err(|error| format!("解析 Hermes Manifest 失败: {error}"))
}

pub async fn pull_latest(
    app: AppHandle,
    bundled_root: PathBuf,
) -> Result<HermesSidecarStatus, String> {
    let reporter = ProgressReporter::new(app.clone());
    reporter.running("checking", 2, "正在检查 NousResearch 官方稳定 Release…");
    let result = pull_latest_inner(app, bundled_root, &reporter).await;
    match &result {
        Ok(_) => reporter.completed(),
        Err(error) => reporter.failed(error),
    }
    result
}

async fn pull_latest_inner(
    app: AppHandle,
    bundled_root: PathBuf,
    reporter: &ProgressReporter,
) -> Result<HermesSidecarStatus, String> {
    let (client, network) = build_download_client()?;
    let release = client
        .get(LATEST_RELEASE_API)
        .send()
        .await
        .map_err(|error| network_error("读取 Hermes 官方 Release", error, network))?
        .error_for_status()
        .map_err(|error| format!("Hermes 官方 Release 响应异常: {error}"))?
        .json::<GithubRelease>()
        .await
        .map_err(|error| format!("解析 Hermes 官方 Release 失败: {error}"))?;
    validate_release(&release)?;
    reporter.running(
        "checking",
        8,
        format!("已找到 {}，正在校验官方 tag…", release.tag_name),
    );
    let commit = client
        .get(format!(
            "https://api.github.com/repos/NousResearch/hermes-agent/commits/{}",
            release.tag_name
        ))
        .send()
        .await
        .map_err(|error| network_error("解析 Hermes Release commit", error, network))?
        .error_for_status()
        .map_err(|error| format!("Hermes Release commit 响应异常: {error}"))?
        .json::<GithubCommit>()
        .await
        .map_err(|error| format!("解析 Hermes Release commit 失败: {error}"))?;
    if !is_commit_sha(&commit.sha) {
        return Err("Hermes Release commit 格式无效".into());
    }
    let archive_ref_sha =
        resolve_release_ref(&client, &release.tag_name, &commit.sha, network).await?;
    reporter.downloading(12, "正在连接 GitHub Release 下载…", 0, None);
    let archive =
        download_release_archive(&client, &release.tarball_url, reporter, network).await?;
    reporter.running("unpacking", 38, "下载完成，正在解压并核对源码包…");
    let app_for_build = app.clone();
    let bundled_root_for_build = bundled_root.clone();
    let tag = release.tag_name;
    let commit_sha = commit.sha;
    let archive_ref_sha_for_build = archive_ref_sha;
    let reporter_for_build = reporter.clone();
    tauri::async_runtime::spawn_blocking(move || {
        build_runtime_slot(
            &app_for_build,
            &bundled_root_for_build,
            &tag,
            &commit_sha,
            &archive_ref_sha_for_build,
            &archive,
            &reporter_for_build,
        )
    })
    .await
    .map_err(|error| format!("Hermes Runtime 构建任务异常: {error}"))??;
    status(&app, &bundled_root)
}

async fn resolve_release_ref(
    client: &reqwest::Client,
    tag: &str,
    commit: &str,
    network: NetworkContext,
) -> Result<String, String> {
    let reference = client
        .get(format!("{GITHUB_API_ROOT}/git/ref/tags/{tag}"))
        .send()
        .await
        .map_err(|error| network_error("解析 Hermes Release tag ref", error, network))?
        .error_for_status()
        .map_err(|error| format!("Hermes Release tag ref 响应异常: {error}"))?
        .json::<GithubTagRef>()
        .await
        .map_err(|error| format!("解析 Hermes Release tag ref 失败: {error}"))?;
    let annotated = if reference.object.kind == "tag" {
        Some(
            client
                .get(format!(
                    "{GITHUB_API_ROOT}/git/tags/{}",
                    reference.object.sha
                ))
                .send()
                .await
                .map_err(|error| network_error("解析 Hermes annotated tag", error, network))?
                .error_for_status()
                .map_err(|error| format!("Hermes annotated tag 响应异常: {error}"))?
                .json::<GithubAnnotatedTag>()
                .await
                .map_err(|error| format!("解析 Hermes annotated tag 失败: {error}"))?,
        )
    } else {
        None
    };
    validate_release_ref_chain(tag, commit, &reference, annotated.as_ref())
}

fn validate_release_ref_chain(
    tag: &str,
    commit: &str,
    reference: &GithubTagRef,
    annotated: Option<&GithubAnnotatedTag>,
) -> Result<String, String> {
    if !is_commit_sha(&reference.object.sha) {
        return Err("Hermes Release tag ref SHA 格式无效".into());
    }
    match reference.object.kind.as_str() {
        "commit" if reference.object.sha == commit => Ok(reference.object.sha.clone()),
        "commit" => Err("Hermes Release tag ref 与官方 commit 不匹配".into()),
        "tag" => {
            let annotated =
                annotated.ok_or_else(|| "Hermes annotated tag 缺少 tag object".to_string())?;
            if annotated.sha != reference.object.sha
                || annotated.tag != tag
                || annotated.object.kind != "commit"
                || annotated.object.sha != commit
                || !is_commit_sha(&annotated.object.sha)
            {
                return Err("Hermes annotated tag 链与官方 commit 不匹配".into());
            }
            Ok(reference.object.sha.clone())
        }
        _ => Err("Hermes Release tag ref 类型无效".into()),
    }
}

fn build_download_client() -> Result<(reqwest::Client, NetworkContext), String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .http1_only()
        .user_agent("SophoNote-Hermes-Sidecar-Updater/1")
        .redirect(reqwest::redirect::Policy::limited(5));
    let mut network = NetworkContext::default();
    #[cfg(target_os = "macos")]
    if !has_explicit_proxy_env() {
        let system_proxy = macos_system_proxy();
        network.macos_pac_enabled = system_proxy.pac_enabled;
        if let Some(url) = system_proxy.url {
            builder = builder.proxy(
                reqwest::Proxy::all(&url)
                    .map_err(|error| format!("读取 macOS 系统代理失败: {error}"))?,
            );
            network.used_macos_fixed_proxy = true;
        }
    }
    builder
        .build()
        .map(|client| (client, network))
        .map_err(|error| format!("创建 Hermes 下载客户端失败: {error}"))
}

async fn download_release_archive(
    client: &reqwest::Client,
    url: &str,
    reporter: &ProgressReporter,
    network: NetworkContext,
) -> Result<Vec<u8>, String> {
    let mut last_error = String::new();
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        if attempt > 1 {
            reporter.downloading(
                12,
                format!("下载连接中断，正在进行第 {attempt}/{DOWNLOAD_ATTEMPTS} 次尝试…"),
                0,
                None,
            );
            tokio::time::sleep(Duration::from_secs((attempt - 1) as u64)).await;
        }
        match download_release_archive_once(client, url, reporter).await {
            Ok(archive) => return Ok(archive),
            Err(error) => last_error = error,
        }
    }
    Err(format!(
        "{}。已重试 {} 次；请确认 api.github.com 与 codeload.github.com 可访问{}",
        last_error,
        DOWNLOAD_ATTEMPTS,
        network_hint(network)
    ))
}

async fn download_release_archive_once(
    client: &reqwest::Client,
    url: &str,
    reporter: &ProgressReporter,
) -> Result<Vec<u8>, String> {
    let archive_response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("下载 Hermes Release 失败: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Hermes Release 下载响应异常: {error}"))?;
    let total = archive_response.content_length();
    if total.unwrap_or(0) > MAX_SOURCE_ARCHIVE_BYTES {
        return Err("Hermes Release 源码包超过 128 MiB 安全上限".into());
    }
    let mut archive_stream = archive_response.bytes_stream();
    let mut archive = Vec::new();
    while let Some(chunk) = archive_stream.next().await {
        let chunk = chunk.map_err(|error| format!("读取 Hermes Release 下载内容失败: {error}"))?;
        if archive.len() as u64 + chunk.len() as u64 > MAX_SOURCE_ARCHIVE_BYTES {
            return Err("Hermes Release 源码包超过 128 MiB 安全上限".into());
        }
        archive.extend_from_slice(&chunk);
        let downloaded = archive.len() as u64;
        let percent = total
            .filter(|value| *value > 0)
            .map(|value| 12 + ((downloaded.saturating_mul(23) / value).min(23) as u8))
            .unwrap_or(18);
        reporter.downloading(percent, "正在下载官方 Release…", downloaded, total);
    }
    reporter.downloading(35, "Release 下载完成", archive.len() as u64, total);
    Ok(archive)
}

fn network_error(action: &str, error: reqwest::Error, network: NetworkContext) -> String {
    format!(
        "{action}失败: {error}。请确认 api.github.com 与 codeload.github.com 可访问{}",
        network_hint(network)
    )
}

fn network_hint(network: NetworkContext) -> &'static str {
    if network.used_macos_fixed_proxy {
        "；SophoNote 已使用 macOS 固定系统代理"
    } else if network.macos_pac_enabled {
        "；检测到 macOS PAC 自动代理，但应用无法安全解析 PAC，请配置固定系统代理或 HTTPS_PROXY 后重试"
    } else {
        "；如所在网络需要代理，请配置 macOS 固定系统代理或 HTTPS_PROXY"
    }
}

#[cfg(target_os = "macos")]
fn has_explicit_proxy_env() -> bool {
    ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default, PartialEq, Eq)]
struct MacosSystemProxy {
    url: Option<String>,
    pac_enabled: bool,
}

#[cfg(target_os = "macos")]
fn macos_system_proxy() -> MacosSystemProxy {
    let output = Command::new("/usr/sbin/scutil").arg("--proxy").output();
    match output {
        Ok(output) if output.status.success() => {
            parse_macos_system_proxy(&String::from_utf8_lossy(&output.stdout))
        }
        _ => MacosSystemProxy::default(),
    }
}

#[cfg(target_os = "macos")]
fn parse_macos_system_proxy(raw: &str) -> MacosSystemProxy {
    let mut values = std::collections::HashMap::new();
    for line in raw.lines() {
        let Some((key, value)) = line.trim().split_once(" : ") else {
            continue;
        };
        values.insert(key.trim(), value.trim());
    }
    let pac_enabled = values.get("ProxyAutoConfigEnable") == Some(&"1");
    for (enable, host, port) in [
        ("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
        ("HTTPEnable", "HTTPProxy", "HTTPPort"),
    ] {
        if values.get(enable) != Some(&"1") {
            continue;
        }
        let Some(host) = values.get(host).filter(|value| !value.is_empty()) else {
            continue;
        };
        let Some(port) = values
            .get(port)
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| *value > 0)
        else {
            continue;
        };
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            (*host).to_string()
        };
        return MacosSystemProxy {
            url: Some(format!("http://{host}:{port}")),
            pac_enabled,
        };
    }
    MacosSystemProxy {
        url: None,
        pac_enabled,
    }
}

fn validate_release(release: &GithubRelease) -> Result<(), String> {
    let tag = Regex::new(r"^v[0-9]{4}\.[0-9]{1,2}\.[0-9]{1,2}(?:\.[0-9]+)?$")
        .map_err(|error| error.to_string())?;
    if release.draft || release.prerelease || !tag.is_match(&release.tag_name) {
        return Err("Hermes latest Release 不是可接受的稳定版本".into());
    }
    if !release
        .tarball_url
        .starts_with("https://api.github.com/repos/NousResearch/hermes-agent/")
    {
        return Err("Hermes Release 下载地址不属于官方仓库".into());
    }
    Ok(())
}

fn is_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn build_runtime_slot(
    app: &AppHandle,
    bundled_root: &Path,
    tag: &str,
    commit: &str,
    archive_ref_sha: &str,
    archive: &[u8],
    reporter: &ProgressReporter,
) -> Result<(), String> {
    let update_root = update_root(app)?;
    let stage = update_root.join(format!("staging-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&stage).map_err(|error| error.to_string())?;
    let result = build_runtime_slot_inner(
        &stage,
        bundled_root,
        tag,
        commit,
        archive_ref_sha,
        archive,
        Some(reporter),
    )
    .and_then(|(version, directory)| {
        reporter.running("staging", 97, "校验完成，正在写入待启用版本槽…");
        let versions = update_root.join("versions");
        let final_root = versions.join(&directory);
        if final_root.exists() {
            std::fs::remove_dir_all(&final_root)
                .map_err(|error| format!("替换旧 Hermes 更新槽失败: {error}"))?;
        }
        std::fs::rename(&stage, &final_root)
            .map_err(|error| format!("提交 Hermes 更新槽失败: {error}"))?;
        write_json_atomic(
            &update_root.join("pending.json"),
            &RuntimePointer {
                directory,
                version,
                commit: commit.to_string(),
                tag: tag.to_string(),
            },
        )
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}

fn build_runtime_slot_inner(
    stage: &Path,
    bundled_root: &Path,
    tag: &str,
    commit: &str,
    archive_ref_sha: &str,
    archive: &[u8],
    reporter: Option<&ProgressReporter>,
) -> Result<(String, String), String> {
    report_build_progress(reporter, "unpacking", 40, "正在解压并验证 Release 源码…");
    let unpack = stage.join("unpack");
    std::fs::create_dir_all(&unpack).map_err(|error| error.to_string())?;
    let decoder = GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(&unpack)
        .map_err(|error| format!("解压 Hermes Release 失败: {error}"))?;
    let source = single_child_directory(&unpack)?;
    let archive_root = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !archive_root_matches_ref(archive_root, archive_ref_sha) {
        return Err("Hermes Release 源码包与官方 tag ref 不匹配".into());
    }
    let pyproject = std::fs::read_to_string(source.join("pyproject.toml"))
        .map_err(|error| format!("读取 Hermes pyproject.toml 失败: {error}"))?;
    let version = Regex::new(r#"(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$"#)
        .map_err(|error| error.to_string())?
        .captures(&pyproject)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
        .ok_or_else(|| "Hermes Release 缺少有效语义版本".to_string())?;
    let uv_lock = source.join("uv.lock");
    if !uv_lock.is_file() {
        return Err("Hermes Release 缺少 uv.lock".into());
    }
    let uv_lock_sha = sha256_file(&uv_lock)?;

    report_build_progress(
        reporter,
        "copying",
        47,
        "正在复制受信任的 Python Runtime 与 SophoNote Skill…",
    );
    let bundled_manifest = read_manifest(bundled_root)?;
    if bundled_manifest.target != target_triple() {
        return Err(format!(
            "包内 Hermes Runtime 架构不匹配: manifest={} host={}",
            bundled_manifest.target,
            target_triple()
        ));
    }
    copy_tree(
        &bundled_root.join("runtime/python"),
        &stage.join("runtime/python"),
    )?;
    copy_tree(
        &bundled_root.join("runtime/bin"),
        &stage.join("runtime/bin"),
    )?;
    copy_tree(&bundled_root.join("seed"), &stage.join("seed"))?;
    let site_packages = stage.join("runtime/site-packages");
    std::fs::create_dir_all(&site_packages).map_err(|error| error.to_string())?;
    let python_relative = bundled_manifest.python;
    let launcher_relative = bundled_manifest.launcher;
    let python = stage.join(&python_relative);
    let source_spec = format!("{}[mcp]", source.display());
    report_build_progress(
        reporter,
        "installing",
        56,
        "正在安装 Hermes 与锁定依赖，通常需要 1–3 分钟…",
    );
    run_checked(
        Command::new(&python)
            .args([
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-input",
                "--upgrade",
                "--target",
            ])
            .arg(&site_packages)
            .arg(source_spec)
            // Hermes deliberately blocks ordinary wheel/sdist distribution.
            // The private immutable sidecar is a sealed downstream package,
            // so use the upstream build gate also used by its sealed Nix build.
            .env("HERMES_NIX_BUILD", "1")
            .env("PYTHONHOME", stage.join("runtime/python"))
            .env_remove("PYTHONPATH"),
        "安装 Hermes Runtime",
    )?;

    report_build_progress(
        reporter,
        "installing",
        70,
        "依赖安装完成，正在整理 Runtime…",
    );
    let upstream_skills = source.join("skills");
    if upstream_skills.is_dir() {
        let seed_skills = stage.join("seed/skills");
        if seed_skills.exists() {
            std::fs::remove_dir_all(&seed_skills).map_err(|error| error.to_string())?;
        }
        copy_tree(&upstream_skills, &seed_skills)?;
        overlay_owned_skills(bundled_root, &seed_skills)?;
    }
    std::fs::remove_dir_all(&unpack).map_err(|error| error.to_string())?;
    remove_bytecode(stage)?;
    report_build_progress(
        reporter,
        "verifying",
        76,
        "正在校验 Hermes 与 MCP HTTP client 导入…",
    );
    run_checked(
        Command::new(&python)
            .args([
                "-c",
                "import hermes_cli.main; from mcp.client.streamable_http import streamable_http_client",
            ])
            .env("PYTHONHOME", stage.join("runtime/python"))
            .env("PYTHONPATH", &site_packages),
        "校验 Hermes 与 MCP HTTP client",
    )?;

    #[cfg(target_os = "macos")]
    {
        report_build_progress(reporter, "signing", 83, "正在签名应用私有 Runtime…");
        sign_macos_runtime(stage)?;
    }
    #[cfg(not(target_os = "macos"))]
    report_build_progress(
        reporter,
        "signing",
        83,
        "当前平台无需额外签名，继续完整性校验…",
    );

    report_build_progress(
        reporter,
        "hashing",
        89,
        "正在生成逐文件 SHA-256 完整性清单…",
    );
    let files = hash_runtime_files(stage)?;
    let files_path = stage.join("FILES.sha256");
    let mut file = File::create(&files_path).map_err(|error| error.to_string())?;
    for (relative, hash) in files {
        writeln!(file, "{hash}  {relative}").map_err(|error| error.to_string())?;
    }
    let launcher_sha = sha256_file(&stage.join(&launcher_relative))?;
    let python_sha = sha256_file(&python)?;
    let files_sha = sha256_file(&files_path)?;
    let manifest = format!(
        "schema_version = 1\nhermes_version = \"{version}\"\nhermes_commit = \"{commit}\"\ntarget = \"{}\"\nlauncher = \"{launcher_relative}\"\nlauncher_sha256 = \"{launcher_sha}\"\npython = \"{python_relative}\"\npython_sha256 = \"{python_sha}\"\nfiles_manifest = \"FILES.sha256\"\nfiles_manifest_sha256 = \"{files_sha}\"\nuv_lock_sha256 = \"{uv_lock_sha}\"\nsource_tag = \"{tag}\"\nsource_ref_sha = \"{archive_ref_sha}\"\nbuilder = \"sophonote-private-slot-v1\"\n",
        target_triple()
    );
    std::fs::write(stage.join("MANIFEST.toml"), manifest).map_err(|error| error.to_string())?;
    let directory = format!("{}-{}", version, &commit[..12]);
    Ok((version, directory))
}

fn archive_root_matches_ref(archive_root: &str, archive_ref_sha: &str) -> bool {
    is_commit_sha(archive_ref_sha) && archive_root.ends_with(&format!("-{}", &archive_ref_sha[..7]))
}

fn report_build_progress(
    reporter: Option<&ProgressReporter>,
    phase: &str,
    percent: u8,
    message: &str,
) {
    if let Some(reporter) = reporter {
        reporter.running(phase, percent, message);
    }
}

fn single_child_directory(root: &Path) -> Result<PathBuf, String> {
    let children = std::fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    match children.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err("Hermes Release 源码包目录结构无效".into()),
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("Hermes Runtime 目录不存在: {}", source.display()));
    }
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            std::fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "复制 Hermes Runtime {} 失败: {error}",
                    source_path.display()
                )
            })?;
            let permissions = std::fs::metadata(&source_path)
                .map_err(|error| error.to_string())?
                .permissions();
            std::fs::set_permissions(&destination_path, permissions)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn overlay_owned_skills(bundled_root: &Path, seed_skills: &Path) -> Result<(), String> {
    for name in OWNED_SKILLS {
        let source = bundled_root.join("seed/skills/productivity").join(name);
        if !source.join("SKILL.md").is_file() {
            // 旧包可能尚未携带后续新增的自有 Skill；更新不应
            // 因一个可选 overlay 卡死。已携带的每个 Skill 仍覆盖网络源。
            eprintln!("[hermes-update] bundled owned skill unavailable: {name}");
            continue;
        }
        let destination = seed_skills.join("productivity").join(name);
        if destination.exists() {
            std::fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
        }
        copy_tree(&source, &destination)?;
    }
    Ok(())
}

fn remove_bytecode(root: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() == "__pycache__" {
                std::fs::remove_dir_all(path).map_err(|error| error.to_string())?;
            } else {
                remove_bytecode(&path)?;
            }
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("pyc" | "pyo")
        ) {
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn run_checked(command: &mut Command, action: &str) -> Result<(), String> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("{action}失败: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail = stderr
        .chars()
        .rev()
        .take(2000)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    Err(format!("{action}失败 ({}): {}", output.status, tail.trim()))
}

fn hash_runtime_files(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    files
        .into_iter()
        .filter(|relative| relative != "FILES.sha256" && relative != "MANIFEST.toml")
        .map(|relative| {
            let hash = sha256_file(&root.join(&relative))?;
            Ok((relative, hash))
        })
        .collect()
}

fn collect_files(root: &Path, current: &Path, files: &mut Vec<String>) -> Result<(), String> {
    for entry in std::fs::read_dir(current).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(target_os = "macos")]
fn sign_macos_runtime(root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    for relative in files {
        let path = root.join(relative);
        let probe = Command::new("/usr/bin/file")
            .arg(&path)
            .output()
            .map_err(|error| format!("探测 Runtime 文件失败: {error}"))?;
        if !String::from_utf8_lossy(&probe.stdout).contains("Mach-O") {
            continue;
        }
        run_checked(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&path),
            "签名 Hermes Runtime 原生依赖",
        )?;
    }
    Ok(())
}

fn target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return "aarch64-apple-darwin";
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return "x86_64-apple-darwin";
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return "x86_64-pc-windows-msvc";
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        return "aarch64-pc-windows-msvc";
    }
    #[allow(unreachable_code)]
    "unsupported-target"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_official_stable_release() {
        let good = GithubRelease {
            tag_name: "v2026.8.27".into(),
            tarball_url:
                "https://api.github.com/repos/NousResearch/hermes-agent/tarball/v2026.8.27".into(),
            prerelease: false,
            draft: false,
        };
        assert!(validate_release(&good).is_ok());
        let mut bad = good.clone();
        bad.tarball_url = "https://example.com/hermes.tar.gz".into();
        assert!(validate_release(&bad).is_err());
        let mut prerelease = good;
        prerelease.prerelease = true;
        assert!(validate_release(&prerelease).is_err());
    }

    #[test]
    fn rejects_pointer_traversal() {
        let root = PathBuf::from("/tmp/sophonote-sidecar-test");
        let pointer = RuntimePointer {
            directory: "../escape".into(),
            version: "1.0.0".into(),
            commit: "a".repeat(40),
            tag: "v2026.8.27".into(),
        };
        assert!(validate_pointer(&root, pointer, true).is_none());
    }

    #[test]
    fn validates_lightweight_and_annotated_tag_chains() {
        let commit = "5fc308a70719a83cccdbba4c0e39c23f5a8239d5";
        let lightweight = GithubTagRef {
            object: GithubGitObject {
                sha: commit.into(),
                kind: "commit".into(),
            },
        };
        assert_eq!(
            validate_release_ref_chain("v2026.8.27", commit, &lightweight, None).unwrap(),
            commit
        );

        let tag_object_sha = "fcebd62163497e77e5de00d26d2ed86cb4ef8761";
        let annotated_ref = GithubTagRef {
            object: GithubGitObject {
                sha: tag_object_sha.into(),
                kind: "tag".into(),
            },
        };
        let annotated = GithubAnnotatedTag {
            sha: tag_object_sha.into(),
            tag: "v2026.8.27".into(),
            object: GithubGitObject {
                sha: commit.into(),
                kind: "commit".into(),
            },
        };
        assert_eq!(
            validate_release_ref_chain("v2026.8.27", commit, &annotated_ref, Some(&annotated),)
                .unwrap(),
            tag_object_sha
        );
        assert!(archive_root_matches_ref(
            "NousResearch-hermes-agent-fcebd62",
            tag_object_sha,
        ));
        assert!(!archive_root_matches_ref(
            "NousResearch-hermes-agent-5fc308a",
            tag_object_sha,
        ));

        let mut mismatched = annotated;
        mismatched.object.sha = "a".repeat(40);
        assert!(validate_release_ref_chain(
            "v2026.8.27",
            commit,
            &annotated_ref,
            Some(&mismatched),
        )
        .is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_fixed_macos_proxy_and_tracks_pac() {
        let proxy = parse_macos_system_proxy(
            r#"<dictionary> {
  HTTPEnable : 1
  HTTPPort : 7890
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7891
  HTTPSProxy : proxy.internal
  ProxyAutoConfigEnable : 1
}"#,
        );
        assert_eq!(proxy.url.as_deref(), Some("http://proxy.internal:7891"));
        assert!(proxy.pac_enabled);

        let pac_only = parse_macos_system_proxy("ProxyAutoConfigEnable : 1");
        assert_eq!(pac_only.url, None);
        assert!(pac_only.pac_enabled);
    }

    /// 手动宿主门禁：对官方 tarball 跑完随包 Python 复制、pip 构建、
    /// MCP 导入、macOS 嵌套签名与逐文件 hash。只写系统临时目录。
    #[test]
    #[ignore = "requires an official release archive and a full sidecar build"]
    fn builds_official_release_fixture_end_to_end() {
        let archive_path = std::env::var("SOPHONOTE_HERMES_RELEASE_ARCHIVE")
            .expect("set SOPHONOTE_HERMES_RELEASE_ARCHIVE");
        let tag =
            std::env::var("SOPHONOTE_HERMES_RELEASE_TAG").expect("set SOPHONOTE_HERMES_RELEASE_TAG");
        let commit = std::env::var("SOPHONOTE_HERMES_RELEASE_COMMIT")
            .expect("set SOPHONOTE_HERMES_RELEASE_COMMIT");
        let archive_ref_sha =
            std::env::var("SOPHONOTE_HERMES_RELEASE_REF_SHA").unwrap_or_else(|_| commit.clone());
        let bundled_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/hermes")
            .join(target_triple());
        let stage = std::env::temp_dir().join(format!(
            "sophonote-hermes-update-test-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&stage).unwrap();
        let archive = std::fs::read(archive_path).unwrap();
        let result = build_runtime_slot_inner(
            &stage,
            &bundled_root,
            &tag,
            &commit,
            &archive_ref_sha,
            &archive,
            None,
        );
        let manifest_exists = stage.join("MANIFEST.toml").is_file();
        let files_exists = stage.join("FILES.sha256").is_file();
        std::fs::remove_dir_all(stage).unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(manifest_exists);
        assert!(files_exists);
    }
}
