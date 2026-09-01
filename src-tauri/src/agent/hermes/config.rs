//! Sidecar 配置与二进制哈希校验。

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::STUB_PROTOCOL_VERSION;

/// SophoNote 拉起 Hermes（或协议 stub）所需的钉扎配置
#[derive(Debug, Clone)]
pub struct HermesSidecarConfig {
    pub binary_path: PathBuf,
    /// 小写 hex sha256
    pub expected_sha256: String,
    /// App Support 私有目录下的 HERMES_HOME（不得指向 notes）
    pub hermes_home: PathBuf,
    pub version: String,
    /// H4 测试钩子：注入 stub 环境变量（如 HERMES_STUB_DROP_AFTER）
    pub stub_env: Vec<(String, String)>,
}

impl HermesSidecarConfig {
    /// 测试/本地构造：动态注入二进制路径与哈希
    pub fn for_binary(binary_path: PathBuf, hermes_home: PathBuf) -> io::Result<Self> {
        let expected_sha256 = file_sha256_hex(&binary_path)?;
        Ok(Self {
            binary_path,
            expected_sha256,
            hermes_home,
            version: STUB_PROTOCOL_VERSION.to_string(),
            stub_env: Vec::new(),
        })
    }

    /// 追加 stub 测试环境变量
    pub fn with_stub_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.stub_env.push((key.into(), value.into()));
        self
    }
}

/// 计算文件 sha256（小写 hex）
pub fn file_sha256_hex(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 校验路径存在且哈希匹配（小写比较）
pub fn verify_binary_hash(path: &Path, expected_sha256: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("sidecar 二进制不存在: {}", path.display()));
    }
    let actual = file_sha256_hex(path).map_err(|e| format!("读取 sidecar 失败: {e}"))?;
    if actual != expected_sha256.to_ascii_lowercase() {
        return Err(format!(
            "sidecar sha256 不匹配: expected={}, actual={}",
            expected_sha256.to_ascii_lowercase(),
            actual
        ));
    }
    Ok(())
}
