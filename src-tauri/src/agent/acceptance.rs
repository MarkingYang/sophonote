//! Executable Host contracts from the acceptance catalog. These are not model
//! quality or Tauri end-to-end evidence; the catalog states that boundary.
use super::super::{
    candidates, commands::AgentRunStartArgs, model_preparation, pi::tools, registry::EngineId,
    routing, runtime_observations::Observation, task_profile, web,
};
use crate::model::openai_compat::ProviderSnapshot;
use serde_json::{json, Value};
use std::{cell::Cell, path::PathBuf};
use tokio_util::sync::CancellationToken;

fn catalog() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/agent-routing-acceptance.json"
    ))
    .unwrap()
}
fn str_or<'a>(v: &'a Value, field: &str, fallback: &'a str) -> &'a str {
    v[field].as_str().unwrap_or(fallback)
}
fn provider(v: &Value) -> ProviderSnapshot {
    ProviderSnapshot {
        id: str_or(v, "providerId", "fixture-primary").into(),
        protocol: str_or(v, "protocol", "openai").into(),
        base_url: str_or(v, "baseUrl", "https://example.test/v1").into(),
        model: "default".into(),
        models: vec!["default".into(), "selected".into()],
        requires_key: v["requiresKey"].as_bool().unwrap_or(true),
    }
}

#[test]
fn catalog_has_eighty_distinct_bounded_scenarios_and_no_claim_of_host_completion() {
    let catalog = catalog();
    let cases = catalog["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 80);
    let mut ids = std::collections::HashSet::new();
    let mut targets = std::collections::HashSet::new();
    for case in cases {
        assert!(ids.insert(case["id"].as_str().unwrap()));
        assert!(targets.insert(case["contractTest"].as_str().unwrap()));
        assert_eq!(case["evidenceLevel"], "host_contract");
        assert!(!case["expected"]["sideEffects"].as_str().unwrap().is_empty());
        assert!(case.get("passed").is_none()); // evidence belongs to a fresh run
    }
    for group in ["A", "B", "C", "D", "E", "F", "G", "H"] {
        assert_eq!(cases.iter().filter(|c| c["group"] == group).count(), 10);
    }
}

fn route(input: &Value, expected: &Value) {
    let request: AgentRunStartArgs = serde_json::from_value(input["request"].clone()).unwrap();
    let original = serde_json::to_value(&request).unwrap();
    let bound = input["bound"].as_str();
    let route = routing::resolve_binding(None, bound, bound.is_some()).unwrap();
    let provider = provider(input);
    let candidates =
        candidates::screen(&request, &route, Some(&provider), |_| Observation::Unknown);
    let available = |engine: EngineId| {
        input["available"]
            .as_array()
            .is_none_or(|values| values.iter().any(|v| v == engine.as_str()))
    };
    let choice = super::choose(
        task_profile::infer(&request).preference(&provider.protocol),
        &candidates,
        available,
        input["hermesVerified"].as_bool().unwrap_or(true),
    );
    assert_eq!(choice.map(|e| e.as_str()), expected["selected"].as_str());
    if let Some(choice) = choice {
        candidates::validate_selected(&candidates, choice).unwrap();
        if let Some(bound) = bound {
            assert_eq!(choice.as_str(), bound);
        }
    }
    assert_eq!(serde_json::to_value(&request).unwrap(), original);
    let encoded = serde_json::to_string(&candidates).unwrap();
    assert!(!encoded.contains(&provider.base_url));
    assert!(!encoded.contains("fixture-primary"));
}

fn credential(input: &Value, expected: &Value) {
    let provider = provider(input);
    let id = provider.id.clone();
    let model = input["model"].as_str();
    let calls = Cell::new(0u32);
    let prepared = model_preparation::select(
        EngineId::parse(str_or(input, "engine", "pi")).unwrap(),
        provider,
        model,
    )
    .and_then(|selection| {
        selection.credentials(|actual_id| {
            calls.set(calls.get() + 1);
            assert_eq!(actual_id, id);
            match str_or(input, "credential", "valid") {
                "denied" => Err("synthetic secure-store failure; must be sanitized".into()),
                "empty" => Ok(String::new()),
                "blank" => Ok("   ".into()),
                "valid" => Ok("synthetic-fixture-value".into()),
                _ => panic!("unknown credential fixture"),
            }
        })
    });
    assert_eq!(u64::from(calls.get()), expected["reads"].as_u64().unwrap());
    match prepared {
        Ok(value) => {
            assert_eq!(expected["ok"], true);
            assert_eq!(value.model, model.unwrap_or("default"));
            assert_eq!(value.provider.id, id);
            if input["requiresKey"] == false {
                assert!(value.key.is_empty());
            }
        }
        Err(error) => assert_eq!(format!("{error:?}"), expected["error"].as_str().unwrap()),
    }
}

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/routing-acceptance")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("workspace/protected")).unwrap();
        for file in [
            "workspace/input.txt",
            "workspace/protected/data.txt",
            "outside.txt",
            "attachment.txt",
        ] {
            std::fs::write(root.join(file), "original fixture text").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("outside.txt"), root.join("workspace/escape.txt"))
            .unwrap();
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn scope(input: &Value, expected: &Value) {
    let files = Files::new();
    let scope = tools::Scope {
        read_lease: None,
        readonly: None,
        workspace: files.0.join("workspace"),
        read_paths: vec![files.0.join("attachment.txt")],
        workcopy: None,
        protected: vec![files.0.join("workspace/protected")],
        mode: input["mode"].as_str().unwrap().into(),
    };
    let mut request: tools::Request = serde_json::from_value(input["tool"].clone()).unwrap();
    if request.args["path"] == "{attachment}" {
        request.args["path"] = json!(files.0.join("attachment.txt"));
    }
    let validation = scope.validate(&request);
    assert_eq!(validation.is_ok(), expected["valid"].as_bool().unwrap());
    if let Ok(approval) = validation {
        assert_eq!(approval, expected["approval"].as_bool().unwrap());
        // Never skip the caller's approval contract for a mutating operation.
        if !approval && request.name == "read" {
            let text = tools::execute(&scope, &request, &CancellationToken::new())
                .await
                .unwrap();
            assert!(text.contains("original fixture text"));
        }
    } else {
        assert!(tools::execute(&scope, &request, &CancellationToken::new())
            .await
            .is_err());
    }
    assert!(!scope.workspace.join("output.txt").exists());
    for file in [
        "workspace/input.txt",
        "workspace/protected/data.txt",
        "outside.txt",
        "attachment.txt",
    ] {
        assert_eq!(
            std::fs::read_to_string(files.0.join(file)).unwrap(),
            "original fixture text"
        );
    }
}

async fn run(id: &str) {
    let catalog = catalog();
    let case = catalog["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap();
    let input = &case["input"];
    let expected = &case["expected"];
    match input["kind"].as_str().unwrap() {
        "route" => route(input, expected),
        "credential" => credential(input, expected),
        "scope" => scope(input, expected).await,
        "web" => {
            let request: tools::Request = serde_json::from_value(input["tool"].clone()).unwrap();
            assert_eq!(
                web::validate(&request).is_ok(),
                expected["valid"].as_bool().unwrap()
            );
            let cancelled = CancellationToken::new();
            cancelled.cancel();
            assert!(web::execute(&request, &cancelled).await.is_err());
        }
        other => panic!("unsupported fixture kind {other}"),
    }
}

macro_rules! cases {
    ($($name:ident => $id:literal),* $(,)?) => {$ (
        #[tokio::test]
        async fn $name() { run($id).await; }
    )*};
}
cases! {
    a01 => "A01", a02 => "A02", a03 => "A03", a04 => "A04", a05 => "A05",
    a06 => "A06", a07 => "A07", a08 => "A08", a09 => "A09", a10 => "A10",
    b01 => "B01", b02 => "B02", b03 => "B03", b04 => "B04", b05 => "B05",
    b06 => "B06", b07 => "B07", b08 => "B08", b09 => "B09", b10 => "B10",
    d01 => "D01", d02 => "D02", d03 => "D03", d04 => "D04", d05 => "D05",
    d06 => "D06", d07 => "D07", d08 => "D08", d09 => "D09", d10 => "D10",
    e01 => "E01", e02 => "E02", e03 => "E03", e04 => "E04", e05 => "E05",
    e06 => "E06", e07 => "E07", e08 => "E08", e09 => "E09", e10 => "E10",
    g01 => "G01", g02 => "G02", g03 => "G03", g04 => "G04", g05 => "G05",
    g06 => "G06", g07 => "G07", g08 => "G08", g09 => "G09", g10 => "G10",
}
