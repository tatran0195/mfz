//! Authentication, MFA sessions, virtual device enrollment, TOTP, and secure credential storage.

pub mod credentials;
pub mod enrollment;
pub mod mfa;
pub mod otp;
pub mod qr;

pub use credentials::CredentialStore;
pub use enrollment::{register, RegistrationOptions};
pub use mfa::{build_totp, AwsAuthSource, MfaCredentials, MfaSessionManager};
pub use otp::decode_secret;
pub use qr::{
    decode as decode_qr, extract_secret_from_uri, generate as generate_qr, print_terminal,
    save_png, save_png_with_overwrite, GeneratedQr, OtpSecret,
};
