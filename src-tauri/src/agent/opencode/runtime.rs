use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::{AppHandle, Manager};

pub const VERSION: &str = "1.18.31";

pub fn target() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else {
        "x86_64-pc-windows-msvc"
    }
}

#[derive(serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Manifest {
    pub(super) version: String,
    pub(super) target: String,
    pub(super) archive_sha256: String,
    pub(super) executable: String,
    pub(super) files: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct Runtime {
    pub version: String,
    pub executable: PathBuf,
}

pub fn locate(app: &AppHandle) -> Result<PathBuf, String> {
    let packaged = app
        .path()
        .resource_dir()
        .map_err(|_| "无法读取应用资源目录")?
        .join("opencode")
        .join(target());
    if packaged.join("manifest.json").is_file() {
        return Ok(packaged);
    }
    #[cfg(debug_assertions)]
    {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/opencode")
            .join(target());
        if source.join("manifest.json").is_file() {
            return Ok(source);
        }
    }
    Err("OpenCode 运行时未随应用安装。源码开发请先运行 pnpm opencode:bundle；安装版请重新安装完整应用。".into())
}

pub fn verify(root: &Path) -> Result<Runtime, String> {
    let expected = match target() {
        "aarch64-apple-darwin" => {
            "caf7f31fa1aec2353ea859d4ef9ab824c6273d941b016e88d51193fa3028d34e"
        }
        "x86_64-apple-darwin" => "f8510eaf400f07c3a2014e3a517e3650c705bcd6ac3e6740351b723ee685042f",
        _ => "0ecd7ffc7f26390ce7799e7bcd409e4f11c410144308a6a5b0fcdce63d871006",
    };
    verify_version(root, VERSION, expected)
}

pub(super) fn verify_version(root: &Path, version: &str, digest: &str) -> Result<Runtime, String> {
    let data = std::fs::read(root.join("manifest.json")).map_err(|_| "OpenCode 缺少完整性清单")?;
    let manifest: Manifest = serde_json::from_slice(&data).map_err(|_| "OpenCode 清单无效")?;
    if manifest.version != version
        || manifest.target != target()
        || manifest.archive_sha256 != digest
    {
        return Err("OpenCode 版本、平台或分发校验不匹配".into());
    }
    if !manifest.files.contains_key(&manifest.executable) {
        return Err("OpenCode 清单缺少执行文件或权限扩展".into());
    }
    let root = root.canonicalize().map_err(|_| "OpenCode 资源不可访问")?;
    for (relative, expected) in &manifest.files {
        let path = root
            .join(relative)
            .canonicalize()
            .map_err(|_| "OpenCode 资源文件缺失")?;
        if !path.starts_with(&root)
            || crate::agent::hermes::file_sha256_hex(&path).map_err(|_| "无法校验 OpenCode 资源")?
                != *expected
        {
            return Err("OpenCode 资源完整性校验失败，请重新构建或安装".into());
        }
    }
    Ok(Runtime {
        version: manifest.version,
        executable: root.join(manifest.executable),
    })
}
