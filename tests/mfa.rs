use chrono::{Duration, Utc};
use mfz::mfa::{build_totp, MfaCredentials};

#[test]
fn test_mfa_credentials_is_valid() {
    let creds = MfaCredentials {
        access_key_id: "test".into(),
        secret_access_key: "test".into(),
        session_token: "test".into(),
        expiration: Utc::now() + Duration::minutes(5),
    };
    assert!(creds.is_valid());

    let expired = MfaCredentials {
        access_key_id: "test".into(),
        secret_access_key: "test".into(),
        session_token: "test".into(),
        expiration: Utc::now() - Duration::minutes(5),
    };
    assert!(!expired.is_valid());

    let slightly_valid = MfaCredentials {
        access_key_id: "test".into(),
        secret_access_key: "test".into(),
        session_token: "test".into(),
        expiration: Utc::now() + Duration::seconds(65), // 60s buffer in mfa.rs
    };
    assert!(slightly_valid.is_valid());
}

#[test]
fn test_build_totp_reproducibility() {
    let secret = "SOMEBASE32SECRET".as_bytes();
    let totp1 = build_totp(secret).unwrap();
    let totp2 = build_totp(secret).unwrap();

    // Check if they generate the same code for the same time
    assert_eq!(
        totp1.generate_current().unwrap(),
        totp2.generate_current().unwrap()
    );
}
