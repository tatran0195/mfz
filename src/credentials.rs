use keyring::Entry;

use crate::error::{AppError, Result};

pub struct CredentialStore;

impl CredentialStore {
    pub fn set_totp_secret(env_name: &str, secret: &str) -> Result<()> {
        let entry = Entry::new("mfz-cli", &format!("totp-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        entry
            .set_password(secret)
            .map_err(|e| AppError::Config(format!("Failed to save secret: {e}")))?;
        Ok(())
    }

    pub fn get_totp_secret(env_name: &str) -> Result<Option<String>> {
        let entry = Entry::new("mfz-cli", &format!("totp-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Config(format!("Failed to read secret: {e}"))),
        }
    }

    pub fn set_rdp_password(env_name: &str, password: &str) -> Result<()> {
        let entry = Entry::new("mfz-cli", &format!("rdp-password-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        entry
            .set_password(password)
            .map_err(|e| AppError::Config(format!("Failed to save rdp password: {e}")))?;
        Ok(())
    }

    pub fn get_rdp_password(env_name: &str) -> Result<Option<String>> {
        let entry = Entry::new("mfz-cli", &format!("rdp-password-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Config(format!(
                "Failed to read rdp password: {e}"
            ))),
        }
    }

    pub fn set_aws_secret_key(env_name: &str, secret_key: &str) -> Result<()> {
        let entry = Entry::new("mfz-cli", &format!("aws-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        entry
            .set_password(secret_key)
            .map_err(|e| AppError::Config(format!("Failed to save AWS secret key: {e}")))?;
        Ok(())
    }

    pub fn get_aws_secret_key(env_name: &str) -> Result<Option<String>> {
        let entry = Entry::new("mfz-cli", &format!("aws-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Config(format!("Failed to read AWS secret key: {e}"))),
        }
    }

    pub fn delete_secrets(env_name: &str) -> Result<()> {
        let totp_entry = Entry::new("mfz-cli", &format!("totp-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        let _ = totp_entry.delete_credential();

        let rdp_entry = Entry::new("mfz-cli", &format!("rdp-password-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        let _ = rdp_entry.delete_credential();

        let aws_entry = Entry::new("mfz-cli", &format!("aws-secret-{}", env_name))
            .map_err(|e| AppError::Config(format!("Keyring error: {e}")))?;
        let _ = aws_entry.delete_credential();

        Ok(())
    }
}
