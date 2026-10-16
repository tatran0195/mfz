//! Evidence-driven regressions for the repository remediation pass.
//! Included unchanged by the app and the optional portable application-core harness.
use std::fs;

use clap::Parser;
use mfz::{cli::Cli, config::ConfigLoader, hosts::HostsManager, rdp, shell};
use tempfile::TempDir;

const VALID: &str =
    "[test]\nprofile=dev\ninstance=i-test\nremote_port=3389\nlocal_port=13389\nrdp_host=test-rdp\n";

fn fixture(content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mfz.ini");
    fs::write(&path, content).unwrap();
    (dir, path)
}

#[test]
fn aud_001_removing_secret_fields_actually_removes_persisted_keys() {
    let (_dir, path) = fixture(&format!("{VALID}aws_secret_access_key=plain-aws\nserial=arn:aws:iam::123456789012:mfa/test\nsecret=plain-totp\nrdp_password=plain-rdp\n"));
    let loader = ConfigLoader::new(&path);
    let mut env = loader.load("test").unwrap();
    env.aws_secret_access_key = None;
    env.secret = None;
    env.rdp_password = None;
    loader.save("test", &env).unwrap();
    let persisted = fs::read_to_string(path).unwrap();
    assert!(
        !persisted.contains("plain-"),
        "save retained removed secret fields"
    );
}

#[test]
fn aud_002_standalone_export_does_not_implicitly_disclose_secrets() {
    let (dir, path) = fixture(&format!("{VALID}aws_secret_access_key=plain-aws\nserial=arn:aws:iam::123456789012:mfa/test\nsecret=plain-totp\nrdp_password=plain-rdp\n"));
    let env = ConfigLoader::new(path).load("test").unwrap();
    let exported = shell::export_mfz_file("test", Some(&env), true, dir.path()).unwrap();
    let content = fs::read_to_string(exported).unwrap();
    assert!(
        !content.contains("plain-"),
        "ordinary export silently includes secrets"
    );
}

#[test]
fn aud_003_export_environment_cannot_escape_destination_directory() {
    let (dir, path) = fixture(VALID);
    let env = ConfigLoader::new(path).load("test").unwrap();
    let destination = dir.path().join("exports");
    fs::create_dir(&destination).unwrap();
    let result = shell::export_mfz_file("../escaped", Some(&env), true, &destination);
    assert!(result.is_err());
    assert!(!dir.path().join("escaped.mfz").exists());
}

#[test]
fn aud_004_invalid_boolean_is_rejected_not_treated_as_absent() {
    let (_dir, path) = fixture(&format!("{VALID}fullscreen=definitely-not-a-boolean\n"));
    assert!(ConfigLoader::new(path).load("test").is_err());
}

#[test]
fn aud_005_zero_ports_are_not_valid_session_configuration() {
    let (_dir, path) = fixture(&VALID.replace("local_port=13389", "local_port=0"));
    assert!(ConfigLoader::new(path).load("test").is_err());
}

#[test]
fn aud_006_semicolons_and_hashes_are_literal_inside_values() {
    let (_dir, path) = fixture(&format!(
        "{VALID}rdp_password=secret#with;punctuation\nshared_drives=C:;D:\n"
    ));
    let env = ConfigLoader::new(path).load("test").unwrap();
    assert_eq!(env.rdp_password.as_deref(), Some("secret#with;punctuation"));
    assert_eq!(env.shared_drives.as_deref(), Some("C:;D:"));
}

#[test]
fn aud_007_malformed_standalone_does_not_launch_unrelated_stem_environment() {
    let (dir, path) = fixture(VALID);
    let profile = dir.path().join("test.mfz");
    fs::write(&profile, "[session]\nprofile=dev\nremote_port=invalid\n").unwrap();
    assert!(ConfigLoader::new(path)
        .load_from_file_or_name(profile.to_str().unwrap())
        .is_err());
}

#[test]
fn aud_008_pointer_config_paths_are_relative_to_the_pointer_file() {
    let (dir, path) = fixture(VALID);
    let profile = dir.path().join("test.mfz");
    fs::write(&profile, "env=test\nconfig=mfz.ini\n").unwrap();
    let unrelated_loader = ConfigLoader::new(dir.path().join("not-present.ini"));
    let (name, env) = unrelated_loader
        .load_from_file_or_name(profile.to_str().unwrap())
        .unwrap();
    assert_eq!(name, "test");
    assert_eq!(env.local_port, 13389);
    assert!(path.exists());
}

#[test]
fn aud_009_environment_prefix_is_not_a_pointer_key() {
    let (dir, path) = fixture(VALID);
    let profile = dir.path().join("profile.mfz");
    fs::write(&profile, format!("{VALID}environment_notes=wrong-target\n")).unwrap();
    let (_, env) = ConfigLoader::new(path)
        .load_from_file_or_name(profile.to_str().unwrap())
        .unwrap();
    assert_eq!(env.local_port, 13389);
}

#[test]
fn aud_010_invalid_sections_are_not_silently_discarded() {
    let (_dir, path) = fixture(&format!("{VALID}\n[broken]\nprofile=dev\n"));
    assert!(ConfigLoader::new(path).get_all().is_err());
}

#[test]
fn aud_011_standalone_without_configuration_is_an_error_not_a_pointer() {
    let dir = tempfile::tempdir().unwrap();
    assert!(shell::export_mfz_file("test", None, true, dir.path()).is_err());
}

#[test]
fn aud_012_comma_separated_drive_lists_are_normalized_for_mstsc() {
    assert_eq!(
        rdp::resolve_drives_to_redirect(Some("c:, D:"), None)
            .unwrap()
            .as_deref(),
        Some("C:;D:")
    );
}

#[test]
fn aud_013_non_ascii_drive_letters_are_not_truncated_to_ascii() {
    // U+0141 truncates to the ASCII byte 'A' if cast before validation.
    assert!(rdp::is_drive_letter_in_use('\u{141}'));
}

#[test]
fn aud_014_inline_hosts_comments_are_not_host_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hosts");
    fs::write(&path, "127.0.0.1 actual # nonexistent\n").unwrap();
    let hosts = HostsManager::with_path(path);
    assert!(!hosts.has_host("nonexistent").unwrap());
    assert!(hosts.has_host("ACTUAL").unwrap());
}

#[test]
fn aud_015_hosts_input_cannot_inject_additional_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hosts");
    fs::write(&path, "127.0.0.1 localhost\n").unwrap();
    let hosts = HostsManager::with_path(path.clone());
    assert!(hosts
        .add_host("127.0.0.1", "normal\n203.0.113.1 other-host")
        .is_err());
    assert!(!fs::read_to_string(path).unwrap().contains("other-host"));
}

#[test]
fn aud_016_cli_rejects_ambiguous_export_modes() {
    assert!(Cli::try_parse_from(["mfz", "export", "test", "--all"]).is_err());
    assert!(Cli::try_parse_from(["mfz", "export", "test", "--standalone", "--pointer"]).is_err());
}
