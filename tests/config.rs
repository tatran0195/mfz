use std::fs;

use mfz::config::{AuthType, ConfigLoader, EnvironmentConfig};

fn create_temp_config(content: &str) -> std::path::PathBuf {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "mfz_test_config_{}.ini",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    fs::write(&file_path, content).unwrap();
    file_path
}

#[test]
fn test_config_add_new() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "mfz_test_config_new_{}.ini",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    if file_path.exists() {
        fs::remove_file(&file_path).unwrap();
    }

    let loader = ConfigLoader::new(&file_path);
    let env_config = EnvironmentConfig {
        profile: Some("test_profile".to_string()),
        aws_access_key_id: None,
        aws_secret_access_key: None,
        region: None,
        auth_type: AuthType::Sso,
        serial: None,
        secret: None,
        instance: Some("i-12345".to_string()),
        instance_tag: None,
        remote_port: 3389,
        local_port: 13389,
        rdp_host: "test_host".to_string(),
        rdp_user: None,
        rdp_password: None,
        shared_drives: None,
        shared_folder: None,
        optimize_connection: None,
        fullscreen: None,
        optimize: None,
        opt_audio: None,
        opt_wallpaper: None,
        opt_font_smoothing: None,
    };

    assert!(loader.save("test_env", &env_config).is_ok());

    let env_config2 = EnvironmentConfig {
        profile: Some("test_profile2".to_string()),
        aws_access_key_id: None,
        aws_secret_access_key: None,
        region: None,
        auth_type: AuthType::Mfa,
        serial: Some("arn:test".to_string()),
        secret: Some("secret123".to_string()),
        instance: None,
        instance_tag: Some("Name=test".to_string()),
        remote_port: 3389,
        local_port: 13390,
        rdp_host: "test_host2".to_string(),
        rdp_user: Some("admin".to_string()),
        rdp_password: Some("passwd".to_string()),
        shared_drives: None,
        shared_folder: None,
        optimize_connection: None,
        fullscreen: None,
        optimize: None,
        opt_audio: None,
        opt_wallpaper: None,
        opt_font_smoothing: None,
    };

    assert!(loader.save("test_env2", &env_config2).is_ok());

    let loaded = loader.load("test_env2").unwrap();
    assert_eq!(loaded.profile.as_deref(), Some("test_profile2"));
    assert_eq!(loaded.serial.as_deref(), Some("arn:test"));
    assert_eq!(loaded.auth_type, AuthType::Mfa);
    assert_eq!(loaded.instance_tag.as_deref(), Some("Name=test"));

    fs::remove_file(&file_path).unwrap();
}

#[test]
fn test_config_list_and_load() {
    let content = r#"
[stg]
profile=nsy_web_app_stg
serial=arn:aws:iam::123:mfa/waka
instance=i-abc
remote_port=3389
local_port=13389
rdp_host=stg-rdp

[prd]
profile=nsy_web_app_prd
serial=arn:aws:iam::456:mfa/waka
instance=i-def
remote_port=3389
local_port=13390
rdp_host=prd-rdp
"#;

    let path = create_temp_config(content);
    let loader = ConfigLoader::new(&path);

    let envs = loader.list_environments().unwrap();
    assert_eq!(envs.len(), 2);
    assert!(envs.contains(&"stg".to_string()));
    assert!(envs.contains(&"prd".to_string()));

    let stg = loader.load("stg").unwrap();
    assert_eq!(stg.profile.as_deref(), Some("nsy_web_app_stg"));
    assert_eq!(stg.serial.as_deref(), Some("arn:aws:iam::123:mfa/waka"));
    assert_eq!(stg.instance.as_deref(), Some("i-abc"));

    let prd = loader.load("prd").unwrap();
    assert_eq!(prd.local_port, 13390);

    fs::remove_file(&path).unwrap();
}

#[test]
fn test_config_missing_required() {
    let content = r#"
[bad]
profile=nsy_web_app_stg
instance=i-abc
"#;

    let path = create_temp_config(content);
    let loader = ConfigLoader::new(&path);

    let res = loader.load("bad");
    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("Configuration error"));
    assert!(err_msg.contains("missing keys"));
    assert!(err_msg.contains("remote_port"));

    fs::remove_file(&path).unwrap();
}

#[test]
fn test_config_get_all() {
    let content = r#"
[e1]
profile=p1
instance=i-1
remote_port=3389
local_port=1111
rdp_host=h1

[e2]
profile=p2
instance_tag=Name=abc
remote_port=3389
local_port=2222
rdp_host=h2
"#;

    let path = create_temp_config(content);
    let loader = ConfigLoader::new(&path);

    let all = loader.get_all().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all.get("e1").unwrap().profile.as_deref(), Some("p1"));
    assert_eq!(
        all.get("e2").unwrap().instance_tag.as_deref(),
        Some("Name=abc")
    );

    fs::remove_file(&path).unwrap();
}

#[test]
fn test_config_shared_drives_and_optimization() {
    let content = r#"
[test_env]
profile = dev
instance = i-12345
rdp_host = test-host
remote_port = 3389
local_port = 13389
shared_drives = C:,D:
shared_folder = C:\Shared
optimize_connection = true
"#;
    let path = create_temp_config(content);
    let loader = ConfigLoader::new(&path);
    let env = loader.load("test_env").unwrap();

    assert_eq!(env.shared_drives.as_deref(), Some("C:,D:"));
    assert_eq!(env.shared_folder.as_deref(), Some(r"C:\Shared"));
    assert_eq!(env.optimize.as_deref(), Some("speed"));

    fs::remove_file(&path).unwrap();
}

#[test]
fn test_config_share_folder_alias_and_fullscreen() {
    let content = r#"
[stg]
profile = dev
instance = i-12345
rdp_host = stg-rdp
remote_port = 3389
local_port = 13389
share_folder = D:\SharedData
share_drives = C:
fullscreen = false
optimize = speed
audio = true
wallpaper = false
font_smoothing = true
"#;
    let path = create_temp_config(content);
    let loader = ConfigLoader::new(&path);
    let env = loader.load("stg").unwrap();

    assert_eq!(env.shared_folder.as_deref(), Some(r"D:\SharedData"));
    assert_eq!(env.shared_drives.as_deref(), Some("C:"));
    assert_eq!(env.fullscreen, Some(false));
    assert_eq!(env.optimize.as_deref(), Some("speed"));
    assert_eq!(env.opt_audio, Some(true));
    assert_eq!(env.opt_wallpaper, Some(false));
    assert_eq!(env.opt_font_smoothing, Some(true));

    fs::remove_file(&path).unwrap();
}

#[test]
fn test_config_load_pointer_mfz() {
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

    let mfz_file = std::env::temp_dir().join(format!(
        "test_pointer_{}.mfz",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    fs::write(&mfz_file, "env = stg\n").unwrap();

    let (name, env) = loader
        .load_from_file_or_name(mfz_file.to_str().unwrap())
        .unwrap();
    assert_eq!(name, "stg");
    assert_eq!(env.profile.as_deref(), Some("dev"));

    let _ = fs::remove_file(&ini_path);
    let _ = fs::remove_file(&mfz_file);
}

#[test]
fn test_config_load_standalone_mfz() {
    let mfz_content = r#"
profile = prd_profile
instance = i-99999
rdp_host = prd-rdp
remote_port = 3389
local_port = 13390
"#;
    let mfz_file = std::env::temp_dir().join(format!(
        "test_standalone_{}.mfz",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    fs::write(&mfz_file, mfz_content).unwrap();

    let dummy_ini = std::env::temp_dir().join("non_existent_mfz.ini");
    let loader = ConfigLoader::new(&dummy_ini);

    let (name, env) = loader
        .load_from_file_or_name(mfz_file.to_str().unwrap())
        .unwrap();
    assert!(name.starts_with("test_standalone"));
    assert_eq!(env.profile.as_deref(), Some("prd_profile"));
    assert_eq!(env.local_port, 13390);

    let _ = fs::remove_file(&mfz_file);
}

#[test]
fn test_config_load_stem_fallback_mfz() {
    let ini_content = r#"
[prd]
profile = prod_profile
instance = i-55555
rdp_host = prd-rdp
remote_port = 3389
local_port = 13390
"#;
    let ini_path = create_temp_config(ini_content);
    let loader = ConfigLoader::new(&ini_path);

    let mfz_file = std::env::temp_dir().join(format!("prd.mfz"));
    fs::write(&mfz_file, "").unwrap(); // empty file

    let (name, env) = loader
        .load_from_file_or_name(mfz_file.to_str().unwrap())
        .unwrap();
    assert_eq!(name, "prd");
    assert_eq!(env.profile.as_deref(), Some("prod_profile"));

    let _ = fs::remove_file(&ini_path);
    let _ = fs::remove_file(&mfz_file);
}

#[test]
fn test_standalone_config_parsing() {
    let content = r#"
[stg_standalone]
aws_access_key_id=AKIAIOSFODNN7EXAMPLE
aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
region=ap-northeast-1
serial=arn:aws:iam::123:mfa/waka
instance=i-abc
remote_port=3389
local_port=13389
rdp_host=stg-rdp
"#;
    let file = create_temp_config(content);
    let loader = ConfigLoader::new(&file);
    let cfg = loader.load("stg_standalone").unwrap();

    assert_eq!(
        cfg.aws_access_key_id.as_deref(),
        Some("AKIAIOSFODNN7EXAMPLE")
    );
    assert_eq!(
        cfg.aws_secret_access_key.as_deref(),
        Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
    );
    assert_eq!(cfg.region.as_deref(), Some("ap-northeast-1"));
    assert_eq!(cfg.profile, None);
    assert_eq!(
        cfg.resolve_aws_secret_key("stg_standalone")
            .unwrap()
            .as_deref(),
        Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
    );

    let _ = fs::remove_file(&file);
}

#[test]
fn test_missing_auth_error() {
    let content = r#"
[invalid_env]
instance=i-abc
remote_port=3389
local_port=13389
rdp_host=stg-rdp
"#;
    let file = create_temp_config(content);
    let loader = ConfigLoader::new(&file);
    let err = loader.load("invalid_env").unwrap_err();
    assert!(err
        .to_string()
        .contains("must specify either `aws_access_key_id`"));

    let _ = fs::remove_file(&file);
}
