use std::collections::BTreeMap;

use configparser::ini::Ini;

use crate::error::{AppError, Result};

/// Whole-line comments only: '#' and ';' are valid password/drive-list bytes.
fn parse_ini(content: String) -> Result<Ini> {
    let mut ini = Ini::new();
    ini.set_inline_comment_symbols(Some(&[]));
    ini.read(content)
        .map_err(|error| AppError::Config(format!("Invalid INI syntax: {error}")))?;
    let map = ini.get_map_ref().clone();
    for (section, entries) in map {
        for (key, value) in entries {
            if let Some(value) = value {
                if value.starts_with('"') {
                    let decoded: String = serde_json::from_str(&value).map_err(|_| {
                        AppError::Config(format!("[{section}] invalid quoted value for `{key}`; use JSON string escaping"))
                    })?;
                    ini.set(&section, &key, Some(decoded));
                }
            }
        }
    }
    Ok(ini)
}

/// Values with significant edge whitespace/control bytes or leading quotes use
/// JSON quoting; this prevents line injection without altering the actual value.
pub(crate) fn encode_ini_value(value: &str) -> String {
    if value.trim() != value || value.starts_with('"') || value.chars().any(char::is_control) {
        serde_json::to_string(value).expect("serializing a UTF-8 string cannot fail")
    } else {
        value.to_owned()
    }
}

fn serialize_ini(ini: &Ini) -> String {
    let mut out = String::new();
    let sections: BTreeMap<_, _> = ini.get_map_ref().iter().collect();
    for (name, entries) in sections {
        out.push_str(&format!("[{name}]\n"));
        let entries: BTreeMap<_, _> = entries.iter().collect();
        for (key, value) in entries {
            if let Some(value) = value {
                out.push_str(&format!("{key}={}\n", encode_ini_value(value)));
            } else {
                out.push_str(&format!("{key}\n"));
            }
        }
        out.push('\n');
    }
    out
}

mod model;
pub use model::{AuthType, EnvironmentConfig};

mod loader;
pub use loader::{ConfigLoader, LoadedEnvironment};

#[cfg(test)]
mod remediation_tests {
    use super::*;
    use crate::sys::storage::MutationLock;
    use std::{
        fs,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    const BASIC: &str = "[test]\nprofile=selected\ninstance=i-test\nremote_port=3389\nlocal_port=13389\nrdp_host=test-rdp\n";
    fn fixture(extra: &str) -> (tempfile::TempDir, ConfigLoader) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mfz.ini");
        fs::write(&path, format!("{BASIC}{extra}")).unwrap();
        (directory, ConfigLoader::new(path))
    }

    #[test]
    fn failed_secure_readback_preserves_all_plaintext_and_file_bytes() {
        let (_directory, loader) = fixture("aws_secret_access_key=plain-aws\nserial=arn:aws:iam::123456789012:mfa/test\nsecret=JBSWY3DPEHPK3PXP\nrdp_password=plain-rdp\n");
        let before = fs::read(loader.path()).unwrap();
        let mut calls = Vec::new();
        assert!(loader
            .secure_with("TEST", |kind, value| {
                calls.push(kind.to_owned());
                Ok(Some(if kind == "TOTP" {
                    "wrong-readback".into()
                } else {
                    value.into()
                }))
            })
            .is_err());
        assert_eq!(calls, ["AWS", "TOTP"]);
        assert_eq!(fs::read(loader.path()).unwrap(), before);
        let loaded = loader.load("test").unwrap();
        assert_eq!(loaded.aws_secret_access_key.as_deref(), Some("plain-aws"));
        assert_eq!(loaded.secret.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert_eq!(loaded.rdp_password.as_deref(), Some("plain-rdp"));
    }

    #[test]
    fn verified_secure_migration_removes_legacy_and_canonical_secret_keys() {
        let (_directory, loader) = fixture("secret_access_key=plain-aws\nserial=arn:aws:iam::123456789012:mfa/test\nsecret=JBSWY3DPEHPK3PXP\nrdp_password=plain-rdp\n");
        let mut calls = Vec::new();
        assert!(loader
            .secure_with("TEST", |kind, value| {
                calls.push(kind.to_owned());
                Ok(Some(value.into()))
            })
            .unwrap());
        assert_eq!(calls, ["AWS", "TOTP", "RDP"]);
        let raw = fs::read_to_string(loader.path()).unwrap();
        for value in [
            "plain-aws",
            "plain-rdp",
            "JBSWY3DPEHPK3PXP",
            "secret_access_key=",
            "secret=",
        ] {
            assert!(!raw.contains(value));
        }
        let loaded = loader.load("test").unwrap();
        assert!(
            loaded.aws_secret_access_key.is_none()
                && loaded.secret.is_none()
                && loaded.rdp_password.is_none()
        );
        assert!(!loader
            .secure_with("test", |_, _| panic!(
                "there are no remaining plaintext secrets"
            ))
            .unwrap());
    }

    #[test]
    fn quoted_passwords_and_literal_comment_characters_round_trip_exactly() {
        let (_directory, loader) = fixture("shared_drives=C:;D:\n");
        let password = "  \"quoted\" #;\\ path\r\n秘密  ";
        loader
            .update("test", |environment| {
                environment.rdp_password = Some(password.into());
                Ok(())
            })
            .unwrap();
        let loaded = loader.load("test").unwrap();
        assert_eq!(loaded.rdp_password.as_deref(), Some(password));
        assert_eq!(loaded.shared_drives.as_deref(), Some("C:;D:"));
        let raw = fs::read_to_string(loader.path()).unwrap();
        assert!(
            !raw.contains("\r\n秘密"),
            "control characters must be escaped, not new INI lines"
        );
    }

    #[test]
    fn insertion_does_not_overwrite_existing_environment() {
        let (_directory, loader) = fixture("");
        let before = fs::read(loader.path()).unwrap();
        let mut changed = loader.load("test").unwrap();
        changed.local_port += 1;
        assert!(loader.insert("TEST", &changed).is_err());
        assert_eq!(fs::read(loader.path()).unwrap(), before);
    }

    #[test]
    fn lock_covers_read_and_update_closure_and_can_be_retried_after_release() {
        let (_directory, loader) = fixture("rdp_user=before\n");
        let lock = MutationLock::acquire(loader.path()).unwrap();
        let reached = Arc::new(AtomicBool::new(false));
        let child_reached = reached.clone();
        let path = loader.path().to_owned();
        let result = std::thread::spawn(move || {
            ConfigLoader::new(path).update("test", |_| {
                child_reached.store(true, Ordering::SeqCst);
                Ok(())
            })
        })
        .join()
        .unwrap();
        assert!(result.is_err());
        assert!(!reached.load(Ordering::SeqCst));
        drop(lock);
        loader
            .update("test", |environment| {
                environment.rdp_user = Some("after".into());
                Ok(())
            })
            .unwrap();
        loader
            .update("test", |environment| {
                environment.local_port = 14444;
                Ok(())
            })
            .unwrap();
        let loaded = loader.load("test").unwrap();
        assert_eq!(loaded.rdp_user.as_deref(), Some("after"));
        assert_eq!(loaded.local_port, 14444);
    }

    #[test]
    fn readonly_persistence_failure_preserves_original_config() {
        let (_directory, loader) = fixture("rdp_password=still-here\n");
        let before = fs::read(loader.path()).unwrap();
        let original = fs::metadata(loader.path()).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(loader.path(), readonly).unwrap();
        let result = loader.update("test", |environment| {
            environment.rdp_password = None;
            Ok(())
        });
        fs::set_permissions(loader.path(), original).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(loader.path()).unwrap(), before);
    }

    #[test]
    fn external_profiles_do_not_gain_keyring_authority_by_copying_a_section_or_stem() {
        let (directory, loader) = fixture("");
        let standalone = directory.path().join("test.mfz");
        fs::write(&standalone, BASIC).unwrap();
        let loaded = loader
            .load_for_launch(standalone.to_str().unwrap())
            .unwrap();
        assert!(!loaded.allow_keyring);
        assert_eq!(loaded.source, standalone);
        fs::write(&standalone, "").unwrap();
        assert!(
            !loader
                .load_for_launch(standalone.to_str().unwrap())
                .unwrap()
                .allow_keyring
        );
        fs::write(&standalone, "env=test\n").unwrap();
        assert!(
            loader
                .load_for_launch(standalone.to_str().unwrap())
                .unwrap()
                .allow_keyring
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_write_is_rejected_without_modifying_target() {
        let (directory, loader) = fixture("");
        let link = directory.path().join("symlink.ini");
        std::os::unix::fs::symlink(loader.path(), &link).unwrap();
        let before = fs::read(loader.path()).unwrap();
        assert!(ConfigLoader::new(link)
            .update("test", |environment| {
                environment.local_port += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(fs::read(loader.path()).unwrap(), before);
    }
}

#[cfg(test)]
mod empty_password_regression {
    #[test]
    fn explicit_empty_password_is_not_replaced_by_a_vault_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mfz.ini");
        std::fs::write(&path, "[test]\nprofile=selected\ninstance=i-test\nremote_port=3389\nlocal_port=13389\nrdp_host=test-rdp\nrdp_user=Administrator\nrdp_password=\n").unwrap();
        let loader = super::ConfigLoader::new(&path);
        let environment = loader.load("test").unwrap();
        assert_eq!(environment.rdp_password.as_deref(), Some(""));
        assert_eq!(
            environment.resolve_rdp_password("test").unwrap().as_deref(),
            Some("")
        );
        loader.save("test", &environment).unwrap();
        assert_eq!(
            loader.load("test").unwrap().rdp_password.as_deref(),
            Some("")
        );
    }
}
