//! Shared Base-32 key handling: enrollment, QR round-trips and MFA must agree.
use crate::error::{AppError, Result};
use base32::{decode, Alphabet};

pub fn decode_secret(secret: &str) -> Result<Vec<u8>> {
    let canonical = secret.trim().to_ascii_uppercase();
    decode(Alphabet::Rfc4648 { padding: false }, &canonical)
        .or_else(|| decode(Alphabet::Rfc4648 { padding: true }, &canonical))
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| AppError::Mfa("MFA secret is not valid, nonempty Base-32".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn padding_case_and_invalid_secrets_are_handled_consistently() {
        assert_eq!(decode_secret("my======").unwrap(), b"f");
        assert_eq!(decode_secret("my").unwrap(), b"f");
        assert!(decode_secret("").is_err());
        assert!(decode_secret("not-valid").is_err());
        assert!(decode_secret("A").is_err());
    }
}
