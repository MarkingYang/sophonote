use super::*;
use std::io::Write;

fn temporary() -> Staging {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("opencode-update-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    Staging(dir)
}
fn release() -> Release {
    Release {
        tag_name: format!("v{}", runtime::VERSION),
        draft: false,
        prerelease: false,
        assets: vec![Asset {
            name: asset_name().into(),
            browser_download_url: format!(
                "https://github.com/anomalyco/opencode/releases/download/v{}/{}",
                runtime::VERSION,
                asset_name()
            ),
            digest: Some(format!("sha256:{}", "a".repeat(64))),
            size: 100,
        }],
    }
}
#[test]
fn official_release_requires_stable_platform_source_digest_and_bounded_size() {
    assert!(select_release(release()).is_ok());
    for mutate in [
        |r: &mut Release| r.draft = true,
        |r: &mut Release| r.prerelease = true,
        |r: &mut Release| r.assets[0].digest = None,
        |r: &mut Release| r.assets[0].digest = Some("sha256:../invalid".into()),
        |r: &mut Release| {
            r.assets[0].browser_download_url = "https://mirror.example/opencode".into()
        },
        |r: &mut Release| r.assets[0].size = MAX_DOWNLOAD + 1,
        |r: &mut Release| r.assets[0].size = 0,
        |r: &mut Release| r.assets[0].name = "opencode-wrong-platform.zip".into(),
        |r: &mut Release| r.tag_name = "v1.0.0".into(),
        |r: &mut Release| r.tag_name = "v1.19.0-beta".into(),
    ] {
        let mut r = release();
        mutate(&mut r);
        assert!(select_release(r).is_err());
    }
    for version in ["../../outside", "1.2", "01.2.3", "1.2.3/else", "1.2.3.4"] {
        assert!(version_tuple(version).is_err());
    }
}
fn zip(name: &str, symlink: bool) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    if symlink {
        writer
            .add_symlink(name, "/outside", zip::write::SimpleFileOptions::default())
            .unwrap();
    } else {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"fixture").unwrap();
    }
    writer.finish().unwrap().into_inner()
}
#[test]
fn zip_rejects_traversal_links_and_duplicate_output() {
    let root = temporary();
    for name in [
        "../opencode",
        "/opencode",
        "nested/opencode",
        "C:\\opencode",
        "manifest.json",
    ] {
        assert!(unpack(&zip(name, false), &root.0).is_err());
    }
    assert!(unpack(&zip(executable_name(), true), &root.0).is_err());
    unpack(&zip(executable_name(), false), &root.0).unwrap();
    assert!(unpack(&zip(executable_name(), false), &root.0).is_err());
}
fn fixture_slot(root: &Path) -> Pointer {
    let pointer = Pointer {
        version: runtime::VERSION.into(),
        digest: "a".repeat(64),
        slot: uuid::Uuid::new_v4().to_string(),
    };
    let directory = slot(root, &pointer).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(executable_name()), b"fixture").unwrap();
    let manifest = Manifest {
        version: pointer.version.clone(),
        target: runtime::target().into(),
        archive_sha256: pointer.digest.clone(),
        executable: executable_name().into(),
        files: BTreeMap::from([(
            executable_name().into(),
            format!("{:x}", Sha256::digest(b"fixture")),
        )]),
    };
    std::fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    pointer
}
fn bundled() -> Result<Runtime, String> {
    Ok(Runtime {
        version: runtime::VERSION.into(),
        executable: PathBuf::from("bundled-opencode"),
    })
}
#[test]
fn immutable_activation_fallback_and_failed_commit_preserve_previous_run() {
    let root = temporary();
    let first = fixture_slot(&root.0);
    activate(&root.0, &first).unwrap();
    let (running, warning) = resolve_at(&root.0, bundled).unwrap();
    assert!(warning.is_none());
    let second = fixture_slot(&root.0);
    activate(&root.0, &second).unwrap();
    let (next, _) = resolve_at(&root.0, bundled).unwrap();
    assert_ne!(running.executable, next.executable);
    assert_eq!(std::fs::read(&running.executable).unwrap(), b"fixture");
    let saved = std::fs::read(root.0.join("active.json")).unwrap();
    std::fs::write(&next.executable, b"tampered").unwrap();
    assert!(activate(&root.0, &second).is_err());
    assert_eq!(std::fs::read(root.0.join("active.json")).unwrap(), saved);
    let (fallback, warning) = resolve_at(&root.0, bundled).unwrap();
    assert_eq!(fallback.executable, Path::new("bundled-opencode"));
    assert!(warning.unwrap().contains("回退随包"));
    std::fs::write(root.0.join("active.json"), b"not json").unwrap();
    assert!(resolve_at(&root.0, bundled).unwrap().1.is_some());
    let older = Pointer {
        version: "1.0.0".into(),
        ..first
    };
    std::fs::write(
        root.0.join("active.json"),
        serde_json::to_vec(&older).unwrap(),
    )
    .unwrap();
    assert!(resolve_at(&root.0, bundled)
        .unwrap()
        .1
        .unwrap()
        .contains("低于随包"));
}
#[tokio::test]
async fn archive_hash_failure_keeps_active_pointer() {
    let root = temporary();
    let pointer = fixture_slot(&root.0);
    activate(&root.0, &pointer).unwrap();
    let saved = std::fs::read(root.0.join("active.json")).unwrap();
    let available = Available {
        version: runtime::VERSION.into(),
        url: String::new(),
        digest: "b".repeat(64),
        size: 7,
    };
    assert!(stage(b"invalid".to_vec(), &available, &root.0)
        .await
        .is_err());
    assert_eq!(std::fs::read(root.0.join("active.json")).unwrap(), saved);
}
#[test]
fn manifest_rejects_unlisted_files_and_links() {
    let root = temporary();
    let p = fixture_slot(&root.0);
    let directory = slot(&root.0, &p).unwrap();
    std::fs::write(directory.join("extra"), b"unlisted").unwrap();
    assert!(runtime::verify_version(&directory, &p.version, &p.digest).is_err());
    std::fs::remove_file(directory.join("extra")).unwrap();
    #[cfg(unix)]
    {
        std::fs::remove_file(directory.join(executable_name())).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", directory.join(executable_name())).unwrap();
        assert!(runtime::verify_version(&directory, &p.version, &p.digest).is_err());
    }
}
#[tokio::test]
#[ignore = "uses cached official OpenCode ZIP, local signing and real isolated HTTP/SSE"]
async fn official_archive_stages_activates_and_passes_native_health() {
    let root = temporary();
    let data = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/opencode-downloads")
            .join(format!("{}-{}", runtime::VERSION, asset_name())),
    )
    .unwrap();
    let available = Available {
        version: runtime::VERSION.into(),
        url: String::new(),
        digest: format!("{:x}", Sha256::digest(&data)),
        size: data.len() as u64,
    };
    let pointer = stage(data, &available, &root.0).await.unwrap();
    assert!(!root.0.join("active.json").exists());
    activate(&root.0, &pointer).unwrap();
    let (runtime, warning) =
        resolve_at(&root.0, || Err("bundle deliberately unavailable".into())).unwrap();
    assert_eq!(runtime.version, available.version);
    assert!(warning.is_none());
    assert!(runtime.executable.starts_with(root.0.join("versions")));
}
