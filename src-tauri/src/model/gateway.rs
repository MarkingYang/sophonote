// ============================================================
// Track B · 智能体演进（docs/architecture.md Phase 0）
// ModelGateway：全应用唯一模型出口。普通摘要、日报、夜间解读、
// 未来 Agent 全部经此调用；项目内不允许第二套 /chat/completions 直连。
//
// Phase 0 范围：complete()（非流式）。
// stream() 随 Phase 1 事件协议（AgentEvent/AgentEventSink，§十一）一起加入，
// 避免在没有事件消费方时预建流式管道。
//
// AG-22（审计 P1-2 整改④）：§十二 错误策略的传输层落地——
// 网络超时=有限重试、429=退避重试；重试有上限、可取消，且只对
// 无副作用的模型补全请求生效（工具执行重试不在此层，驱动侧永不重放工具）。
// ============================================================
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::messages::{ModelError, ModelRequest, ModelResponse};

#[async_trait]
pub trait ModelGateway: Send + Sync {
    /// 单次非流式补全。cancel 用于传播用户取消（Phase 0 调用方传新建 token 即可）。
    async fn complete(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError>;
}

pub type SharedGateway = Arc<dyn ModelGateway>;

// ============================================================
// AG-22：有限重试（有上限、可取消、指数退避）
// ============================================================

/// 重试策略（模型补全专用；§十二：网络超时有限重试 / 429 退避重试）
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// 最大重试次数（不含首次尝试）。0 = 不重试
    pub max_retries: u32,
    /// 退避基数；第 n 次重试前等待 base * 2^n（指数退避）
    pub base_delay: Duration,
}

impl RetryPolicy {
    /// 全应用默认：最多重试 2 次（共 3 次尝试），500ms 起步指数退避
    /// （500ms → 1000ms）。最坏额外耗时 1.5s，远小于单次 120s 超时，
    /// 不会显著拉长 CompletionService 等低延迟路径的失败感知。
    pub fn model_default() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_millis(500),
        }
    }

    /// 第 attempt 次重试前的等待时长（attempt 从 0 起）
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        self.base_delay.saturating_mul(2u32.saturating_pow(attempt))
    }
}

/// 可重试错误判别（§十二 错误表）：网络层失败 + 429 限流。
/// 其余（Config/Parse/其它 HTTP 状态/Cancelled）一律不重试——
/// 4xx 重试无意义，5xx 供应商语义不一不擅自扩大，Cancelled 是用户意图。
pub fn is_retryable(err: &ModelError) -> bool {
    matches!(
        err,
        ModelError::Network(_) | ModelError::Http { status: 429, .. }
    )
}

/// 通用重试循环：对无副作用的单次尝试函数做有限重试。
/// 语义保证：
/// - 首次 + 至多 max_retries 次重试，耗尽返回最后一次的错误；
/// - 非可重试错误立即返回，不多跑一次；
/// - 退避等待期间 cancel 触发 → 立即返回 Cancelled（不空等退避）；
/// - 成功即返回，绝不重复执行已成功的尝试。
pub async fn with_retry<T, F, Fut>(
    policy: RetryPolicy,
    cancel: &CancellationToken,
    mut attempt: F,
) -> Result<T, ModelError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ModelError>>,
{
    let mut attempt_no: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(ModelError::Cancelled);
        }
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(err) => {
                let can_retry = is_retryable(&err) && attempt_no < policy.max_retries;
                if !can_retry {
                    return Err(err);
                }
                let delay = policy.delay_for_attempt(attempt_no);
                attempt_no += 1;
                // 退避期可取消：cancel 先到即返回，不空等 sleep
                tokio::select! {
                    _ = cancel.cancelled() => return Err(ModelError::Cancelled),
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 测试口径：退避基数压到 1ms，重试上限 2（共 3 次尝试），跑得快、断言面清晰
    fn test_policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
        }
    }

    fn ok_response() -> ModelResponse {
        ModelResponse {
            content: "ok".into(),
            reasoning: None,
            tool_calls: Vec::new(),
            finish_reason: super::super::messages::FinishReason::Stop,
            usage: Default::default(),
            provider_request_id: None,
        }
    }

    #[test]
    fn is_retryable_classification_follows_error_table() {
        assert!(is_retryable(&ModelError::Network("timeout".into())));
        assert!(is_retryable(&ModelError::Http {
            status: 429,
            body: "rate limited".into(),
            url: String::new(),
        }));
        // 其余一律不重试：4xx/5xx 语义不一不擅自扩大，Parse/Config 重试无意义
        assert!(!is_retryable(&ModelError::Http {
            status: 400,
            body: String::new(),
            url: String::new(),
        }));
        assert!(!is_retryable(&ModelError::Http {
            status: 500,
            body: String::new(),
            url: String::new(),
        }));
        assert!(!is_retryable(&ModelError::Parse("bad json".into())));
        assert!(!is_retryable(&ModelError::Config("no key".into())));
        assert!(!is_retryable(&ModelError::Cancelled));
    }

    #[test]
    fn delay_for_attempt_is_exponential() {
        let p = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
        };
        assert_eq!(p.delay_for_attempt(0), Duration::from_millis(500));
        assert_eq!(p.delay_for_attempt(1), Duration::from_millis(1000));
        assert_eq!(p.delay_for_attempt(2), Duration::from_millis(2000));
    }

    #[tokio::test]
    async fn first_try_success_runs_exactly_once() {
        let calls = AtomicU32::new(0);
        let cancel = CancellationToken::new();
        let res = with_retry(test_policy(), &cancel, || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(ok_response()))
        })
        .await;
        assert!(res.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retryable_errors_retried_until_success() {
        let calls = AtomicU32::new(0);
        let cancel = CancellationToken::new();
        let res = with_retry(test_policy(), &cancel, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(if n < 2 {
                Err(ModelError::Network("瞬态断连".into()))
            } else {
                Ok(ok_response())
            })
        })
        .await;
        assert!(res.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3); // 2 次失败 + 1 次成功
    }

    #[tokio::test]
    async fn retry_exhausted_returns_last_error_after_bounded_attempts() {
        let calls = AtomicU32::new(0);
        let cancel = CancellationToken::new();
        let res: Result<ModelResponse, ModelError> = with_retry(test_policy(), &cancel, || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err(ModelError::Http {
                status: 429,
                body: format!("第 {} 次限流", n),
                url: String::new(),
            }))
        })
        .await;
        // 上限钉死：首次 + 2 次重试 = 3 次尝试，不多跑
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        match res {
            Err(ModelError::Http {
                status: 429, body, ..
            }) => assert!(body.contains("第 2 次")),
            other => panic!("应为最后一次的 429 错误，实际 {:?}", other),
        }
    }

    #[tokio::test]
    async fn non_retryable_error_returns_immediately_without_retry() {
        let calls = AtomicU32::new(0);
        let cancel = CancellationToken::new();
        let res: Result<ModelResponse, ModelError> = with_retry(test_policy(), &cancel, || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err(ModelError::Http {
                status: 401,
                body: "unauthorized".into(),
                url: String::new(),
            }))
        })
        .await;
        assert!(matches!(res, Err(ModelError::Http { status: 401, .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 1); // 401 不重试
    }

    #[tokio::test]
    async fn cancel_during_backoff_returns_cancelled_without_waiting() {
        let calls = AtomicU32::new(0);
        let cancel = CancellationToken::new();
        let cancel_bg = cancel.clone();
        // 首次失败后退避期（基数 200ms）内取消：应立即返回 Cancelled
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_bg.cancel();
        });
        let slow_policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(200),
        };
        let start = std::time::Instant::now();
        let res: Result<ModelResponse, ModelError> = with_retry(slow_policy, &cancel, || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err(ModelError::Network("down".into())))
        })
        .await;
        assert!(matches!(res, Err(ModelError::Cancelled)));
        assert_eq!(calls.load(Ordering::SeqCst), 1); // 只跑了首次，退避中被取消
        assert!(start.elapsed() < Duration::from_millis(180)); // 未空等满退避
    }

    #[tokio::test]
    async fn already_cancelled_token_rejects_before_any_attempt() {
        let calls = AtomicU32::new(0);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let res: Result<ModelResponse, ModelError> = with_retry(test_policy(), &cancel, || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(ok_response()))
        })
        .await;
        assert!(matches!(res, Err(ModelError::Cancelled)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
