//! Bounded, process-local cache of runtime validation evidence. Never a launch
//! permit, credential cache, or model endpoint health claim.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::registry::EngineId;

const TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Observation {
    Unknown,
    Verified { version: String, checked_at_ms: u64 },
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    canonical: PathBuf,
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn stamp(path: &Path) -> Result<Option<FileStamp>, ()> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Some(FileStamp {
        canonical: path.canonicalize().map_err(|_| ())?,
        len: meta.len(),
        modified: meta.modified().map_err(|_| ())?,
        #[cfg(unix)]
        identity: (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()),
    }))
}

fn locator_environment() -> Vec<Option<std::ffi::OsString>> {
    ["SOPHONOTE_CLAUDE_BIN", "PATH", "HOME", "USERPROFILE"]
        .into_iter()
        .map(std::env::var_os)
        .collect()
}

struct Entry {
    engine: EngineId,
    namespace: PathBuf,
    observed: Instant,
    evidence: Observation,
    files: Vec<(PathBuf, Option<FileStamp>)>,
    locator_environment: Vec<Option<std::ffi::OsString>>,
}

#[derive(Default)]
struct Cache(Vec<Entry>);

impl Cache {
    fn record(&mut self, entry: Entry) {
        self.0.retain(|old| old.engine != entry.engine);
        self.0.push(entry); // Closed EngineId => at most four entries.
    }

    fn get(&mut self, namespace: &Path, engine: EngineId, now: Instant) -> Observation {
        let Some(index) = self.0.iter().position(|e| e.engine == engine) else {
            return Observation::Unknown;
        };
        let entry = &self.0[index];
        let fresh = entry.namespace == namespace
            && now
                .checked_duration_since(entry.observed)
                .is_some_and(|age| age < TTL)
            && (engine != EngineId::ClaudeCode
                || entry.locator_environment == locator_environment())
            && entry
                .files
                .iter()
                .all(|(path, old)| stamp(path).is_ok_and(|current| &current == old));
        if fresh {
            entry.evidence.clone()
        } else {
            self.0.remove(index);
            Observation::Unknown
        }
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

pub(crate) fn get(namespace: &Path, engine: EngineId) -> Observation {
    cache()
        .lock()
        .map(|mut cache| cache.get(namespace, engine, Instant::now()))
        .unwrap_or(Observation::Unknown)
}

/// Called only after existing native runtime validation. Watched files include
/// the executable/manifest/policy and active pointer (including pointer absence).
/// File metadata is merely invalidation evidence; prepare still hashes resources.
pub(crate) fn record(namespace: PathBuf, engine: EngineId, version: &str, paths: Vec<PathBuf>) {
    if engine == EngineId::Hermes || paths.is_empty() {
        return;
    }
    let files = paths
        .into_iter()
        .map(|path| stamp(&path).map(|s| (path, s)))
        .collect::<Result<Vec<_>, _>>();
    let Ok(files) = files else {
        invalidate(engine);
        return;
    };
    // The executable is first and must exist. Only optional pointers may be absent.
    if files.first().is_none_or(|(_, s)| s.is_none()) {
        invalidate(engine);
        return;
    }
    if let Ok(mut cache) = cache().lock() {
        cache.record(Entry {
            engine,
            namespace,
            observed: Instant::now(),
            evidence: Observation::Verified {
                version: version.into(),
                checked_at_ms: super::pi::now_ms(),
            },
            files,
            locator_environment: locator_environment(),
        });
    }
}

pub(crate) fn invalidate(engine: EngineId) {
    if let Ok(mut cache) = cache().lock() {
        cache.0.retain(|e| e.engine != engine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(engine: EngineId, now: Instant) -> Entry {
        Entry {
            engine,
            namespace: PathBuf::from("fixture"),
            observed: now,
            evidence: Observation::Verified {
                version: "test".into(),
                checked_at_ms: 1,
            },
            files: vec![],
            locator_environment: locator_environment(),
        }
    }

    #[test]
    fn expires_at_exact_ttl_and_does_not_share_namespaces_or_future_observations() {
        let mut cache = Cache::default();
        let now = Instant::now();
        let namespace = Path::new("fixture");
        cache.record(entry(EngineId::Pi, now));
        assert!(matches!(
            cache.get(namespace, EngineId::Pi, now + TTL - Duration::from_nanos(1)),
            Observation::Verified { .. }
        ));
        assert!(matches!(
            cache.get(namespace, EngineId::Pi, now + TTL),
            Observation::Unknown
        ));
        cache.record(entry(EngineId::Pi, now));
        assert!(matches!(
            cache.get(Path::new("other"), EngineId::Pi, now),
            Observation::Unknown
        ));
        cache.record(entry(EngineId::Pi, now + Duration::from_secs(1)));
        assert!(matches!(
            cache.get(namespace, EngineId::Pi, now),
            Observation::Unknown
        ));
    }

    #[test]
    fn cache_is_bounded_and_locator_changes_invalidate() {
        let mut cache = Cache::default();
        let now = Instant::now();
        for _ in 0..10 {
            for engine in EngineId::ALL {
                cache.record(entry(engine, now));
            }
        }
        assert_eq!(cache.0.len(), 4);
        let mut changed = entry(EngineId::ClaudeCode, now);
        changed.locator_environment = vec![];
        cache.record(changed);
        assert!(matches!(
            cache.get(Path::new("fixture"), EngineId::ClaudeCode, now),
            Observation::Unknown
        ));
    }

    #[test]
    fn file_modification_deletion_and_pointer_creation_invalidate() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = Fixture(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!(".runtime-cache-test-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir(&root.0).unwrap();
        let executable = root.0.join("runtime");
        let pointer = root.0.join("active.json");
        std::fs::write(&executable, "old").unwrap();
        let now = Instant::now();
        let mut cache = Cache::default();
        let capture = || {
            let mut e = entry(EngineId::Pi, now);
            e.files = vec![
                (executable.clone(), stamp(&executable).unwrap()),
                (pointer.clone(), stamp(&pointer).unwrap()),
            ];
            e
        };
        cache.record(capture());
        assert!(matches!(
            cache.get(Path::new("fixture"), EngineId::Pi, now),
            Observation::Verified { .. }
        ));
        std::fs::write(&pointer, "new slot").unwrap();
        assert!(matches!(
            cache.get(Path::new("fixture"), EngineId::Pi, now),
            Observation::Unknown
        ));
        cache.record(capture());
        std::fs::write(&executable, "changed runtime").unwrap();
        assert!(matches!(
            cache.get(Path::new("fixture"), EngineId::Pi, now),
            Observation::Unknown
        ));
        cache.record(capture());
        std::fs::remove_file(&executable).unwrap();
        assert!(matches!(
            cache.get(Path::new("fixture"), EngineId::Pi, now),
            Observation::Unknown
        ));
    }
}
