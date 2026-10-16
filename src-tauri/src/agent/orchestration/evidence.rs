//! Host-owned, versioned evidence. Model references never become filesystem paths.
use super::contracts::{Capability, ContextRef};
use crate::agent::pi::tools::{Scope, FILE_LIMIT};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenEvidence {
    reference: ContextRef,
    path: PathBuf,
    digest: String,
    document: Option<DocumentIdentity>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentIdentity {
    id: String,
    project: String,
    version: i64,
}

pub fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_scope_evidence (
        scope_id TEXT NOT NULL, ref_id TEXT NOT NULL, binding_json TEXT NOT NULL,
        PRIMARY KEY(scope_id,ref_id));
        CREATE TRIGGER IF NOT EXISTS immutable_agent_evidence BEFORE UPDATE ON agent_scope_evidence
        BEGIN SELECT RAISE(ABORT,'Evidence bindings are immutable'); END;",
    )
}
fn sql_error(_: rusqlite::Error) -> String {
    "无法核对资料引用".into()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    // Reject symlink replacement instead of reading the new target and comparing afterwards.
    if path.canonicalize().map_err(|_| "资料已不可读取")? != path {
        return Err("资料路径已变化".into());
    }
    let file = std::fs::File::open(path).map_err(|_| "资料已不可读取")?;
    if !file.metadata().map_err(|_| "资料元数据不可读取")?.is_file() {
        return Err("证据必须是具体文件，目录不是正文证据".into());
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "资料读取失败")?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err("单项资料超过 1 MiB".into());
    }
    if std::str::from_utf8(&bytes).is_err() {
        return Err("资料不是 UTF-8 文本".into());
    }
    Ok(bytes)
}
fn document_version(conn: &Connection, doc: &DocumentIdentity) -> Result<()> {
    let visible: Option<i64> = conn.query_row(
        "SELECT a.version FROM articles a JOIN project_documents p ON p.article_id=a.id WHERE a.id=?1 AND p.project_id=?2",
        params![doc.id,doc.project], |row| row.get(0)).optional().map_err(sql_error)?;
    if visible != Some(doc.version) {
        return Err("文档版本或项目可见范围已变化".into());
    }
    Ok(())
}
impl FrozenEvidence {
    pub fn reference(&self) -> ContextRef {
        self.reference.clone()
    }

    /// Called only with the already-authorized parent scope and a Host-generated ID.
    pub fn capture_file(scope: &Scope, id: &str, path: &Path) -> Result<Self> {
        if !valid_id(id) {
            return Err("资料引用标识无效".into());
        }
        let path = scope.path(path.to_str().ok_or("资料路径编码无效")?, false)?;
        let bytes = read_bytes(&path)?;
        let hash = digest(&bytes);
        Ok(Self {
            reference: ContextRef {
                id: id.into(),
                capability: Capability::FileRead,
                version: format!("sha256:{hash}"),
            },
            path,
            digest: hash,
            document: None,
        })
    }

    /// The caller supplies an explicitly selected document, not a model-provided path.
    /// Membership and the disk source are checked even when SQLite has legacy content.
    pub fn capture_document(
        conn: &Connection,
        notes: &Path,
        project: &str,
        article: &str,
        id: &str,
    ) -> Result<Self> {
        if !valid_id(id) || article.is_empty() || crate::notes::safe_article_id(article) != article
        {
            return Err("文档引用标识无效".into());
        }
        let _guard = crate::documents::repository::document_lock(article);
        let version: i64 = conn.query_row("SELECT a.version FROM articles a JOIN project_documents p ON p.article_id=a.id WHERE a.id=?1 AND p.project_id=?2",params![article,project],|row|row.get(0)).map_err(|_| "文档不在当前资料范围内")?;
        let root = notes.canonicalize().map_err(|_| "笔记目录不可用")?;
        let path = root.join(format!("{article}.md"));
        let bytes = read_bytes(&path)?;
        let hash = digest(&bytes);
        Ok(Self {
            reference: ContextRef {
                id: id.into(),
                capability: Capability::DocumentRead,
                version: format!("document:{version}:sha256:{hash}"),
            },
            path,
            digest: hash,
            document: Some(DocumentIdentity {
                id: article.into(),
                project: project.into(),
                version,
            }),
        })
    }
    fn validate_bytes(&self, conn: &Connection, bytes: &[u8]) -> Result<()> {
        if let Some(doc) = &self.document {
            document_version(conn, doc)?;
        }
        if digest(bytes) != self.digest {
            return Err("资料正文已变化，请重新确认资料后执行".into());
        }
        Ok(())
    }
}

fn scope(conn: &Connection, id: &str, now: i64) -> Result<Scope> {
    let json: String = conn.query_row("SELECT scope_json FROM agent_scope_snapshots WHERE id=?1 AND revoked=0 AND created_at<=?2 AND deadline_ms>?2",params![id,now],|row|row.get(0)).map_err(|_| "资料授权已撤销或到期")?;
    let scope: Scope = serde_json::from_str(&json).map_err(|_| "资料范围格式无效")?;
    if scope.mode != "plan" || scope.readonly.is_none() {
        return Err("资料范围不是只读授权".into());
    }
    Ok(scope)
}
/// Register within the same Host claim transaction as the scope. Never serialize
/// paths back to the model. Once a workflow owns a scope its catalog is sealed.
pub fn persist(
    conn: &Connection,
    scope_id: &str,
    evidence: &FrozenEvidence,
    now: i64,
) -> Result<()> {
    let boundary = scope(conn, scope_id, now)?;
    if evidence.document.is_none() {
        let path = boundary.path(evidence.path.to_str().ok_or("资料路径编码无效")?, false)?;
        if path != evidence.path {
            return Err("资料路径已变化".into());
        }
    }
    evidence.validate_bytes(conn, &read_bytes(&evidence.path)?)?;
    let encoded = serde_json::to_string(evidence).map_err(|_| "无法保存资料引用")?;
    let current: Option<String> = conn
        .query_row(
            "SELECT binding_json FROM agent_scope_evidence WHERE scope_id=?1 AND ref_id=?2",
            params![scope_id, evidence.reference.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if let Some(current) = current {
        return if current == encoded {
            Ok(())
        } else {
            Err("资料引用已绑定其他版本".into())
        };
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_scope_evidence WHERE scope_id=?1",
            [scope_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if count >= 20 {
        return Err("只读任务最多引用二十项资料".into());
    }
    // Scope-only tests/consumers need not create workflow tables.
    let has_workflows: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='agent_workflows')",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let seal_guard = if has_workflows {
        "AND NOT EXISTS(SELECT 1 FROM agent_workflows WHERE json_extract(policy_json,'$.scopeSnapshotId')=?1)"
    } else {
        ""
    };
    // The final INSERT checks both quota and sealing in its write transaction,
    // so separate callers cannot race the preflight checks above.
    let changed = conn.execute(
        &format!("INSERT INTO agent_scope_evidence(scope_id,ref_id,binding_json)
          SELECT ?1,?2,?3 WHERE
          (SELECT COUNT(*) FROM agent_scope_evidence WHERE scope_id=?1)<20
          AND EXISTS(SELECT 1 FROM agent_scope_snapshots WHERE id=?1 AND revoked=0 AND created_at<=?4 AND deadline_ms>?4) {seal_guard}"),
        params![scope_id, evidence.reference.id, encoded, now],
    ).map_err(sql_error)?;
    if changed != 1 {
        return Err("资料授权已封存、撤销或达到上限".into());
    }
    Ok(())
}
fn load(conn: &Connection, scope_id: &str, reference: &ContextRef) -> Result<FrozenEvidence> {
    let json: String = conn
        .query_row(
            "SELECT binding_json FROM agent_scope_evidence WHERE scope_id=?1 AND ref_id=?2",
            params![scope_id, reference.id],
            |row| row.get(0),
        )
        .map_err(|_| "资料引用未由宿主登记")?;
    let binding: FrozenEvidence = serde_json::from_str(&json).map_err(|_| "资料引用格式无效")?;
    if binding.reference != *reference {
        return Err("资料引用能力或版本不匹配".into());
    }
    Ok(binding)
}
/// Verify every input, not only citations the model elects to return.
pub fn verify(
    conn: &Connection,
    scope_id: &str,
    references: &[ContextRef],
    now: i64,
) -> Result<()> {
    scope(conn, scope_id, now)?;
    if references.len() > 20 {
        return Err("资料引用超过限制".into());
    }
    for reference in references {
        let binding = load(conn, scope_id, reference)?;
        binding.validate_bytes(conn, &read_bytes(&binding.path)?)?;
    }
    Ok(())
}
/// Verify the actual bytes returned by the native file tool, eliminating a
/// separate preflight-read / output-read race. Unregistered files are rejected.
pub fn verify_read(
    conn: &Connection,
    scope_id: &str,
    path: &Path,
    bytes: &[u8],
    now: i64,
) -> Result<()> {
    let boundary = scope(conn, scope_id, now)?;
    if boundary.path(path.to_str().ok_or("资料路径编码无效")?, false)? != path {
        return Err("资料路径已变化".into());
    }
    let bindings = conn
        .prepare("SELECT binding_json FROM agent_scope_evidence WHERE scope_id=?1")
        .map_err(sql_error)?
        .query_map([scope_id], |row| row.get::<_, String>(0))
        .map_err(sql_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sql_error)?;
    for json in bindings {
        let binding: FrozenEvidence =
            serde_json::from_str(&json).map_err(|_| "资料引用格式无效")?;
        if binding.path == path {
            return binding.validate_bytes(conn, bytes);
        }
    }
    Err("文件未登记为本轮版本化资料".into())
}

/// Resolve only the references in this child package. The model cannot turn an
/// arbitrary path into a grant by placing it in its objective or result.
pub fn task_paths(
    conn: &Connection,
    scope: &str,
    refs: &[ContextRef],
    now: i64,
) -> Result<Vec<PathBuf>> {
    verify(conn, scope, refs, now)?;
    refs.iter()
        .map(|reference| {
            let binding: String = conn
                .query_row(
                    "SELECT binding_json FROM agent_scope_evidence WHERE scope_id=?1 AND ref_id=?2",
                    params![scope, reference.id],
                    |r| r.get(0),
                )
                .map_err(sql_error)?;
            let evidence: FrozenEvidence =
                serde_json::from_str(&binding).map_err(|_| "资料引用数据无效")?;
            Ok(evidence.path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        conn: Connection,
        root: PathBuf,
        parent: Scope,
        scope_id: String,
        now: i64,
    }
    impl Fixture {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("evidence-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let conn = Connection::open_in_memory().unwrap();
            super::super::scope::create_schema(&conn).unwrap();
            let parent = Scope {
                read_lease: None,
                readonly: None,
                workspace: root.clone(),
                read_paths: vec![],
                workcopy: None,
                protected: vec![],
                mode: "ask".into(),
            };
            let now = 1;
            let scope_id = super::super::scope::persist_readonly(
                &conn,
                &parent,
                std::slice::from_ref(&root),
                false,
                1,
                1000,
                now,
            )
            .unwrap();
            Self {
                conn,
                root,
                parent,
                scope_id,
                now,
            }
        }
        fn file(&self, name: &str, content: &str) -> FrozenEvidence {
            let path = self.root.join(name);
            std::fs::write(&path, content).unwrap();
            FrozenEvidence::capture_file(&self.parent, name, &path).unwrap()
        }
        fn persist(&self, e: &FrozenEvidence) {
            persist(&self.conn, &self.scope_id, e, self.now).unwrap();
        }
        fn verify(&self, e: &FrozenEvidence) -> Result<()> {
            verify(&self.conn, &self.scope_id, &[e.reference()], self.now)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn hashes_actual_output_bytes_and_rejects_changed_missing_or_invented_references() {
        let f = Fixture::new();
        let evidence = f.file("source", "original");
        f.persist(&evidence);
        f.verify(&evidence).unwrap();
        verify_read(&f.conn, &f.scope_id, &evidence.path, b"original", f.now).unwrap();
        assert!(verify_read(&f.conn, &f.scope_id, &evidence.path, b"replaced", f.now).is_err());
        let unregistered = f.file("unregistered", "original");
        assert!(verify_read(&f.conn, &f.scope_id, &unregistered.path, b"original", f.now).is_err());
        let mut forged = evidence.reference();
        forged.version = "model-claimed-version".into();
        assert!(verify(&f.conn, &f.scope_id, &[forged], f.now).is_err());
        std::fs::write(&evidence.path, "replaced").unwrap();
        assert!(f.verify(&evidence).is_err());
        std::fs::remove_file(&evidence.path).unwrap();
        assert!(f.verify(&evidence).is_err());
    }
    #[test]
    fn catalogs_are_immutable_bounded_and_revocable() {
        let f = Fixture::new();
        let first = f.file("ref0", "evidence");
        f.persist(&first);
        f.persist(&first);
        assert!(f
            .conn
            .execute("UPDATE agent_scope_evidence SET binding_json='{}'", [])
            .is_err());
        for i in 1..20 {
            f.persist(&f.file(&format!("ref{i}"), "evidence"));
        }
        assert!(persist(&f.conn, &f.scope_id, &f.file("overflow", "evidence"), f.now).is_err());
        assert!(verify(&f.conn, &f.scope_id, &[first.reference()], 1000).is_err());
        super::super::scope::revoke(&f.conn, &f.scope_id).unwrap();
        assert!(f.verify(&first).is_err());
        assert!(verify_read(&f.conn, &f.scope_id, &first.path, b"evidence", f.now).is_err());
    }
    #[test]
    fn document_evidence_requires_disk_version_and_current_project_membership() {
        let f = Fixture::new();
        f.conn.execute_batch("CREATE TABLE articles(id TEXT PRIMARY KEY, version INTEGER, content TEXT); CREATE TABLE project_documents(project_id TEXT, article_id TEXT); INSERT INTO articles VALUES('note',7,'legacy content is not truth'); INSERT INTO project_documents VALUES('project','note');").unwrap();
        assert!(
            FrozenEvidence::capture_document(&f.conn, &f.root, "project", "note", "ref").is_err()
        );
        let path = f.root.join("note.md");
        std::fs::write(&path, "disk evidence").unwrap();
        assert!(
            FrozenEvidence::capture_document(&f.conn, &f.root, "other", "note", "ref").is_err()
        );
        let evidence =
            FrozenEvidence::capture_document(&f.conn, &f.root, "project", "note", "ref").unwrap();
        f.persist(&evidence);
        f.verify(&evidence).unwrap();
        f.conn.execute("UPDATE articles SET version=8", []).unwrap();
        assert!(f.verify(&evidence).is_err());
        f.conn.execute("UPDATE articles SET version=7", []).unwrap();
        std::fs::write(&path, "external modification without version increment").unwrap();
        assert!(f.verify(&evidence).is_err());
        std::fs::write(&path, "disk evidence").unwrap();
        f.conn.execute("DELETE FROM project_documents", []).unwrap();
        assert!(f.verify(&evidence).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn linked_replacement_and_directories_cannot_be_frozen_evidence() {
        let f = Fixture::new();
        let evidence = f.file("source", "same bytes");
        f.persist(&evidence);
        assert!(FrozenEvidence::capture_file(&f.parent, "folder", &f.root).is_err());
        let replacement = f.file("replacement", "same bytes");
        std::fs::remove_file(&evidence.path).unwrap();
        std::os::unix::fs::symlink(&replacement.path, &evidence.path).unwrap();
        assert!(f.verify(&evidence).is_err());
        assert!(verify_read(&f.conn, &f.scope_id, &evidence.path, b"same bytes", f.now).is_err());
    }
    #[test]
    fn workflow_seals_catalog_and_oversize_sources_are_rejected() {
        let f = Fixture::new();
        f.conn
            .execute_batch("CREATE TABLE agent_workflows(policy_json TEXT);")
            .unwrap();
        f.conn
            .execute(
                "INSERT INTO agent_workflows VALUES(?1)",
                [serde_json::json!({"scopeSnapshotId":f.scope_id}).to_string()],
            )
            .unwrap();
        assert!(persist(
            &f.conn,
            &f.scope_id,
            &f.file("late", "late evidence"),
            f.now
        )
        .is_err());
        let path = f.root.join("large");
        std::fs::write(&path, vec![b'x'; FILE_LIMIT as usize + 1]).unwrap();
        assert!(FrozenEvidence::capture_file(&f.parent, "large", &path).is_err());
    }
}
