use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tauri::{AppHandle, Manager};

pub const VERSION: &str = "0.85.1";

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
    pub extension: PathBuf,
}

pub fn locate(app: &AppHandle) -> Result<PathBuf, String> {
    let packaged = app
        .path()
        .resource_dir()
        .map_err(|_| "无法读取应用资源目录")?
        .join("pi")
        .join(target());
    if packaged.join("manifest.json").is_file() {
        return Ok(packaged);
    }
    #[cfg(debug_assertions)]
    {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/pi")
            .join(target());
        if source.join("manifest.json").is_file() {
            return Ok(source);
        }
    }
    Err("Pi 运行时未随应用安装。源码开发请先运行 pnpm pi:bundle；安装版请重新安装完整应用。".into())
}

pub fn verify(root: &Path) -> Result<Runtime, String> {
    let expected = match target() {
        "aarch64-apple-darwin" => {
            "d5f70e3c0cf7398eac239fd0261ee074d98b7ba7f6b43fe3617f052ed5b79d06"
        }
        "x86_64-apple-darwin" => "adb918b845625f184d8bea408d55eacaf21aa87238793c0f5b4f3b9737bce62b",
        _ => "002fa95b90d521245b9985d8f168caebc237ad56e7e30b319807dee1b2e17e1c",
    };
    verify_version(root, VERSION, expected)
}

pub(super) fn verify_version(root: &Path, version: &str, digest: &str) -> Result<Runtime, String> {
    let data = std::fs::read(root.join("manifest.json")).map_err(|_| "Pi 缺少完整性清单")?;
    let manifest: Manifest = serde_json::from_slice(&data).map_err(|_| "Pi 清单无效")?;
    if manifest.version != version
        || manifest.target != target()
        || manifest.archive_sha256 != digest
    {
        return Err("Pi 版本、平台或分发校验不匹配".into());
    }
    if !manifest.files.contains_key(&manifest.executable)
        || !manifest.files.contains_key("sophonote-policy.ts")
    {
        return Err("Pi 清单缺少执行文件或权限扩展".into());
    }
    let root = root.canonicalize().map_err(|_| "Pi 资源不可访问")?;
    for (relative, expected) in &manifest.files {
        let path = root
            .join(relative)
            .canonicalize()
            .map_err(|_| "Pi 资源文件缺失")?;
        if !path.starts_with(&root)
            || crate::agent::hermes::file_sha256_hex(&path).map_err(|_| "无法校验 Pi 资源")?
                != *expected
        {
            return Err("Pi 资源完整性校验失败，请重新构建或安装".into());
        }
    }
    let extension = root.join("sophonote-policy.ts");
    if std::fs::read(&extension).map_err(|_| "Pi 权限扩展不可读")?
        != include_bytes!("../../../../scripts/assets/pi-policy.ts")
    {
        return Err("Pi 权限扩展与宿主版本不一致，请运行 pnpm pi:bundle".into());
    }
    Ok(Runtime {
        version: manifest.version,
        executable: root.join(manifest.executable),
        extension,
    })
}
