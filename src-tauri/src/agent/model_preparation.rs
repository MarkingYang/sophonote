//! Shared metadata validation and selected-provider credential acquisition.
//! This is the model part of prepare, not a complete dispatch authorization.
use crate::model::openai_compat::ProviderSnapshot;

use super::registry::EngineId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparationError {
    NativeCatalogRequired,
    UnsupportedProtocol,
    ModelNotConfigured,
    CredentialMissing,
    CredentialUnavailable,
}

impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NativeCatalogRequired => "Hermes 需要通过原生模型目录核验所选供应商与模型",
            Self::UnsupportedProtocol => "所选引擎不支持当前供应商协议，请检查模型设置",
            Self::ModelNotConfigured => "所选模型不在供应商配置白名单中，请检查模型设置",
            Self::CredentialMissing => "请先在模型设置中保存所选供应商的 API Key",
            Self::CredentialUnavailable => {
                "无法读取所选供应商凭据，请完成系统授权或在模型设置中重新保存凭据"
            }
        })
    }
}

/// Owns the resolved provider snapshot. No credential has been read yet.
/// Deliberately no Debug/Serialize: endpoint configuration is not route audit data.
pub(crate) struct Selection {
    provider: ProviderSnapshot,
    model: String,
}

/// A short-lived value for the selected adapter, never a persisted route DTO.
/// No Clone/Debug/Serialize so credentials cannot accidentally enter route logs.
pub(crate) struct PreparedModel {
    pub(crate) provider: ProviderSnapshot,
    pub(crate) model: String,
    pub(crate) key: String,
}

/// Both candidate screening and actual preparation must use this function.
/// It never reads credentials, defaults to another provider, or probes an endpoint.
pub(crate) fn select(
    engine: EngineId,
    provider: ProviderSnapshot,
    requested_model: Option<&str>,
) -> Result<Selection, PreparationError> {
    let provider = match engine {
        EngineId::Hermes => return Err(PreparationError::NativeCatalogRequired),
        EngineId::ClaudeCode => super::claude::provider_snapshot(provider)
            .ok_or(PreparationError::UnsupportedProtocol)?,
        EngineId::Pi | EngineId::Opencode => {
            if !matches!(provider.protocol.as_str(), "openai" | "anthropic") {
                return Err(PreparationError::UnsupportedProtocol);
            }
            provider
        }
    };
    let model = requested_model.unwrap_or(&provider.model);
    if model.trim().is_empty() || !provider.supports_model(model) {
        return Err(PreparationError::ModelNotConfigured);
    }
    Ok(Selection {
        model: model.to_owned(),
        provider,
    })
}

impl Selection {
    /// Exactly one read for a keyed provider; zero reads for a keyless provider.
    /// Failure is sanitized here; the credential service owns any diagnostics.
    pub(crate) fn credentials(
        self,
        read: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<PreparedModel, PreparationError> {
        let key = if self.provider.requires_key {
            let key =
                read(&self.provider.id).map_err(|_| PreparationError::CredentialUnavailable)?;
            if key.trim().is_empty() {
                return Err(PreparationError::CredentialMissing);
            }
            key
        } else {
            String::new()
        };
        Ok(PreparedModel {
            provider: self.provider,
            model: self.model,
            key,
        })
    }
}

pub(crate) fn prepare(
    app: &tauri::AppHandle,
    engine: EngineId,
    provider_override: Option<&str>,
    requested_model: Option<&str>,
) -> Result<PreparedModel, String> {
    let provider = crate::model::openai_compat::resolve_provider(app, provider_override)
        .map_err(|e| e.to_string())?;
    select(engine, provider, requested_model)
        .and_then(|selection| {
            selection.credentials(|id| crate::commands::get_cached_api_key(app, id))
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn provider(protocol: &str, url: &str, requires_key: bool) -> ProviderSnapshot {
        ProviderSnapshot {
            id: "provider-instance-2".into(),
            protocol: protocol.into(),
            base_url: url.into(),
            model: "default".into(),
            models: vec!["default".into(), "selected".into()],
            requires_key,
        }
    }

    #[test]
    fn keyless_providers_never_read_historical_keys_or_keychain() {
        for engine in [EngineId::Pi, EngineId::ClaudeCode, EngineId::Opencode] {
            let selected = select(
                engine,
                provider("anthropic", "http://localhost:1234", false),
                None,
            )
            .unwrap();
            let prepared = selected
                .credentials(|_| panic!("keyless provider accessed credentials"))
                .unwrap();
            assert!(prepared.key.is_empty());
            assert_eq!(prepared.model, "default");
        }
    }

    #[test]
    fn explicit_model_and_provider_instance_are_preserved_with_one_credential_read() {
        let calls = Cell::new(0);
        let selected = select(
            EngineId::Pi,
            provider("openai", "https://example.test/v1", true),
            Some("selected"),
        )
        .unwrap();
        let prepared = selected
            .credentials(|id| {
                calls.set(calls.get() + 1);
                assert_eq!(id, "provider-instance-2");
                Ok("fixture-key".into())
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(prepared.model, "selected");
        assert_eq!(prepared.provider.id, "provider-instance-2");
        assert_eq!(prepared.provider.base_url, "https://example.test/v1");
        assert_eq!(prepared.key, "fixture-key");
    }

    #[test]
    fn missing_and_denied_credentials_are_distinct_and_never_become_keyless() {
        for value in ["", "  "] {
            let result = select(
                EngineId::Pi,
                provider("openai", "https://example.test", true),
                None,
            )
            .unwrap()
            .credentials(|_| Ok(value.into()));
            assert!(matches!(result, Err(PreparationError::CredentialMissing)));
        }
        let result = select(
            EngineId::Pi,
            provider("openai", "https://example.test", true),
            None,
        )
        .unwrap()
        .credentials(|_| Err("sensitive backend detail".into()));
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("credential failure was ignored"),
        };
        assert_eq!(error, PreparationError::CredentialUnavailable);
        assert!(!error.to_string().contains("sensitive"));
    }

    #[test]
    fn invalid_or_empty_model_is_rejected_before_credential_access() {
        let calls = Cell::new(0);
        for model in ["missing", "", " "] {
            let result = select(
                EngineId::Opencode,
                provider("openai", "https://example.test", true),
                Some(model),
            )
            .and_then(|s| {
                s.credentials(|_| {
                    calls.set(calls.get() + 1);
                    Ok("key".into())
                })
            });
            assert!(matches!(result, Err(PreparationError::ModelNotConfigured)));
        }
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn claude_mapping_only_changes_the_verified_official_protocol_endpoint() {
        let selected = select(
            EngineId::ClaudeCode,
            provider("openai", "https://api.deepseek.com/v1", false),
            Some("selected"),
        )
        .unwrap();
        let prepared = selected
            .credentials(|_| panic!("unexpected key read"))
            .unwrap();
        assert_eq!(prepared.provider.id, "provider-instance-2");
        assert_eq!(prepared.model, "selected");
        assert_eq!(prepared.provider.protocol, "anthropic");
        assert_eq!(
            prepared.provider.base_url,
            "https://api.deepseek.com/anthropic"
        );
        for url in [
            "https://api.deepseek.com.evil.test/v1",
            "https://proxy.example/v1",
            "http://api.deepseek.com/v1",
        ] {
            assert!(matches!(
                select(EngineId::ClaudeCode, provider("openai", url, true), None),
                Err(PreparationError::UnsupportedProtocol)
            ));
        }
    }

    #[test]
    fn protocol_matrix_and_hermes_native_catalog_fail_closed() {
        for engine in [EngineId::Pi, EngineId::ClaudeCode, EngineId::Opencode] {
            assert!(select(
                engine,
                provider("anthropic", "https://example.test", false),
                None
            )
            .is_ok());
            assert!(matches!(
                select(
                    engine,
                    provider("unknown", "https://example.test", false),
                    None
                ),
                Err(PreparationError::UnsupportedProtocol)
            ));
        }
        assert!(matches!(
            select(
                EngineId::Hermes,
                provider("anthropic", "https://example.test", false),
                None
            ),
            Err(PreparationError::NativeCatalogRequired)
        ));
    }
}
