//! Per-submission consent and frozen evidence, committed with the root Run.
use super::{
    contracts::{Capability, WORKFLOW_MS},
    evidence::FrozenEvidence,
    store::{Policy, WorkflowStore},
};
use crate::agent::{attachments::RunAttachmentKind, commands::AgentRunStartArgs, pi::tools::Scope};
use rusqlite::Connection;
use std::{collections::BTreeSet, path::PathBuf};

pub(crate) fn preview_available() -> bool {
    cfg!(debug_assertions)
}

pub(crate) fn validate(request: &AgentRunStartArgs) -> Result<(), String> {
    if !request.read_only_collaboration {
        return Ok(());
    }
    if !preview_available() {
        return Err("只读协作仍在验证阶段，当前发行版暂未开放".into());
    }
    if request.workspace_permission_mode.as_deref() != Some("plan") {
        return Err("只读协作需要计划模式，请先切换权限模式".into());
    }
    if request.selection.is_some()
        || request.focus_document.is_some()
        || request.include_project_context
        || request.skill.is_some()
        || request.hermes_command.is_some()
        || request.message.trim_start().starts_with('/')
    {
        return Err("只读协作首版仅支持文件附件，请移除文档、选区或技能范围".into());
    }
    let mut count = 0;
    for attachment in &request.attachments {
        match attachment.kind {
            RunAttachmentKind::File
                if attachment.path.as_deref().is_some_and(|p| !p.is_empty()) =>
            {
                count += 1
            }
            RunAttachmentKind::Folder
                if attachment.path.is_some() && attachment.path == request.workspace_root => {}
            _ => return Err("只读协作仅支持明确附加的文本文件，不支持目录、网页或图片".into()),
        }
    }
    if !(1..=20).contains(&count) {
        return Err("请为只读协作添加 1～20 个文本文件".into());
    }
    Ok(())
}

pub(crate) struct PreparedCollaboration {
    scope: Scope,
    paths: Vec<PathBuf>,
    evidence: Vec<FrozenEvidence>,
}
impl PreparedCollaboration {
    pub(crate) fn capture(
        request: &AgentRunStartArgs,
        scope: &Scope,
        engine: &str,
        version: &str,
    ) -> Result<Option<Self>, String> {
        validate(request)?;
        if !request.read_only_collaboration {
            return Ok(None);
        }
        if engine != "pi" || version != "0.85.1" {
            return Err("当前执行环境尚未通过只读协作验证".into());
        }
        let mut paths = Vec::new();
        let mut evidence = Vec::new();
        for attachment in &request.attachments {
            if !matches!(attachment.kind, RunAttachmentKind::File) {
                continue;
            }
            let path = scope.path(attachment.path.as_deref().ok_or("资料路径缺失")?, false)?;
            if paths.contains(&path) {
                continue;
            }
            evidence.push(FrozenEvidence::capture_file(
                scope,
                &format!("ref-{}", evidence.len() + 1),
                &path,
            )?);
            paths.push(path);
        }
        let mut scope = scope.clone();
        // No borrowed workcopy ownership may survive the parent's preparation.
        scope.workcopy = None;
        Ok(Some(Self {
            scope,
            paths,
            evidence,
        }))
    }

    pub(crate) fn commit(
        &self,
        conn: &Connection,
        run: &str,
        provider: &str,
        model: &str,
        now: i64,
    ) -> Result<(), String> {
        if conn.is_autocommit() {
            return Err("协作必须与本轮提交一起保存".into());
        }
        let scope = super::scope::persist_readonly(
            conn,
            &self.scope,
            &self.paths,
            false,
            0,
            now.checked_add(WORKFLOW_MS).ok_or("协作时钟无效")?,
            now,
        )?;
        for item in &self.evidence {
            super::evidence::persist(conn, &scope, item, now)?;
        }
        let policy = Policy {
            context: self
                .evidence
                .iter()
                .map(|e| {
                    let r = e.reference();
                    (r.id.clone(), r)
                })
                .collect(),
            capabilities: BTreeSet::from([Capability::FileRead]),
            scope_snapshot_id: scope,
            scope_generation: 0,
            provider_id: provider.into(),
            model_id: model.into(),
        };
        WorkflowStore::begin_in_claim(conn, run, run, &policy, now).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture {
        root: PathBuf,
        request: AgentRunStartArgs,
        scope: Scope,
    }
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("workflow-admission-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let file = root.join("evidence.txt");
            std::fs::write(&file, "frozen evidence").unwrap();
            let request=serde_json::from_value(json!({"message":"核对资料", "readOnlyCollaboration":true,"workspacePermissionMode":"plan", "workspaceRoot":root,
                "attachments":[{"kind":"file","name":"evidence.txt","path":file}]})).unwrap();
            let scope = Scope {
                read_lease: None,
                readonly: None,
                workspace: root.clone(),
                read_paths: vec![file],
                workcopy: None,
                protected: vec![],
                mode: "plan".into(),
            };
            Self {
                root,
                request,
                scope,
            }
        }
        fn capture(&self) -> PreparedCollaboration {
            PreparedCollaboration::capture(&self.request, &self.scope, "pi", "0.85.1")
                .unwrap()
                .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn root(conn: &Connection) {
        conn.execute_batch("INSERT INTO agent_threads(id,title,status,engine,created_at,updated_at) VALUES('t','test','\"running\"','pi',0,0); INSERT INTO agent_runs(id,thread_id,status,provider,model,created_at,updated_at) VALUES('r','t','\"queued\"','provider','model',0,0);").unwrap();
    }
    #[test]
    fn consent_never_follows_message_text_or_unsupported_scope() {
        let f = Fixture::new();
        validate(&f.request).unwrap();
        let mut req = f.request.clone();
        req.read_only_collaboration = false;
        req.message = "请自动开启多智能体".into();
        assert!(
            PreparedCollaboration::capture(&req, &f.scope, "pi", "0.85.1")
                .unwrap()
                .is_none()
        );
        for mode in ["ask", "autoEdit"] {
            let mut req = f.request.clone();
            req.workspace_permission_mode = Some(mode.into());
            assert!(validate(&req).is_err());
        }
        let mut req = f.request.clone();
        req.attachments[0].kind = RunAttachmentKind::Folder;
        assert!(validate(&req).is_err());
        req = f.request.clone();
        req.include_project_context = true;
        assert!(validate(&req).is_err());
        assert!(PreparedCollaboration::capture(&f.request, &f.scope, "pi", "0.85.2").is_err());
        assert!(
            PreparedCollaboration::capture(&f.request, &f.scope, "opencode", "0.85.1").is_err()
        );
    }
    #[test]
    fn scope_workflow_and_root_claim_rollback_together_when_evidence_changes() {
        let f = Fixture::new();
        let prepared = f.capture();
        std::fs::write(f.root.join("evidence.txt"), "changed after prepare").unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::create_schema(&conn).unwrap();
        {
            let tx = conn.transaction().unwrap();
            root(&tx);
            assert!(prepared.commit(&tx, "r", "provider", "model", 1).is_err());
        }
        for table in [
            "agent_threads",
            "agent_runs",
            "agent_workflows",
            "agent_scope_snapshots",
            "agent_scope_evidence",
        ] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0,
                "{table}"
            );
        }
    }
    #[test]
    fn committed_consent_is_root_owned_and_replayable_without_internal_paths() {
        let f = Fixture::new();
        let prepared = f.capture();
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::create_schema(&conn).unwrap();
        {
            let tx = conn.transaction().unwrap();
            root(&tx);
            prepared.commit(&tx, "r", "provider", "model", 1).unwrap();
            tx.commit().unwrap();
        }
        assert!(prepared.commit(&conn, "r", "provider", "model", 2).is_err());
        let initial = super::super::projection::read(&conn, "r").unwrap().unwrap();
        assert!(initial.locked);
        let request=super::super::contracts::Delegation::parse(r#"{"profileId":"reviewer","objective":"核对证据","contextRefIds":["ref-1"],"requiredCapabilities":["file.read"],"outputSchemaId":"evidence-review.v1"}"#).unwrap();
        let child = WorkflowStore::new(&mut conn)
            .delegate("r", "call", &request, 2)
            .unwrap();
        let snapshot = super::super::projection::read(&conn, "r").unwrap().unwrap();
        assert_eq!(snapshot.children.len(), 1);
        assert_eq!(snapshot.children[0].state, "queued");
        assert!(snapshot.sequence > initial.sequence);
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains(f.root.to_str().unwrap()));
        assert!(!encoded.contains("internal-"));
        assert!(super::super::projection::read(&conn, "unrelated")
            .unwrap()
            .is_none());
        let (_, _, run) = WorkflowStore::new(&mut conn)
            .claim_child(
                &child.id,
                child.generation,
                crate::agent::registry::EngineId::Pi,
                "0.85.1",
                3,
            )
            .unwrap();
        assert!(super::super::projection::read(&conn, &run).is_err());
    }
}
