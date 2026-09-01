//! Hermes Client Surface 附件边界。
//!
//! SophoNote 只校验用户显式选择的资源；正文读取、目录探索和图片解码均交给
//! Hermes Gateway 的原生 attach RPC。这里不得生成 system prompt 或附件提示词。

use std::fs;
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

const MAX_ATTACHMENTS: usize = 20;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_URL_LENGTH: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAttachmentKind {
    Image,
    File,
    Folder,
    Url,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAttachmentInput {
    pub kind: RunAttachmentKind,
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// 粘贴图片走 data URL；文件选择只传 path，由 Hermes 原生附件协议接收。
    #[serde(default)]
    pub data_url: Option<String>,
}

pub fn validate_surface_attachments(
    user_message: &str,
    attachments: &[RunAttachmentInput],
) -> Result<(), String> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(format!("单轮最多添加 {MAX_ATTACHMENTS} 个附件"));
    }
    if user_message.trim().is_empty() && attachments.is_empty() {
        return Err("消息与附件不能同时为空".into());
    }
    for attachment in attachments {
        match attachment.kind {
            RunAttachmentKind::Image => validate_image(attachment)?,
            RunAttachmentKind::File => {
                let path = required_path(attachment)?;
                if !reject_symlink(path)?.is_file() {
                    return Err(format!("文件附件不是普通文件：{}", path.display()));
                }
            }
            RunAttachmentKind::Folder => {
                let path = required_path(attachment)?;
                if !reject_symlink(path)?.is_dir() {
                    return Err(format!("文件夹附件不是目录：{}", path.display()));
                }
            }
            RunAttachmentKind::Url => validate_url(
                attachment
                    .url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "URL 附件不能为空".to_string())?,
            )?,
        }
    }
    Ok(())
}

fn validate_image(attachment: &RunAttachmentInput) -> Result<(), String> {
    if let Some(data_url) = attachment
        .data_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        validate_image_data_url(data_url)?;
        return Ok(());
    }

    let path = required_path(attachment)?;
    let metadata = reject_symlink(path)?;
    if !metadata.is_file() {
        return Err(format!("图片附件不是文件：{}", path.display()));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "图片超过 {} MB：{}",
            MAX_IMAGE_BYTES / 1024 / 1024,
            path.display()
        ));
    }
    image_mime(path).ok_or_else(|| format!("不支持的图片格式：{}", path.display()))?;
    Ok(())
}

fn required_path(attachment: &RunAttachmentInput) -> Result<&Path, String> {
    attachment
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(Path::new)
        .ok_or_else(|| format!("附件「{}」缺少路径", attachment.name))
}

fn reject_symlink(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("无法读取附件 {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "为避免越界读取，不支持符号链接附件：{}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn validate_image_data_url(value: &str) -> Result<(), String> {
    let (header, encoded) = value
        .split_once(',')
        .ok_or_else(|| "无法识别粘贴图片数据".to_string())?;
    if !matches!(
        header,
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/jpg;base64"
            | "data:image/gif;base64"
            | "data:image/webp;base64"
    ) {
        return Err("粘贴图片仅支持 PNG/JPEG/GIF/WebP".into());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "粘贴图片 base64 无效".to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!("粘贴图片超过 {} MB", MAX_IMAGE_BYTES / 1024 / 1024));
    }
    Ok(())
}

fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn validate_url(value: &str) -> Result<(), String> {
    if value.len() > MAX_URL_LENGTH {
        return Err("URL 过长".into());
    }
    let lower = value.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("URL 仅支持 http:// 或 https://".into());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("URL 不能包含空白字符".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_url() {
        let error = validate_surface_attachments(
            "打开",
            &[RunAttachmentInput {
                kind: RunAttachmentKind::Url,
                name: "bad".into(),
                path: None,
                url: Some("file:///etc/passwd".into()),
                data_url: None,
            }],
        )
        .unwrap_err();
        assert!(error.contains("http"));
    }

    #[test]
    fn accepts_supported_pasted_image() {
        let data = format!("data:image/png;base64,{}", STANDARD.encode([1_u8, 2, 3]));
        assert!(validate_surface_attachments(
            "描述图片",
            &[RunAttachmentInput {
                kind: RunAttachmentKind::Image,
                name: "pasted.png".into(),
                path: None,
                url: None,
                data_url: Some(data),
            }],
        )
        .is_ok());
    }
}
