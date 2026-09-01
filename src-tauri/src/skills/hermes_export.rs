//! H5：Skill 只读导出缓存（非真相源；Hermes 侧只读消费）。
//! 真相源仍为 SophoNote Skill Loader / 启用态。

use std::fs;
use std::path::Path;

use serde::Serialize;

use super::{LoadedSkill, SkillManifest, SkillSource};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesSkillExportEntry {
    pub name: String,
    pub version: u32,
    pub description: String,
    pub source: String,
    pub tools: Vec<String>,
    pub body: String,
}

impl HermesSkillExportEntry {
    pub fn from_manifest(manifest: &SkillManifest, source: SkillSource) -> Self {
        Self {
            name: manifest.name.clone(),
            version: manifest.version,
            description: manifest.description.clone(),
            source: source.as_str().to_string(),
            tools: manifest.tools.clone(),
            body: manifest.body.clone(),
        }
    }
}

/// 将可激活 Skill 导出为只读 JSON 缓存目录（覆盖写入 `skills.json`）。
/// 返回写出的条目数。
pub fn export_skills_readonly_cache(
    skills: &[LoadedSkill],
    dest_dir: &Path,
) -> Result<usize, String> {
    fs::create_dir_all(dest_dir).map_err(|e| format!("创建 Skill 导出目录失败: {e}"))?;
    let mut entries = Vec::new();
    for skill in skills {
        let Some(manifest) = &skill.manifest else {
            continue;
        };
        entries.push(HermesSkillExportEntry::from_manifest(
            manifest,
            skill.source,
        ));
    }
    let path = dest_dir.join("skills.json");
    let json = serde_json::to_string_pretty(&entries)
        .map_err(|e| format!("序列化 Skill 导出失败: {e}"))?;
    // 原子写：tmp + rename
    let tmp = dest_dir.join("skills.json.tmp");
    fs::write(&tmp, &json).map_err(|e| format!("写 Skill 导出临时文件失败: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("提交 Skill 导出失败: {e}"))?;
    // 尽量只读（最佳努力；失败不阻断）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o444));
    }
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillExecution, SkillManifest, SkillSource};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn export_writes_skills_json_without_mutating_source() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sophonote-skill-export-{nanos}"));
        let manifest = SkillManifest {
            name: "demo".into(),
            version: 1,
            description: "d".into(),
            execution: SkillExecution::Agent,
            tools: vec!["list_project_documents".into()],
            max_model_calls: None,
            max_tool_calls: None,
            body: "body".into(),
        };
        let loaded = LoadedSkill {
            manifest: Some(manifest),
            source: SkillSource::Bundled,
            origin: "bundled:demo".into(),
            problems: Vec::new(),
        };
        let n = export_skills_readonly_cache(&[loaded], &dir).unwrap();
        assert_eq!(n, 1);
        let raw = fs::read_to_string(dir.join("skills.json")).unwrap();
        assert!(raw.contains("\"name\": \"demo\""));
        assert!(raw.contains("list_project_documents"));
        let _ = fs::remove_dir_all(dir);
    }
}
