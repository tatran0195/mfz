use mfz::rdp::{build_rdp_file_content, RdpProfileOptions};

#[test]
fn test_build_rdp_content_with_optimization_and_drives() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: true,
        username: Some("Administrator"),
        drives_to_redirect: Some("*"),
        optimize_connection: true,
    };

    let content = build_rdp_file_content(&opts);

    assert!(content.contains("full address:s:stg-rdp:13389"));
    assert!(content.contains("screen mode id:i:2"));
    assert!(content.contains("username:s:Administrator"));
    assert!(content.contains("drivestoredirect:s:*"));
    assert!(content.contains("compression:i:1"));
    assert!(content.contains("bitmapcachepersistenable:i:1"));
    assert!(content.contains("connection type:i:6"));
    assert!(content.contains("audiomode:i:2"));
}

#[test]
fn test_build_rdp_content_no_optimization_and_specific_drives() {
    let opts = RdpProfileOptions {
        host: "prd-rdp",
        port: 3389,
        fullscreen: false,
        username: None,
        drives_to_redirect: Some("C:;D:"),
        optimize_connection: false,
    };

    let content = build_rdp_file_content(&opts);

    assert!(content.contains("full address:s:prd-rdp:3389"));
    assert!(content.contains("screen mode id:i:1"));
    assert!(content.contains("drivestoredirect:s:C:;D:"));
    assert!(!content.contains("connection type:i:6"));
    assert!(!content.contains("compression:i:1"));
}

#[test]
fn test_build_rdp_content_disabled_drives() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: false,
        username: None,
        drives_to_redirect: None,
        optimize_connection: true,
    };

    let content = build_rdp_file_content(&opts);
    assert!(content.contains("drivestoredirect:s:"));
}
