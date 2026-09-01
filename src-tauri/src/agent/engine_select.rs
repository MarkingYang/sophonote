//! DEC-019：产品 Agent 执行平面固定为 Hermes。
//! `agent.engine` 设置与 Rig 产品回退均已移除；Hermes 未就绪时明确失败。

use crate::agent::hermes::gateway_env_configured;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineChoice {
    Hermes,
}

impl EngineChoice {
    pub fn engine_id(self) -> &'static str {
        "hermes"
    }
}

/// 运行时就绪结果：Hermes 可用，或明确不可用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineResolve {
    Use(EngineChoice),
    /// Hermes 附着未配置 / 未就绪
    Unavailable {
        reason: String,
    },
}

pub fn resolve_engine(hermes_ready: bool) -> EngineResolve {
    if hermes_ready {
        EngineResolve::Use(EngineChoice::Hermes)
    } else {
        EngineResolve::Unavailable {
            reason: format!(
                "Hermes Agent 未连接：请设置 {} 与 {}",
                crate::agent::hermes::ENV_GATEWAY_URL,
                crate::agent::hermes::ENV_GATEWAY_TOKEN
            ),
        }
    }
}

/// 生产/开发就绪探测：正式 Surface Gateway env 齐备即 true；否则检查钉扎二进制路径。
pub fn probe_hermes_production_health() -> bool {
    if gateway_env_configured() {
        return true;
    }
    let Ok(path) = std::env::var("SOPHONOTE_HERMES_BIN") else {
        return false;
    };
    let p = std::path::Path::new(&path);
    p.is_file()
}

pub fn is_engine_unavailable(resolve: &EngineResolve) -> bool {
    matches!(resolve, EngineResolve::Unavailable { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_not_ready_is_unavailable() {
        assert_eq!(
            resolve_engine(false),
            EngineResolve::Unavailable {
                reason: format!(
                    "Hermes Agent 未连接：请设置 {} 与 {}",
                    crate::agent::hermes::ENV_GATEWAY_URL,
                    crate::agent::hermes::ENV_GATEWAY_TOKEN
                ),
            }
        );
        assert_eq!(
            resolve_engine(true),
            EngineResolve::Use(EngineChoice::Hermes)
        );
    }

    #[test]
    fn unavailable_when_hermes_preferred_but_not_ready() {
        let r = resolve_engine(false);
        assert!(is_engine_unavailable(&r));
        let r = resolve_engine(true);
        assert!(!is_engine_unavailable(&r));
    }
}
