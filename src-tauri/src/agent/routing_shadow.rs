//! Opt-in diagnostic routing. Suggestions are for preparation only, never dispatch.
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::candidates::Candidate;
use super::pi::ExecutionRuntime;
use super::registry::EngineId;

const DEADLINE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProbeState {
    RuntimeValidated,
    CachedRuntimeValidated,
    GatewayConnected,
    Unavailable,
    TimedOut,
    Busy,
    Excluded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Probe {
    engine: EngineId,
    state: ProbeState,
    version: Option<String>,
    elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Report {
    policy_version: String,
    /// This is NOT an eligible engine or a dispatch authorization.
    pub(crate) recommended_preparation_engine: Option<EngineId>,
    automatic_execution: bool,
    probes: Vec<Probe>,
    elapsed_ms: u64,
}

impl Report {
    pub(crate) fn available(&self, engine: EngineId) -> bool {
        self.probes.iter().any(|p| {
            p.engine == engine
                && matches!(
                    p.state,
                    ProbeState::RuntimeValidated
                        | ProbeState::CachedRuntimeValidated
                        | ProbeState::GatewayConnected
                )
        })
    }
}

pub(crate) fn enabled() -> bool {
    cfg!(debug_assertions) && std::env::var("SOPHONOTE_ROUTE_SHADOW").as_deref() == Ok("1")
}

fn gate(engine: EngineId) -> Arc<Semaphore> {
    static GATES: OnceLock<[Arc<Semaphore>; 4]> = OnceLock::new();
    let gates = GATES.get_or_init(|| std::array::from_fn(|_| Arc::new(Semaphore::new(1))));
    let index = match engine {
        EngineId::Hermes => 0,
        EngineId::Pi => 1,
        EngineId::ClaudeCode => 2,
        EngineId::Opencode => 3,
    };
    gates[index].clone()
}

async fn bounded<T, F: Future<Output = Result<T, ProbeState>>>(
    gate: Arc<Semaphore>,
    deadline: Duration,
    work: impl FnOnce(OwnedSemaphorePermit) -> F,
) -> Result<T, ProbeState> {
    let permit = gate.try_acquire_owned().map_err(|_| ProbeState::Busy)?;
    tokio::time::timeout(deadline, work(permit))
        .await
        .map_err(|_| ProbeState::TimedOut)?
}

enum Evidence {
    Gateway,
    Native(ExecutionRuntime),
}

async fn gateway_ready(endpoint: &super::hermes::HermesGatewayEndpoint) -> Result<(), ProbeState> {
    // Connecting validates the native gateway.ready first frame. No RPC, Session,
    // model query, user input, or installation is part of this probe.
    super::hermes::gateway_client::HermesGatewayConnection::connect(endpoint)
        .await
        .map(|_| ())
        .map_err(|_| ProbeState::Unavailable)
}

async fn native_evidence(
    app: tauri::AppHandle,
    engine: EngineId,
    permit: OwnedSemaphorePermit,
) -> Result<Evidence, ProbeState> {
    match engine {
        EngineId::Hermes => {
            let _permit = permit;
            let endpoint =
                super::hermes::HermesGatewayEndpoint::from_env().ok_or(ProbeState::Unavailable)?;
            gateway_ready(&endpoint).await?;
            Ok(Evidence::Gateway)
        }
        EngineId::ClaudeCode => {
            let _permit = permit;
            let _update = super::runtime_updates::CLAUDE_GATE
                .try_lock()
                .map_err(|_| ProbeState::Busy)?;
            super::claude::runtime::locate()
                .await
                .map(|runtime| Evidence::Native(ExecutionRuntime::Claude(runtime)))
                .map_err(|_| ProbeState::Unavailable)
        }
        EngineId::Pi | EngineId::Opencode => {
            // spawn_blocking cannot be aborted once hashing starts. Its permit
            // stays in the worker until it REALLY exits, including after timeout.
            // The worker never publishes results or changes the observation cache.
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                match engine {
                    EngineId::Pi => super::pi::update::resolve(&app)
                        .map(|(runtime, _)| Evidence::Native(ExecutionRuntime::Pi(runtime))),
                    EngineId::Opencode => super::opencode::update::resolve(&app)
                        .map(|(runtime, _)| Evidence::Native(ExecutionRuntime::OpenCode(runtime))),
                    _ => unreachable!(),
                }
                .map_err(|_| ProbeState::Unavailable)
            })
            .await
            .map_err(|_| ProbeState::Unavailable)?
        }
    }
}

async fn probe(app: &tauri::AppHandle, candidate: &Candidate) -> Probe {
    let start = Instant::now();
    let engine = candidate.engine;
    let mut result = Probe {
        engine,
        state: ProbeState::Excluded,
        version: None,
        elapsed_ms: 0,
    };
    if !candidate.can_probe() {
        return result;
    }
    if let super::runtime_observations::Observation::Verified { version, .. } =
        super::runtime_observations::get(&crate::db::get_db_path(app), engine)
    {
        result.state = ProbeState::CachedRuntimeValidated;
        result.version = Some(version);
    } else {
        let handle = app.clone();
        match bounded(gate(engine), DEADLINE, |permit| {
            native_evidence(handle, engine, permit)
        })
        .await
        {
            Ok(Evidence::Gateway) => result.state = ProbeState::GatewayConnected,
            Ok(Evidence::Native(runtime)) => {
                result.state = ProbeState::RuntimeValidated;
                result.version = Some(runtime.version().to_owned());
                // Only results returned before the deadline can warm the cache.
                super::pi::record_runtime_observation(app, &runtime);
            }
            Err(state) => result.state = state,
        }
    }
    result.elapsed_ms = start.elapsed().as_millis() as u64;
    result
}

fn recommend(candidates: &[Candidate], probes: &[Probe]) -> Option<EngineId> {
    // general-v1 is a stable baseline, not a claim that an engine is best.
    // Caller order and completion timing cannot alter the rule result.
    EngineId::ALL.into_iter().find(|engine| {
        candidates
            .iter()
            .any(|c| c.engine == *engine && c.can_recommend_preparation())
            && probes.iter().any(|p| {
                p.engine == *engine
                    && matches!(
                        p.state,
                        ProbeState::RuntimeValidated | ProbeState::CachedRuntimeValidated
                    )
            })
    })
}

pub(crate) async fn evaluate(app: &tauri::AppHandle, candidates: &[Candidate]) -> Report {
    let start = Instant::now();
    let probes = futures_util::future::join_all(candidates.iter().map(|c| probe(app, c))).await;
    Report {
        policy_version: "shadow-general-v1".into(),
        recommended_preparation_engine: recommend(candidates, &probes),
        automatic_execution: false,
        probes,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        candidates::screen, commands::AgentRunStartArgs, routing::resolve_binding,
        runtime_observations::Observation,
    };
    use super::*;
    use crate::model::openai_compat::ProviderSnapshot;

    fn candidates(engine: Option<&str>) -> Vec<Candidate> {
        let req: AgentRunStartArgs =
            serde_json::from_value(serde_json::json!({"message":"private input", "engine":engine}))
                .unwrap();
        let route = resolve_binding(engine, None, false).unwrap();
        let provider = ProviderSnapshot {
            id: "private provider".into(),
            protocol: "anthropic".into(),
            base_url: "https://private.endpoint".into(),
            model: "model".into(),
            models: vec!["model".into()],
            requires_key: true,
        };
        screen(&req, &route, Some(&provider), |_| Observation::Unknown)
    }

    fn result(engine: EngineId, state: ProbeState) -> Probe {
        Probe {
            engine,
            state,
            version: None,
            elapsed_ms: 0,
        }
    }

    #[test]
    #[ignore = "requires bundled Pi/OpenCode resources; validates real checksum probe budget"]
    fn bundled_runtime_integrity_fits_probe_deadline() {
        let resources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let results = std::thread::scope(|scope| {
            let pi = scope.spawn(|| {
                let start = Instant::now();
                super::super::pi::runtime::verify(
                    &resources
                        .join("pi")
                        .join(super::super::pi::runtime::target()),
                )
                .expect("bundled Pi integrity");
                start.elapsed()
            });
            let opencode = scope.spawn(|| {
                let start = Instant::now();
                super::super::opencode::runtime::verify(
                    &resources
                        .join("opencode")
                        .join(super::super::opencode::runtime::target()),
                )
                .expect("bundled OpenCode integrity");
                start.elapsed()
            });
            [
                ("Pi", pi.join().unwrap()),
                ("OpenCode", opencode.join().unwrap()),
            ]
        });
        for (engine, elapsed) in results {
            eprintln!("{engine} complete integrity check: {elapsed:?}");
            assert!(
                elapsed < DEADLINE,
                "{engine} checksum exceeds routing probe deadline: {elapsed:?}"
            );
        }
    }

    #[test]
    fn zero_one_many_candidates_and_timing_independent_preferences() {
        let candidates = candidates(None);
        assert_eq!(recommend(&candidates, &[]), None);
        assert_eq!(
            recommend(
                &candidates,
                &[result(EngineId::Opencode, ProbeState::RuntimeValidated)]
            ),
            Some(EngineId::Opencode)
        );
        let mut probes = vec![
            result(EngineId::Opencode, ProbeState::RuntimeValidated),
            result(EngineId::Pi, ProbeState::CachedRuntimeValidated),
        ];
        assert_eq!(recommend(&candidates, &probes), Some(EngineId::Pi));
        probes.reverse();
        assert_eq!(recommend(&candidates, &probes), Some(EngineId::Pi));
    }

    #[test]
    fn explicit_choice_failure_never_suggests_another_engine() {
        let rows = candidates(Some("claude_code"));
        for state in [
            ProbeState::TimedOut,
            ProbeState::Unavailable,
            ProbeState::Busy,
        ] {
            let probes = vec![
                result(EngineId::ClaudeCode, state),
                result(EngineId::Pi, ProbeState::RuntimeValidated),
            ];
            assert_eq!(recommend(&rows, &probes), None);
        }
        // Gateway reachability is not a native model contract.
        assert_eq!(
            recommend(
                &candidates(Some("hermes")),
                &[result(EngineId::Hermes, ProbeState::GatewayConnected)]
            ),
            None
        );
    }

    #[test]
    fn missing_model_evidence_and_bound_session_are_not_overridden_by_runtime_health() {
        let req: AgentRunStartArgs =
            serde_json::from_value(serde_json::json!({"message":"hello"})).unwrap();
        let probes = EngineId::ALL
            .into_iter()
            .map(|e| result(e, ProbeState::RuntimeValidated))
            .collect::<Vec<_>>();
        let unknown = screen(
            &req,
            &resolve_binding(None, None, false).unwrap(),
            None,
            |_| Observation::Unknown,
        );
        assert_eq!(recommend(&unknown, &probes), None);
        let provider = ProviderSnapshot {
            id: "fixture".into(),
            protocol: "anthropic".into(),
            base_url: "https://example.test".into(),
            model: "model".into(),
            models: vec!["model".into()],
            requires_key: true,
        };
        let bound = screen(
            &req,
            &resolve_binding(None, Some("opencode"), true).unwrap(),
            Some(&provider),
            |_| Observation::Unknown,
        );
        assert_eq!(recommend(&bound, &probes), Some(EngineId::Opencode));
    }

    #[tokio::test]
    async fn deadline_drops_async_work_and_releases_permit() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let gate = Arc::new(Semaphore::new(1));
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let result: Result<(), _> = bounded(
            gate.clone(),
            Duration::from_millis(10),
            |permit| async move {
                let _guard = guard;
                let _permit = permit;
                std::future::pending().await
            },
        )
        .await;
        assert_eq!(result, Err(ProbeState::TimedOut));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(gate.available_permits(), 1);
    }

    #[tokio::test]
    async fn timed_out_blocking_work_keeps_slot_and_late_result_is_not_returned() {
        let gate = Arc::new(Semaphore::new(1));
        let (release, wait) = std::sync::mpsc::channel();
        let (started, has_started) = tokio::sync::oneshot::channel();
        let (finished, has_finished) = tokio::sync::oneshot::channel();
        let work_gate = gate.clone();
        let task = tokio::spawn(async move {
            bounded(work_gate, Duration::from_millis(100), |permit| async move {
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let _ = started.send(());
                    let _ = wait.recv_timeout(Duration::from_secs(2));
                    drop(_permit);
                    let _ = finished.send(());
                    Ok(42)
                })
                .await
                .unwrap()
            })
            .await
        });
        has_started.await.unwrap();
        assert_eq!(task.await.unwrap(), Err(ProbeState::TimedOut));
        assert_eq!(
            bounded(gate.clone(), DEADLINE, |_| async { Ok(1) }).await,
            Err(ProbeState::Busy)
        );
        release.send(()).unwrap();
        has_finished.await.unwrap();
        assert_eq!(gate.available_permits(), 1);
    }

    #[tokio::test]
    async fn independent_probes_progress_concurrently() {
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let tasks = (0..3).map(|_| {
            let barrier = barrier.clone();
            bounded(
                Arc::new(Semaphore::new(1)),
                Duration::from_secs(1),
                |permit| async move {
                    let _permit = permit;
                    barrier.wait().await;
                    Ok(())
                },
            )
        });
        assert!(futures_util::future::join_all(tasks)
            .await
            .into_iter()
            .all(|r| r.is_ok()));
    }

    #[tokio::test]
    async fn hermes_probe_validates_ready_without_sending_any_rpc() {
        use futures_util::{SinkExt, StreamExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = super::super::hermes::HermesGatewayEndpoint::bundled(
            format!("ws://{}/api/ws", listener.local_addr().unwrap()),
            "fixture".into(),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::json!({"params":{"type":"gateway.ready"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            match ws.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(_))) => {
                    panic!("probe sent an RPC")
                }
                _ => true,
            }
        });
        assert!(tokio::time::timeout(DEADLINE, gateway_ready(&endpoint))
            .await
            .unwrap()
            .is_ok());
        assert!(tokio::time::timeout(DEADLINE, server)
            .await
            .unwrap()
            .unwrap());
    }

    #[test]
    fn report_has_no_task_provider_path_or_endpoint_and_never_claims_execution() {
        let rows = candidates(None);
        let probes = vec![result(EngineId::Pi, ProbeState::RuntimeValidated)];
        let report = Report {
            policy_version: "shadow-general-v1".into(),
            recommended_preparation_engine: recommend(&rows, &probes),
            automatic_execution: false,
            probes,
            elapsed_ms: 0,
        };
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["automaticExecution"], false);
        assert_eq!(json["recommendedPreparationEngine"], "pi");
        assert!(!json.to_string().contains("private"));
    }
}
