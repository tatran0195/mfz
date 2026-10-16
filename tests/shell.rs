use std::fs;

use mfz::{
    config::ConfigLoader,
    shell::{export_mfz_file, get_desktop_dir},
};

fn create_temp_config(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mfz.ini");
    fs::write(&path, content).unwrap();
    (directory, path)
}

#[cfg(windows)]
#[test]
fn test_get_desktop_dir() {
    let path = get_desktop_dir().unwrap();
    assert!(
        path.is_absolute(),
        "Known Folder must be absolute, even when redirected/localized"
    );
}

#[cfg(not(windows))]
#[test]
fn desktop_export_does_not_claim_windows_support_on_other_platforms() {
    assert!(get_desktop_dir().is_err());
}

#[test]
fn test_export_pointer_and_load() {
    let ini_content = r#"
[stg]
profile = dev
instance = i-12345
rdp_host = stg-rdp
remote_port = 3389
local_port = 13389
"#;
    let (_directory, ini_path) = create_temp_config(ini_content);
    let loader = ConfigLoader::new(&ini_path);

    let temp_dir = _directory.path().join("exports");
    fs::create_dir_all(&temp_dir).unwrap();

    let exported = export_mfz_file("stg", None, false, &temp_dir).unwrap();
    assert!(exported.exists());
    assert_eq!(exported.file_name().unwrap(), "stg.mfz");

    let content = fs::read_to_string(&exported).unwrap();
    assert!(content.contains("env = stg"));

    let (name, env) = loader
        .load_from_file_or_name(exported.to_str().unwrap())
        .unwrap();
    assert_eq!(name, "stg");
    assert_eq!(env.profile.as_deref(), Some("dev"));
    assert_eq!(env.local_port, 13389);

    let _ = fs::remove_file(&ini_path);
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_export_standalone_and_load() {
    let ini_content = r#"
[prd]
profile = prod_profile
instance = i-99999
rdp_host = prd-rdp
remote_port = 3389
local_port = 13390
shared_drives = C:,D:
fullscreen = true
optimize = speed
"#;
    let (_directory, ini_path) = create_temp_config(ini_content);
    let loader = ConfigLoader::new(&ini_path);
    let prd_cfg = loader.load("prd").unwrap();

    let temp_dir = _directory.path().join("exports");
    fs::create_dir_all(&temp_dir).unwrap();

    let target_file = temp_dir.join("custom_prd.mfz");
    let exported = export_mfz_file("prd", Some(&prd_cfg), true, &target_file).unwrap();
    assert_eq!(exported, target_file);
    assert!(exported.exists());

    let content = fs::read_to_string(&exported).unwrap();
    assert!(content.contains("[prd]"));
    assert!(content.contains("profile = prod_profile"));
    assert!(content.contains("shared_drives = C:,D:"));
    assert!(content.contains("fullscreen = true"));
    assert!(content.contains("optimize = speed"));

    // Verify it loads with dummy loader that has no ini file
    let dummy_ini = _directory.path().join("non_existent.ini");
    let standalone_loader = ConfigLoader::new(&dummy_ini);
    let (name, loaded) = standalone_loader
        .load_from_file_or_name(exported.to_str().unwrap())
        .unwrap();
    assert_eq!(name, "prd");
    assert_eq!(loaded.profile.as_deref(), Some("prod_profile"));
    assert_eq!(loaded.local_port, 13390);
    assert_eq!(loaded.fullscreen, Some(true));
    assert_eq!(loaded.optimize.as_deref(), Some("speed"));

    let _ = fs::remove_file(&ini_path);
    let _ = fs::remove_dir_all(&temp_dir);
}
