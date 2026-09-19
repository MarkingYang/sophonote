//! Provider credentials. Only an explicit save may request Keychain authorization.
//! All access to the legacy macOS login keychain shares this gate because its UI
//! permission is process-wide, including writes and migration verification.

use std::sync::Mutex;

const SERVICE: &str = "com.fei.sophonote";
static ACCESS: Mutex<()> = Mutex::new(());

fn with_access<T>(
    interactive: bool,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _access = ACCESS.lock().map_err(|_| "凭据访问状态异常，请重启应用")?;
    #[cfg(target_os = "macos")]
    let _quiet = {
        use security_framework::os::macos::keychain::SecKeychain;
        // The RAII guard restores true, so create it only when the previous
        // state was true. Never enable interaction disabled by another caller.
        if !interactive && SecKeychain::user_interaction_allowed().map_err(platform_error)? {
            Some(SecKeychain::disable_user_interaction().map_err(platform_error)?)
        } else {
            None
        }
    };
    #[cfg(not(target_os = "macos"))]
    let _ = interactive;
    operation()
}

fn entry(provider: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, provider).map_err(keyring_error)
}

fn keyring_error(error: keyring::Error) -> String {
    format!("无法访问已保存的 API Key，请在对应配置中重新保存并完成系统授权：{error}")
}

#[cfg(target_os = "macos")]
fn platform_error(error: security_framework::base::Error) -> String {
    keyring_error(keyring::macos::decode_error(error))
}

pub(crate) fn read(provider: &str) -> Result<Option<String>, String> {
    with_access(false, || match entry(provider)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(keyring_error(error)),
    })
}

/// Query attributes only. A configured credential need not currently be readable.
pub(crate) fn contains(provider: &str) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
        // Preserve keyring's rejection of empty account names (wildcards).
        entry(provider)?;
        with_access(false, || {
            let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)
                .map_err(platform_error)?;
            contains_in_keychain(keychain, provider)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        read(provider).map(|key| key.is_some_and(|key| !key.is_empty()))
    }
}

#[cfg(target_os = "macos")]
fn contains_in_keychain(
    keychain: security_framework::os::macos::keychain::SecKeychain,
    provider: &str,
) -> Result<bool, String> {
    use security_framework::item::{ItemClass, ItemSearchOptions};
    match ItemSearchOptions::new()
        .keychains(&[keychain])
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(provider)
        .load_attributes(true)
        .search()
    {
        Ok(items) => Ok(!items.is_empty()),
        Err(error) if error.code() == -25300 => Ok(false),
        Err(error) => Err(platform_error(error)),
    }
}

/// Migration is silent; only the explicit save command passes interactive=true.
pub(crate) fn save(provider: &str, key: &str, interactive: bool) -> Result<(), String> {
    with_access(interactive, || {
        let entry = entry(provider)?;
        entry.set_password(key).map_err(keyring_error)?;
        let verified = entry.get_password().map_err(keyring_error)?;
        if verified != key {
            return Err("Keychain 回读不一致，旧凭据已保留".into());
        }
        Ok(())
    })
}

pub(crate) fn delete(provider: &str) -> Result<(), String> {
    with_access(false, || match entry(provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keyring_error(error)),
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use security_framework::os::macos::keychain::SecKeychain;
    static TEST_ACCESS: Mutex<()> = Mutex::new(());

    #[test]
    fn quiet_access_restores_ui_after_success_and_error() {
        let _test = TEST_ACCESS.lock().unwrap();
        let before = SecKeychain::user_interaction_allowed().unwrap();
        with_access(false, || {
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            Ok(())
        })
        .unwrap();
        assert_eq!(SecKeychain::user_interaction_allowed().unwrap(), before);
        // Respect a pre-existing no-UI policy, including the save path.
        let _disabled = SecKeychain::disable_user_interaction().unwrap();
        for interactive in [false, true] {
            with_access(interactive, || {
                assert!(!SecKeychain::user_interaction_allowed().unwrap());
                Ok(())
            })
            .unwrap();
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
        }
        drop(_disabled);
        assert!(with_access(false, || {
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            Err::<(), _>("denied".into())
        })
        .is_err());
        assert_eq!(SecKeychain::user_interaction_allowed().unwrap(), before);
    }

    #[test]
    fn missing_credential_and_empty_account_do_not_prompt() {
        let _test = TEST_ACCESS.lock().unwrap();
        let provider = format!("sophonote-test-{}", uuid::Uuid::new_v4());
        assert!(!contains(&provider).unwrap());
        assert_eq!(read(&provider).unwrap(), None);
        assert!(contains("").is_err());
        assert!(read("").is_err());
    }

    #[test]
    #[ignore = "macOS host regression: creates and deletes an isolated test keychain"]
    fn protected_credential_status_does_not_require_secret_access() {
        let _test = TEST_ACCESS.lock().unwrap();
        use security_framework::os::macos::passwords::find_generic_password;
        use std::{path::PathBuf, process::Command};

        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = Command::new("/usr/bin/security")
                    .arg("delete-keychain")
                    .arg(&self.0)
                    .output();
            }
        }
        let fixture = Fixture(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "credential-test-{}.keychain-db",
                    uuid::Uuid::new_v4()
                )),
        );
        let security = |args: &[&str]| {
            let output = Command::new("/usr/bin/security")
                .args(args)
                .arg(&fixture.0)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        security(&["create-keychain", "-p", "fixture-only"]);
        // Only /usr/bin/security is trusted, deliberately excluding this test
        // executable. No user credential or existing keychain is changed.
        security(&[
            "add-generic-password",
            "-s",
            SERVICE,
            "-a",
            "fixture",
            "-w",
            "fixture-value",
            "-T",
            "/usr/bin/security",
        ]);
        let keychain = SecKeychain::open(&fixture.0).unwrap();
        with_access(false, || {
            assert!(contains_in_keychain(keychain.clone(), "fixture")?);
            let denied =
                find_generic_password(Some(std::slice::from_ref(&keychain)), SERVICE, "fixture");
            assert!(
                denied.is_err(),
                "untrusted test binary must not read the secret"
            );
            assert!(!SecKeychain::user_interaction_allowed().unwrap());
            Ok(())
        })
        .unwrap();
        security(&["lock-keychain"]);
        with_access(false, || {
            assert!(find_generic_password(Some(&[keychain]), SERVICE, "fixture").is_err());
            Ok(())
        })
        .unwrap();
    }
}
