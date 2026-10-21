#[cfg(windows)]
use mfz::credentials::CredentialStore;

#[cfg(windows)]
#[test]
#[ignore = "Requires an interactive Windows Credential Manager; explicitly run --test credentials_test -- --ignored (random disposable entries only)"]
fn test_aws_secret_keyring_roundtrip() {
    let test_env = format!("mfz_test_{}", uuid::Uuid::new_v4());
    let test_env = test_env.as_str();
    let test_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    struct Cleanup<'a>(&'a str);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            if let Err(error) = CredentialStore::delete_secrets(self.0) {
                eprintln!("Test credential cleanup failed: {error}");
            }
        }
    }
    let _cleanup = Cleanup(test_env);

    // Set
    let set_res = CredentialStore::set_aws_secret_key(test_env, test_key);
    assert!(set_res.is_ok());

    // Get
    let read_key = CredentialStore::get_aws_secret_key(test_env).unwrap();
    assert_eq!(read_key.as_deref(), Some(test_key));

    CredentialStore::set_rdp_username(test_env, "Administrator").unwrap();
    CredentialStore::set_rdp_password(test_env, "disposable-test-password").unwrap();
    assert_eq!(
        CredentialStore::get_rdp_username(test_env)
            .unwrap()
            .as_deref(),
        Some("Administrator")
    );
    assert_eq!(
        CredentialStore::get_rdp_password(test_env)
            .unwrap()
            .as_deref(),
        Some("disposable-test-password")
    );

    // Delete
    let del_res = CredentialStore::delete_secrets(test_env);
    assert!(del_res.is_ok());

    // Verify deleted
    let after_del = CredentialStore::get_aws_secret_key(test_env).unwrap();
    assert_eq!(after_del, None);
    assert_eq!(CredentialStore::get_rdp_username(test_env).unwrap(), None);
    assert_eq!(CredentialStore::get_rdp_password(test_env).unwrap(), None);
}
