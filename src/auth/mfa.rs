use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_config::BehaviorVersion;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_sdk_sts::{
    error::{ProvideErrorMetadata, SdkError},
    Client as StsClient,
};
use chrono::{DateTime, Utc};
use tokio::{sync::OnceCell, time::sleep};
use totp_rs::{Algorithm, TOTP};
use tracing::{info, warn};

use crate::{
    error::{AppError, Result},
    ui::Spinner,
};

const TOTP_SAFETY_MARGIN_SECS: f64 = 5.0;
const TOTP_INTERVAL_SECS: f64 = 30.0;
const SESSION_EXPIRY_BUFFER_SECS: i64 = 60;

// ─── MfaCredentials ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct MfaCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub expiration: DateTime<Utc>,
}

impl MfaCredentials {
    pub fn is_valid(&self) -> bool {
        (self.expiration - Utc::now()).num_seconds() > SESSION_EXPIRY_BUFFER_SECS
    }

    /// Preserve AWS token optionality; an empty security-token header is not a
    /// long-lived IAM credential. Every service client uses this conversion.
    pub fn to_sdk_credentials(&self) -> aws_credential_types::Credentials {
        let token = self.session_token.clone().filter(|token| !token.is_empty());
        let expiry = token.as_ref().map(|_| SystemTime::from(self.expiration));
        aws_credential_types::Credentials::new(
            self.access_key_id.clone(),
            self.secret_access_key.clone(),
            token,
            expiry,
            "mfz-native",
        )
    }
}

// ─── MfaSessionManager ───────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub enum AwsAuthSource {
    /// Standalone: direct IAM API credentials from mfz.ini or Keyring
    Direct {
        access_key_id: String,
        secret_access_key: String,
    },
    /// Legacy: named AWS profile from ~/.aws/credentials
    Profile(String),
}

pub struct MfaSessionManager {
    auth_source: AwsAuthSource,
    region: Option<String>,
    serial: Option<String>,
    secret: Option<String>,
    cached: Option<MfaCredentials>,
    sdk_config: OnceCell<aws_config::SdkConfig>,
}

impl MfaSessionManager {
    pub fn new(
        auth_source: AwsAuthSource,
        region: Option<String>,
        serial: Option<String>,
        secret: Option<String>,
    ) -> Self {
        Self {
            auth_source,
            region,
            serial,
            secret,
            cached: None,
            sdk_config: OnceCell::new(),
        }
    }

    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }

    /// One SDK configuration/provider chain for MFA, EC2 and SSM. In particular,
    /// a selected named profile supplies the REGION as well as credentials.
    pub async fn aws_config(&self) -> &aws_config::SdkConfig {
        self.sdk_config
            .get_or_init(|| async {
                let mut loader = aws_config::defaults(BehaviorVersion::latest());
                loader = match &self.auth_source {
                    AwsAuthSource::Direct {
                        access_key_id,
                        secret_access_key,
                    } => loader.credentials_provider(aws_credential_types::Credentials::new(
                        access_key_id,
                        secret_access_key,
                        None,
                        None,
                        "mfz-direct",
                    )),
                    AwsAuthSource::Profile(profile) => loader.profile_name(profile),
                };
                if let Some(region) = &self.region {
                    loader = loader.region(aws_config::Region::new(region.clone()));
                }
                loader.load().await
            })
            .await
    }

    /// Preserve refresh-capable profile/SSO providers unless an explicit MFA
    /// session must replace them. The WebSocket still signs one start snapshot.
    pub async fn service_config(&self, credentials: &MfaCredentials) -> aws_config::SdkConfig {
        let base = self.aws_config().await;
        if self.serial.is_some() {
            base.to_builder()
                .credentials_provider(SharedCredentialsProvider::new(
                    credentials.to_sdk_credentials(),
                ))
                .build()
        } else {
            base.clone()
        }
    }

    fn validate_auth(&self) -> Result<()> {
        match (&self.serial, &self.secret) {
            (None, None) => {}
            (Some(serial), Some(secret))
                if !serial.trim().is_empty() && !secret.trim().is_empty() => {}
            _ => return Err(AppError::Mfa(
                "MFA requires both a nonempty serial and TOTP secret; refusing a non-MFA fallback"
                    .into(),
            )),
        }
        match &self.auth_source {
            AwsAuthSource::Direct {
                access_key_id,
                secret_access_key,
            } if access_key_id.trim().is_empty() || secret_access_key.trim().is_empty() => Err(
                AppError::Mfa("Direct AWS credentials must not be empty".into()),
            ),
            AwsAuthSource::Profile(profile) if profile.trim().is_empty() => {
                Err(AppError::Mfa("AWS profile must not be empty".into()))
            }
            _ => Ok(()),
        }
    }

    pub async fn get_credentials(&mut self, spinner: Option<&Spinner>) -> Result<MfaCredentials> {
        self.validate_auth()?;
        if let Some(ref creds) = self.cached {
            if creds.is_valid() {
                let mins = (creds.expiration - Utc::now()).num_seconds() / 60;
                info!("Reusing cached MFA/SSO session ({}m remaining)", mins);
                return Ok(creds.clone());
            }
            info!("Cached session expiring soon — refreshing");
        }

        let creds = if self.serial.is_some() && self.secret.is_some() {
            // IAM User MFA mode
            self.acquire_session_token(spinner).await?
        } else {
            // SSO or direct profile mode
            self.load_from_profile(spinner).await?
        };

        if creds.access_key_id.is_empty() || creds.secret_access_key.is_empty() || !creds.is_valid()
        {
            return Err(AppError::Mfa(
                "AWS returned empty or already expiring credentials".into(),
            ));
        }
        self.cached = Some(creds.clone());
        Ok(creds)
    }

    async fn acquire_session_token(&self, spinner: Option<&Spinner>) -> Result<MfaCredentials> {
        let serial = self
            .serial
            .as_ref()
            .ok_or_else(|| AppError::Mfa("MFA serial required for STS flow".into()))?;
        let secret = self
            .secret
            .as_ref()
            .ok_or_else(|| AppError::Mfa("TOTP secret required for STS flow".into()))?;

        info!("Requesting new AWS session token via MFA...");
        let otp = self.safe_totp(secret, spinner).await?;

        let config = self.aws_config().await;

        let response = StsClient::new(config)
            .get_session_token()
            .serial_number(serial)
            .token_code(&otp)
            .send()
            .await
            .map_err(|e| {
                warn!("STS GetSessionToken failed: {}", extract_msg(&e));
                AppError::Mfa(format!("STS GetSessionToken failed: {}", extract_msg(&e)))
            })?;

        let creds = response
            .credentials()
            .ok_or_else(|| AppError::Mfa("STS response contained no credentials".into()))?;

        let expiry_secs = creds.expiration().secs();
        let expiration = DateTime::from_timestamp(expiry_secs, 0)
            .ok_or_else(|| AppError::Mfa("Invalid expiration timestamp from STS".into()))?;

        Ok(MfaCredentials {
            access_key_id: creds.access_key_id().to_owned(),
            secret_access_key: creds.secret_access_key().to_owned(),
            session_token: Some(creds.session_token().to_owned()),
            expiration,
        })
    }

    async fn load_from_profile(&self, _spinner: Option<&Spinner>) -> Result<MfaCredentials> {
        match &self.auth_source {
            AwsAuthSource::Direct {
                access_key_id,
                secret_access_key,
            } => {
                info!("Using direct AWS credentials without MFA session token...");
                Ok(MfaCredentials {
                    access_key_id: access_key_id.clone(),
                    secret_access_key: secret_access_key.clone(),
                    session_token: None,
                    expiration: Utc::now() + chrono::Duration::hours(12),
                })
            }
            AwsAuthSource::Profile(base_profile) => {
                info!(
                    "Retrieving credentials from AWS profile `{}`...",
                    base_profile
                );

                let config = self.aws_config().await;

                let provider = config.credentials_provider().ok_or_else(|| {
                    AppError::Mfa(format!(
                        "No credential provider found for profile `{}`",
                        base_profile
                    ))
                })?;

                let creds = provider.provide_credentials().await.map_err(|e| {
                    AppError::Mfa(format!(
                        "Failed to retrieve credentials for profile `{}`: {}",
                        base_profile, e
                    ))
                })?;

                let expiration = creds
                    .expiry()
                    .and_then(|e| {
                        e.duration_since(UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_secs() as i64)
                    })
                    .and_then(|secs| DateTime::from_timestamp(secs, 0))
                    .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));

                Ok(MfaCredentials {
                    access_key_id: creds.access_key_id().to_owned(),
                    secret_access_key: creds.secret_access_key().to_owned(),
                    session_token: creds.session_token().map(str::to_owned),
                    expiration,
                })
            }
        }
    }

    /// Generate a TOTP code, waiting safely away from the window boundary.
    pub async fn safe_totp(&self, secret: &str, spinner: Option<&Spinner>) -> Result<String> {
        let secret_bytes = super::otp::decode_secret(secret)?;

        let totp = build_totp(&secret_bytes)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let remaining = TOTP_INTERVAL_SECS - (now % TOTP_INTERVAL_SECS);

        if remaining < TOTP_SAFETY_MARGIN_SECS {
            let wait = remaining + 0.5;
            warn!(
                "TOTP window expiring in {:.1}s — waiting {:.1}s",
                remaining, wait
            );
            if let Some(s) = spinner {
                s.set_message(format!("Waiting {:.1}s for new TOTP window...", wait));
            }
            sleep(Duration::from_secs_f64(wait)).await;
            if let Some(s) = spinner {
                s.set_message("Refreshing MFA credentials...");
            }
        }

        totp.generate_current()
            .map_err(|e| AppError::Mfa(format!("TOTP generation failed: {e}")))
    }
}

/// Build a TOTP instance from raw secret bytes (shared with qr module).
pub fn build_totp(secret_bytes: &[u8]) -> Result<TOTP> {
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret_bytes.to_vec(),
        None,
        "".to_string(),
    )
    .map_err(|e| AppError::Mfa(format!("Invalid TOTP secret: {e}")))
}

fn extract_msg<E>(err: &SdkError<E>) -> String
where
    E: ProvideErrorMetadata,
{
    if let SdkError::ServiceError(context) = err {
        if let Some(msg) = context.err().message() {
            return msg.to_string();
        }
    }
    err.to_string()
}

impl std::fmt::Debug for MfaCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MfaCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"[REDACTED]")
            .field("session_token", &"[REDACTED]")
            .field("expiration", &self.expiration)
            .finish()
    }
}

impl std::fmt::Debug for AwsAuthSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Profile(name) => f.debug_tuple("Profile").field(name).finish(),
            Self::Direct { access_key_id, .. } => f
                .debug_struct("Direct")
                .field("access_key_id", access_key_id)
                .field("secret_access_key", &"[REDACTED]")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn incomplete_mfa_and_empty_auth_fail_before_provider_or_network_access() {
        for (serial, secret) in [
            (Some("serial"), None),
            (None, Some("JBSWY3DPEHPK3PXP")),
            (Some(""), Some("JBSWY3DPEHPK3PXP")),
        ] {
            let mut manager = MfaSessionManager::new(
                AwsAuthSource::Profile("intentionally-nonexistent-profile".into()),
                None,
                serial.map(str::to_owned),
                secret.map(str::to_owned),
            );
            let error = manager.get_credentials(None).await.unwrap_err();
            assert!(error.to_string().contains("MFA requires both"));
            assert!(manager.sdk_config.get().is_none());
        }
        let mut manager = MfaSessionManager::new(
            AwsAuthSource::Direct {
                access_key_id: "".into(),
                secret_access_key: "dummy".into(),
            },
            None,
            None,
            None,
        );
        assert!(manager.get_credentials(None).await.is_err());
        assert!(manager.sdk_config.get().is_none());
    }

    #[tokio::test]
    async fn direct_non_mfa_credentials_do_not_invent_a_session_token_or_sdk_expiry() {
        let mut manager = MfaSessionManager::new(
            AwsAuthSource::Direct {
                access_key_id: "AKIDEXAMPLE".into(),
                secret_access_key: "dummy-private-key".into(),
            },
            None,
            None,
            None,
        );
        let credentials = manager.get_credentials(None).await.unwrap();
        assert!(credentials.session_token.is_none());
        let sdk = credentials.to_sdk_credentials();
        assert!(sdk.session_token().is_none());
        assert!(sdk.expiry().is_none());
        assert!(!format!("{credentials:?}").contains("dummy-private-key"));
        assert!(!format!("{:?}", manager.auth_source).contains("dummy-private-key"));
    }
}
