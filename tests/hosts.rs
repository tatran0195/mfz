use std::fs;

use mfz::hosts::HostsManager;

#[test]
fn test_hosts_add_new_entry() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "hosts_test_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));

    // Initial content with comments
    fs::write(&file_path, "# Test hosts file\n127.0.0.1 localhost\n").unwrap();

    let mgr = HostsManager::with_path(file_path.clone());

    // Add a new entry
    mgr.add_host("127.0.0.1", "test-srv").unwrap();

    let result = fs::read_to_string(&file_path).unwrap();
    assert!(result.contains("127.0.0.1\ttest-srv"));

    fs::remove_file(&file_path).unwrap();
}

#[test]
fn test_hosts_skip_existing() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "hosts_test_skip_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));

    fs::write(&file_path, "127.0.0.1\texisting-srv\n").unwrap();

    let mgr = HostsManager::with_path(file_path.clone());

    // Add same entry again
    mgr.add_host("1.2.3.4", "existing-srv").unwrap();

    let result = fs::read_to_string(&file_path).unwrap();
    // It should not have been added again, and IP should not have changed (it skips by hostname)
    assert!(!result.contains("1.2.3.4"));

    fs::remove_file(&file_path).unwrap();
}

#[test]
fn test_hosts_has_host_and_ensure() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "hosts_test_ensure_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));

    fs::write(&file_path, "127.0.0.1\tlocalhost\n").unwrap();
    let mgr = HostsManager::with_path(file_path.clone());

    assert!(mgr.has_host("localhost").unwrap());
    assert!(!mgr.has_host("app-rdp").unwrap());

    mgr.ensure_host("127.0.0.1", "app-rdp").unwrap();
    assert!(mgr.has_host("app-rdp").unwrap());

    // Calling ensure_host again should succeed without duplicating
    mgr.ensure_host("127.0.0.1", "app-rdp").unwrap();

    let content = fs::read_to_string(&file_path).unwrap();
    let matches: Vec<_> = content.lines().filter(|l| l.contains("app-rdp")).collect();
    assert_eq!(matches.len(), 1);

    fs::remove_file(&file_path).unwrap();
}

#[test]
fn test_hosts_ensure_hosts_batch() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "hosts_test_batch_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));

    fs::write(&file_path, "127.0.0.1\tlocalhost\n").unwrap();
    let mgr = HostsManager::with_path(file_path.clone());

    let entries = [("127.0.0.1", "host-a"), ("127.0.0.1", "host-b")];
    mgr.ensure_hosts(&entries, None).unwrap();

    assert!(mgr.has_host("host-a").unwrap());
    assert!(mgr.has_host("host-b").unwrap());

    fs::remove_file(&file_path).unwrap();
}

