use mfz::credentials::CredentialStore;

#[test]
fn test_aws_secret_keyring_roundtrip() {
    let test_env = "test_env_keyring_spec";
    let test_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    // Set
    let set_res = CredentialStore::set_aws_secret_key(test_env, test_key);
    assert!(set_res.is_ok());

    // Get
    let read_key = CredentialStore::get_aws_secret_key(test_env).unwrap();
    assert_eq!(read_key.as_deref(), Some(test_key));

    // Delete
    let del_res = CredentialStore::delete_secrets(test_env);
    assert!(del_res.is_ok());

    // Verify deleted
    let after_del = CredentialStore::get_aws_secret_key(test_env).unwrap();
    assert_eq!(after_del, None);
}
