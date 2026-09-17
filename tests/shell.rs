use std::fs;

use mfz::{
    config::ConfigLoader,
    shell::{export_mfz_file, get_desktop_dir},
};

fn create_temp_config(content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "test_shell_{}.ini",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn test_get_desktop_dir() {
    let desktop = get_desktop_dir();
    assert!(desktop.is_some());
    let path = desktop.unwrap();
    assert!(path.to_str().unwrap().contains("Desktop"));
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
    let ini_path = create_temp_config(ini_content);
    let loader = ConfigLoader::new(&ini_path);

    let temp_dir = std::env::temp_dir().join(format!(
        "test_exp_ptr_{}",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
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
    assert_eq!(env.profile, "dev");
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
    let ini_path = create_temp_config(ini_content);
    let loader = ConfigLoader::new(&ini_path);
    let prd_cfg = loader.load("prd").unwrap();

    let temp_dir = std::env::temp_dir().join(format!(
        "test_exp_stand_{}",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
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
    let dummy_ini = std::env::temp_dir().join("non_existent.ini");
    let standalone_loader = ConfigLoader::new(&dummy_ini);
    let (name, loaded) = standalone_loader
        .load_from_file_or_name(exported.to_str().unwrap())
        .unwrap();
    assert_eq!(name, "prd");
    assert_eq!(loaded.profile, "prod_profile");
    assert_eq!(loaded.local_port, 13390);
    assert_eq!(loaded.fullscreen, Some(true));
    assert_eq!(loaded.optimize.as_deref(), Some("speed"));

    let _ = fs::remove_file(&ini_path);
    let _ = fs::remove_dir_all(&temp_dir);
}
