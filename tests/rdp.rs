use mfz::rdp::{build_rdp_file_content, OptimizationSettings, RdpProfileOptions};

#[test]
fn test_build_rdp_content_default_fullscreen_no_optimization() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: true,
        username: Some("Administrator"),
        drives_to_redirect: Some("Z:"),
        optimization: OptimizationSettings::default(),
    };

    let content = build_rdp_file_content(&opts).unwrap();

    assert!(content.contains("full address:s:stg-rdp:13389"));
    assert!(content.contains("screen mode id:i:2"));
    assert!(content.contains("username:s:Administrator"));
    assert!(content.contains("drivestoredirect:s:Z:"));
    assert!(content.contains("dynamic resolution:i:1"));
    assert!(content.contains("smart sizing:i:1"));
    assert!(!content.contains("connection type:i:6"));
    assert!(!content.contains("compression:i:1"));
}

#[test]
fn test_build_rdp_content_windowed_speed_optimization() {
    let opts = RdpProfileOptions {
        host: "prd-rdp",
        port: 3389,
        fullscreen: false,
        username: None,
        drives_to_redirect: Some("C:;D:"),
        optimization: OptimizationSettings::from_profile_name(Some("speed")).unwrap(),
    };

    let content = build_rdp_file_content(&opts).unwrap();

    assert!(content.contains("full address:s:prd-rdp:3389"));
    assert!(content.contains("screen mode id:i:1"));
    assert!(content.contains("dynamic resolution:i:1"));
    assert!(content.contains("smart sizing:i:1"));
    assert!(content.contains("drivestoredirect:s:C:;D:"));
    assert!(content.contains("connection type:i:6"));
    assert!(content.contains("compression:i:1"));
    assert!(content.contains("bitmapcachepersistenable:i:1"));
    assert!(content.contains("audiomode:i:2"));
}

#[test]
fn test_build_rdp_content_balanced_with_audio_override() {
    let mut opt = OptimizationSettings::from_profile_name(Some("balanced")).unwrap();
    opt.audio = Some(false); // override audio off in balanced mode

    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: true,
        username: None,
        drives_to_redirect: None,
        optimization: opt,
    };

    let content = build_rdp_file_content(&opts).unwrap();
    assert!(content.contains("connection type:i:6"));
    assert!(content.contains("audiomode:i:2"));
    assert!(content.contains("drivestoredirect:s:"));
}

#[test]
fn test_resolve_drives_to_redirect() {
    use mfz::rdp::resolve_drives_to_redirect;

    assert_eq!(
        resolve_drives_to_redirect(Some("none"), None).unwrap(),
        None
    );
    assert_eq!(
        resolve_drives_to_redirect(Some("C:"), Some('Z')).unwrap(),
        Some("C:;Z:".to_string())
    );
    assert_eq!(
        resolve_drives_to_redirect(None, Some('Z')).unwrap(),
        Some("Z:".to_string())
    );
    assert_eq!(
        resolve_drives_to_redirect(Some("none"), Some('Z')).unwrap(),
        Some("Z:".to_string())
    );

    // The operating system owns which drives exist; do not assert a developer's
    // C:/D:/empty-E: layout on every platform. Exact wildcard/empty-drive cases
    // are separately tested at the production enumeration boundary.
    let expected = mfz::rdp::get_ready_local_drives().unwrap();
    assert_eq!(
        resolve_drives_to_redirect(Some("*"), None).unwrap(),
        (!expected.is_empty()).then(|| expected.join(";"))
    );
}
