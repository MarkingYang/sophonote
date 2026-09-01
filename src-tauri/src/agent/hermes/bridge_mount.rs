//! 将 SophoNote Surface 的宿主配置写入私有或显式附着的 Hermes `config.yaml`。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};

use crate::sophonote_mcp::{BRIDGE_MCP_NAME, BRIDGE_TOOL_NAMES};

pub const ENV_HERMES_HOME: &str = "SOPHONOTE_HERMES_HOME";

static BUNDLED_HERMES_HOME: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn bundled_home_slot() -> &'static RwLock<Option<PathBuf>> {
    BUNDLED_HERMES_HOME.get_or_init(|| RwLock::new(None))
}

/// 由包内 Runtime 生命周期安装私有 Hermes Home。Release 的配置、MCP 与凭据
/// 只能写入这个目录，绝不回退到机器上的 `~/.hermes`。
pub fn install_bundled_home(path: PathBuf) {
    if let Ok(mut slot) = bundled_home_slot().write() {
        *slot = Some(path);
    }
}

pub fn clear_bundled_home() {
    if let Ok(mut slot) = bundled_home_slot().write() {
        *slot = None;
    }
}

/// 解析 Hermes home：包内私有目录，或显式开发附着目录。
/// 不允许隐式回退到 `~/.hermes`，避免修改 Hermes Desktop 的独立数据。
pub fn hermes_home() -> Option<PathBuf> {
    if let Ok(slot) = bundled_home_slot().read() {
        if let Some(path) = slot.as_ref() {
            return Some(path.clone());
        }
    }
    if let Ok(p) = std::env::var(ENV_HERMES_HOME) {
        let t = p.trim();
        if !t.is_empty() {
            return Some(PathBuf::from(t));
        }
    }
    None
}

/// 配置 SophoNote Hermes Surface 的 Host-owned 项。
///
/// 不覆盖 Runtime/用户的其他设置：
/// - SophoNote 自己维护 Thread 标题，关闭重复的辅助标题请求；
/// - Hermes 默认 cwd 指向 Agent 可直接操作的 `workspace/`，绝不指向 `notes/`；
/// - 挂载受 Lease 约束的 sophonote-bridge。
pub fn configure_sophonote_surface(
    mcp_url: &str,
    bearer: &str,
    workspace: &Path,
) -> Result<(PathBuf, bool), String> {
    let home =
        hermes_home().ok_or_else(|| format!("未找到 Hermes home：请设置 {ENV_HERMES_HOME}"))?;
    let config_path = home.join("config.yaml");
    let raw = if config_path.is_file() {
        fs::read_to_string(&config_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };

    let prev_url = read_bridge_url(&config_path);
    let prev_bearer = read_bridge_bearer(&config_path);

    let mut root: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_yaml::from_str(&raw).map_err(|e| format!("parse config.yaml: {e}"))?
    };

    let root_object = root
        .as_object_mut()
        .ok_or("config.yaml root 不是 mapping")?;
    let auxiliary = object_entry(root_object, "auxiliary")?;
    let title_generation = object_entry(auxiliary, "title_generation")?;
    title_generation.insert("enabled".to_string(), Value::Bool(false));

    let terminal = object_entry(root_object, "terminal")?;
    terminal.insert(
        "cwd".to_string(),
        Value::String(workspace.to_string_lossy().into_owned()),
    );

    // SophoNote Composer 的图片是用户明确选择的多模态输入。强制走主模型
    // 原生视觉通道，避免 Runtime 在未知/自定义模型上降级成一个并未注册的
    // `vision_analyze` 文本提示。若 Provider 确实不支持视觉，真实模型请求会
    // 明确失败，用户可切换支持图片的模型。
    let agent = object_entry(root_object, "agent")?;
    agent.insert(
        "image_input_mode".to_string(),
        Value::String("native".into()),
    );

    let servers = root_object
        .entry("mcp_servers".to_string())
        .or_insert_with(|| json!({}));

    let include: Vec<Value> = BRIDGE_TOOL_NAMES
        .iter()
        .map(|n| Value::String((*n).to_string()))
        .collect();

    let entry = json!({
        "url": mcp_url,
        "headers": {
            "Authorization": format!("Bearer {bearer}")
        },
        "enabled": true,
        "timeout": 120,
        "skip_preflight": true,
        "tools": {
            "include": include
        }
    });

    servers
        .as_object_mut()
        .ok_or("mcp_servers 不是 mapping")?
        .insert(BRIDGE_MCP_NAME.to_string(), entry);

    let yaml = serde_yaml::to_string(&root).map_err(|e| e.to_string())?;
    // 备份
    if config_path.is_file() {
        let bak = config_path.with_extension("yaml.sophonote.bak");
        let _ = fs::copy(&config_path, &bak);
    }
    fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    fs::write(&config_path, yaml).map_err(|e| e.to_string())?;

    let changed = prev_url.as_deref() != Some(mcp_url) || prev_bearer.as_deref() != Some(bearer);
    eprintln!(
        "[sophonote-bridge] 已写入 {} → mcp_servers.{BRIDGE_MCP_NAME} url={mcp_url} changed={changed}",
        config_path.display()
    );
    // 不调用 PATH 上的 `hermes gateway restart`。包内 Runtime 在启动前写入
    // 配置；运行中变更由 BundledHermesRuntime 生命周期显式重启。
    Ok((config_path, changed))
}

fn object_entry<'a>(
    parent: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, Value>, String> {
    parent
        .entry(key.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("config.yaml {key} 不是 mapping"))
}

fn read_bridge_bearer(config_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(config_path).ok()?;
    let root: Value = serde_yaml::from_str(&raw).ok()?;
    let auth = root
        .get("mcp_servers")?
        .get(BRIDGE_MCP_NAME)?
        .get("headers")?
        .get("Authorization")?
        .as_str()?;
    auth.strip_prefix("Bearer ")
        .map(|s| s.to_string())
        .or_else(|| Some(auth.to_string()))
}

/// 从现有 config 读取是否已配置 sophonote-bridge url（测试用）
pub fn read_bridge_url(config_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(config_path).ok()?;
    let root: Value = serde_yaml::from_str(&raw).ok()?;
    root.get("mcp_servers")?
        .get(BRIDGE_MCP_NAME)?
        .get("url")?
        .as_str()
        .map(|s| s.to_string())
}

// ============================================================
// MODEL-11③：本地/免鉴权供应商同步进 Hermes `providers:`
//
// Hermes Runtime 对本地端点不读 OLLAMA_API_KEY/BASE_URL 之类注入——
// 裸 "ollama" 在 Hermes 里是 "custom" 本地别名，用户自定义端点只有
// config.yaml 的 `providers:` 块这一条注册路径。SophoNote 因此把设置中
// requiresKey=false 的 OpenAI 兼容实例写成带 sophonote_managed 标记的
// 条目：Runtime 目录出现该行（Chat 选择器可见），执行时
// resolve_provider_full 第 0 步按原始名命中用户供应商（可选可跑）。
// 同步只触碰带标记的条目，Hermes/用户自己的 providers 配置不受影响。
// ============================================================

const SOPHONOTE_MANAGED_KEY: &str = "sophonote_managed";

/// 免鉴权实例统一引用的占位凭据环境变量（非机密，由 bundled_runtime 注入）。
/// Hermes 对无鉴权本地端点就是这么做的（LM Studio 用内置占位 Key）；给出
/// 非空 key_env 可让 Runtime 把该供应商视为已配置，请求携带的占位 Bearer
/// 会被 Ollama/vLLM 等本地端点忽略。
pub const LOCAL_PLACEHOLDER_KEY_ENV: &str = "SOPHONOTE_LOCAL_API_KEY";

#[derive(Debug, Clone, PartialEq)]
pub struct LocalProviderSyncEntry {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub models: Vec<String>,
}

/// 从 ai_config 提取应同步进 Hermes 的实例：仅 requiresKey=false、
/// openai 协议、端点与默认模型非空。带凭据的云供应商走 env 注入 +
/// Hermes 内置目录，不在此列（避免与内置行重复）。
pub fn local_provider_sync_entries(ai_config: &Value) -> Vec<LocalProviderSyncEntry> {
    let Some(providers) = ai_config.get("providers").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for (id, provider) in providers {
        if provider.get("requiresKey").and_then(Value::as_bool) != Some(false) {
            continue;
        }
        let protocol = provider
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("openai");
        if protocol != "openai" {
            continue;
        }
        let base_url = provider
            .get("baseUrl")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let model = provider
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() || model.is_empty() {
            continue;
        }
        // 免鉴权语义只对本地/私有网络端点成立。云上端点被标记 requiresKey=false
        // 通常来自抽屉误切换——绝不能把它写成托管免鉴权条目（占位凭据发往云端
        // 必 401）；这类实例由 bundled_runtime 的安全网按真实凭据处理。
        if !super::local_proxy::is_local_endpoint(&base_url) {
            continue;
        }
        let name = provider
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(id.as_str())
            .to_string();
        let mut models = vec![model.clone()];
        if let Some(list) = provider.get("models").and_then(Value::as_array) {
            for candidate in list {
                let Some(candidate) = candidate
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                if !models.iter().any(|existing| existing == candidate) {
                    models.push(candidate.to_string());
                }
            }
        }
        entries.push(LocalProviderSyncEntry {
            id: id.clone(),
            name,
            base_url,
            model,
            models,
        });
    }
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    entries
}

/// 把同步条目写入 config root 的 `providers:`，返回内容是否变化。
/// upsert 当前实例、删除 SophoNote 早先写入但设置里已移除的条目；
/// 无 sophonote_managed 标记的键一律不动。
/// `proxy_port` 为 loopback 剥鉴权代理端口（MODEL-11④）：http 目标的
/// base_url 改写为代理地址，Hermes 发出的占位 Bearer 由代理剥离后再转发；
/// https 或无代理时直连真实端点。
pub fn apply_local_provider_sync(
    root: &mut Value,
    entries: &[LocalProviderSyncEntry],
    proxy_port: Option<u16>,
) -> bool {
    let before = root.to_string();
    let Some(root_object) = root.as_object_mut() else {
        return false;
    };
    let Some(providers) = root_object
        .entry("providers".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
    else {
        return false;
    };

    let current_ids: std::collections::BTreeSet<&str> =
        entries.iter().map(|entry| entry.id.as_str()).collect();
    providers.retain(|key, value| {
        let managed = value
            .get(SOPHONOTE_MANAGED_KEY)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        !managed || current_ids.contains(key.as_str())
    });
    for entry in entries {
        let models: Vec<Value> = entry
            .models
            .iter()
            .map(|model| Value::String(model.clone()))
            .collect();
        // 免鉴权端点（Ollama 等）会校验请求携带的 Authorization 头，而 Hermes
        // 必发占位 Bearer——经 loopback 代理剥离后再转发真实端点。
        let effective_base_url = match proxy_port {
            Some(port) if super::local_proxy::is_http_target(&entry.base_url) => {
                super::local_proxy::proxy_base_url(port, &entry.id)
            }
            _ => entry.base_url.clone(),
        };
        providers.insert(
            entry.id.clone(),
            json!({
                "name": entry.name,
                "base_url": effective_base_url,
                "model": entry.model,
                "models": models,
                // 占位凭据：本地端点不校验该头，但 Hermes 需要非空 Key 才把
                // 供应商视为已配置（与 LM Studio 免鉴权占位同一做法）。
                "key_env": LOCAL_PLACEHOLDER_KEY_ENV,
                // 列表形 models 在 Hermes 侧是 allowlist：目录只展示设置确认过的
                // 模型，与 MODEL-10 白名单语义一致；显式关闭探测避免打开选择器
                // 时阻塞在本地端点的 /models 请求上。
                "discover_models": false,
                "sophonote_managed": true,
            }),
        );
    }
    // 序列化文本比较（幂等检测）：序列化失败时按“有变化”处理，交给上层重写
    serde_json::to_string(root).is_ok_and(|current| current != before)
}

/// 主入口：读 settings.ai_config，把本地/免鉴权实例同步进 Hermes home 的
/// config.yaml。返回 true 表示配置内容变化（调用方据此决定是否重启 Runtime）。
pub fn sync_local_providers(app: &tauri::AppHandle) -> Result<bool, String> {
    let home =
        hermes_home().ok_or_else(|| format!("未找到 Hermes home：请设置 {ENV_HERMES_HOME}"))?;
    let config_path = home.join("config.yaml");
    let raw = if config_path.is_file() {
        fs::read_to_string(&config_path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut root: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_yaml::from_str(&raw).map_err(|e| format!("parse config.yaml: {e}"))?
    };

    let conn = rusqlite::Connection::open(crate::db::get_db_path(app))
        .map_err(|error| error.to_string())?;
    let raw_config: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'ai_config'",
            [],
            |row| row.get(0),
        )
        .ok();
    let ai_config: Value = raw_config
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();

    let entries = local_provider_sync_entries(&ai_config);

    // MODEL-11④：先登记代理目标并（按需）拉起 loopback 代理；代理不可用时
    // 回退直连（占位 Bearer 可能被严格端点拒绝，属降级而非阻断）。
    let targets: Vec<(String, String)> = entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.base_url.clone()))
        .collect();
    let proxied = super::local_proxy::set_proxy_targets(&targets);
    let proxy_port = if proxied.is_empty() {
        None
    } else {
        match super::local_proxy::ensure_proxy() {
            Ok(port) => Some(port),
            Err(error) => {
                eprintln!("[local-proxy] start failed, fallback direct: {error}");
                None
            }
        }
    };

    if !apply_local_provider_sync(&mut root, &entries, proxy_port) {
        return Ok(false);
    }
    let yaml = serde_yaml::to_string(&root).map_err(|e| format!("serialize config.yaml: {e}"))?;
    fs::write(&config_path, yaml).map_err(|e| e.to_string())?;
    eprintln!(
        "[hermes] synced {} SophoNote local provider(s) into {}",
        entries.len(),
        config_path.display()
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn upsert_writes_include_tools() {
        let dir = std::env::temp_dir().join(format!(
            "mb-hermes-home-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var(ENV_HERMES_HOME, &dir);
        let workspace = dir.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let (path, _) =
            configure_sophonote_surface("http://127.0.0.1:9/mcp", "tok", &workspace).unwrap();
        let url = read_bridge_url(&path).unwrap();
        assert!(url.contains("127.0.0.1"));
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("list_project_documents"));
        assert!(raw.contains("sophonote-bridge"));
        assert!(raw.contains("title_generation"));
        assert!(raw.contains("enabled: false"));
        assert!(raw.contains("workspace"));
        std::env::remove_var(ENV_HERMES_HOME);
        let _ = fs::remove_dir_all(&dir);
    }

    fn ai_config_with(providers: Value) -> Value {
        json!({ "activeProvider": "deepseek", "providers": providers })
    }

    #[test]
    fn sync_entries_keep_only_keyless_openai_instances() {
        let ai_config = ai_config_with(json!({
            "ollama": {
                "name": "Ollama 本地", "protocol": "openai",
                "baseUrl": "http://localhost:11434/v1", "model": "qwen3:8b",
                "models": ["qwen3:8b", "llama3.1"], "requiresKey": false
            },
            "deepseek": {
                "name": "DeepSeek", "protocol": "openai",
                "baseUrl": "https://api.deepseek.com/v1", "model": "deepseek-v4-pro",
                "models": ["deepseek-v4-pro"]
            },
            "private-openai": {
                "name": "私有化", "protocol": "openai",
                "baseUrl": "", "model": "", "models": [], "requiresKey": false
            },
            "local-anthropic": {
                "name": "本地 Anthropic", "protocol": "anthropic",
                "baseUrl": "http://localhost:9/v1", "model": "x",
                "models": ["x"], "requiresKey": false
            },
            "cloud-keyless-mistake": {
                "name": "云端误标免鉴权", "protocol": "openai",
                "baseUrl": "https://api.deepseek.com/v1", "model": "deepseek-chat",
                "models": ["deepseek-chat"], "requiresKey": false
            }
        }));
        let entries = local_provider_sync_entries(&ai_config);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "ollama");
        assert_eq!(entries[0].name, "Ollama 本地");
        assert_eq!(
            entries[0].models,
            vec!["qwen3:8b".to_string(), "llama3.1".to_string()]
        );
    }

    #[test]
    fn sync_entries_default_model_always_in_whitelist() {
        let ai_config = ai_config_with(json!({
            "ollama": {
                "protocol": "openai", "baseUrl": "http://localhost:11434/v1",
                "model": "qwen3:8b", "models": [], "requiresKey": false
            }
        }));
        let entries = local_provider_sync_entries(&ai_config);
        assert_eq!(entries[0].name, "ollama");
        assert_eq!(entries[0].models, vec!["qwen3:8b".to_string()]);
    }

    #[test]
    fn apply_upserts_managed_entries_and_preserves_foreign_config() {
        let mut root = json!({
            "providers": {
                "user-own": { "base_url": "https://corp.example/v1", "api_key": "kept" }
            },
            "model": { "provider": "deepseek" }
        });
        let entries = vec![LocalProviderSyncEntry {
            id: "ollama".into(),
            name: "Ollama 本地".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "qwen3:8b".into(),
            models: vec!["qwen3:8b".into()],
        }];

        assert!(apply_local_provider_sync(&mut root, &entries, None));
        let providers = &root["providers"];
        assert_eq!(providers["ollama"]["base_url"], "http://localhost:11434/v1");
        assert_eq!(providers["ollama"]["sophonote_managed"], true);
        assert_eq!(providers["ollama"]["discover_models"], false);
        assert_eq!(providers["ollama"]["key_env"], "SOPHONOTE_LOCAL_API_KEY");
        assert_eq!(providers["user-own"]["api_key"], "kept");
        // 幂等：内容一致时第二次同步不再报变化
        assert!(!apply_local_provider_sync(&mut root, &entries, None));
    }

    #[test]
    fn apply_rewrites_http_base_url_to_strip_proxy_and_keeps_https_direct() {
        let mut root = json!({});
        let entries = vec![
            LocalProviderSyncEntry {
                id: "ollama".into(),
                name: "Ollama 本地".into(),
                base_url: "http://localhost:11434/v1".into(),
                model: "qwen3:8b".into(),
                models: vec!["qwen3:8b".into()],
            },
            LocalProviderSyncEntry {
                id: "tls".into(),
                name: "私有 https".into(),
                base_url: "https://private.example/v1".into(),
                model: "m".into(),
                models: vec!["m".into()],
            },
        ];
        assert!(apply_local_provider_sync(&mut root, &entries, Some(1234)));
        let providers = &root["providers"];
        assert_eq!(
            providers["ollama"]["base_url"],
            "http://127.0.0.1:1234/mbp/ollama"
        );
        assert_eq!(providers["tls"]["base_url"], "https://private.example/v1");
    }

    #[test]
    fn apply_prunes_stale_managed_entries_only() {
        let mut root = json!({
            "providers": {
                "ollama": { "base_url": "http://old/v1", "sophonote_managed": true },
                "ollama-2": { "base_url": "http://old2/v1", "sophonote_managed": true },
                "user-own": { "base_url": "https://corp.example/v1" }
            }
        });
        // 设置里只剩 ollama-2（重命名端点后保存）：ollama 被清理，user-own 不动
        let entries = vec![LocalProviderSyncEntry {
            id: "ollama-2".into(),
            name: "Ollama 远程".into(),
            base_url: "http://new2/v1".into(),
            model: "qwen3:32b".into(),
            models: vec!["qwen3:32b".into()],
        }];
        assert!(apply_local_provider_sync(&mut root, &entries, None));
        let providers = root["providers"].as_object().unwrap();
        assert!(!providers.contains_key("ollama"));
        assert!(providers.contains_key("user-own"));
        assert_eq!(root["providers"]["ollama-2"]["base_url"], "http://new2/v1");
    }
}
