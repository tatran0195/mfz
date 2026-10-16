use keyring::Entry;

use crate::{
    error::{AppError, Result},
    validation,
};

pub struct CredentialStore;

impl CredentialStore {
    fn entry(env_name: &str, kind: &str) -> Result<Entry> {
        let name = validation::environment_name(env_name)?;
        if !cfg!(windows) {
            // keyring's fallback backend is a per-entry, nonpersistent mock.
            // Never report a durable write or remove plaintext using that backend.
            return Err(AppError::Config("Persistent secure storage is supported only by the Windows Credential Manager backend on this build".into()));
        }
        Entry::new("mfz-cli", &format!("{kind}-{name}")).map_err(|error| {
            AppError::Config(format!("Cannot open Windows secure-store entry: {error}"))
        })
    }

    fn set(env_name: &str, kind: &str, value: &str) -> Result<()> {
        if kind != "rdp-password" && value.trim().is_empty() {
            return Err(AppError::Config(
                "AWS/TOTP secrets must not be empty".into(),
            ));
        }
        Self::entry(env_name, kind)?
            .set_password(value)
            .map_err(|error| {
                AppError::Config(format!(
                    "Cannot write Windows secure-store credential: {error}"
                ))
            })?;
        // Reopen, rather than reading the same entry object. This detects the
        // mock/nonpersistent-store class of failure as well as corrupt writes.
        if Self::get(env_name, kind)?.as_deref() != Some(value) {
            return Err(AppError::Config(
                "Windows secure-store readback did not match; plaintext must be retained".into(),
            ));
        }
        Ok(())
    }

    fn get(env_name: &str, kind: &str) -> Result<Option<String>> {
        match Self::entry(env_name, kind)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(AppError::Config(format!(
                "Cannot read Windows secure-store credential: {error}"
            ))),
        }
    }

    pub fn set_totp_secret(env_name: &str, secret: &str) -> Result<()> {
        Self::set(env_name, "totp-secret", secret)
    }
    pub fn get_totp_secret(env_name: &str) -> Result<Option<String>> {
        Self::get(env_name, "totp-secret")
    }
    pub fn set_rdp_password(env_name: &str, password: &str) -> Result<()> {
        Self::set(env_name, "rdp-password", password)
    }
    pub fn get_rdp_password(env_name: &str) -> Result<Option<String>> {
        Self::get(env_name, "rdp-password")
    }
    pub fn set_aws_secret_key(env_name: &str, secret: &str) -> Result<()> {
        Self::set(env_name, "aws-secret", secret)
    }
    pub fn get_aws_secret_key(env_name: &str) -> Result<Option<String>> {
        Self::get(env_name, "aws-secret")
    }

    pub fn delete_secrets(env_name: &str) -> Result<()> {
        let mut failures = Vec::new();
        for kind in ["totp-secret", "rdp-password", "aws-secret"] {
            match Self::entry(env_name, kind)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(error) => failures.push(format!("{kind}: {error}")),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Config(format!(
                "Secure-store cleanup incomplete: {}",
                failures.join("; ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn unsupported_backend_is_not_successful_persistence() {
        assert!(CredentialStore::set_totp_secret("test", "JBSWY3DPEHPK3PXP").is_err());
        assert!(CredentialStore::get_totp_secret("test").is_err());
        assert!(CredentialStore::delete_secrets("test").is_err());
    }

    #[test]
    fn invalid_environment_names_never_reach_the_backend() {
        assert!(CredentialStore::set_aws_secret_key("../other", "secret").is_err());
        assert!(CredentialStore::get_rdp_password("bad\nname").is_err());
    }
}
