use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_config::BehaviorVersion;
use aws_sdk_sts::Client as StsClient;
use chrono::{DateTime, Utc};
use tokio::time::sleep;
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

#[derive(Debug, Clone)]
pub struct MfaCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration: DateTime<Utc>,
}

impl MfaCredentials {
    pub fn is_valid(&self) -> bool {
        (self.expiration - Utc::now()).num_seconds() > SESSION_EXPIRY_BUFFER_SECS
    }

    pub fn as_env_pairs(&self) -> [(&'static str, &str); 3] {
        [
            ("AWS_ACCESS_KEY_ID", &self.access_key_id),
            ("AWS_SECRET_ACCESS_KEY", &self.secret_access_key),
            ("AWS_SESSION_TOKEN", &self.session_token),
        ]
    }
}

// ─── MfaSessionManager ───────────────────────────────────────────────────────

pub struct MfaSessionManager {
    base_profile: String,
    serial: String,
    secret: String,
    cached: Option<MfaCredentials>,
}

impl MfaSessionManager {
    pub fn new(base_profile: String, serial: String, secret: String) -> Self {
        Self {
            base_profile,
            serial,
            secret,
            cached: None,
        }
    }

    pub async fn get_credentials(&mut self, spinner: Option<&Spinner>) -> Result<MfaCredentials> {
        if let Some(ref creds) = self.cached {
            if creds.is_valid() {
                let mins = (creds.expiration - Utc::now()).num_seconds() / 60;
                info!("Reusing cached MFA session ({}m remaining)", mins);
                return Ok(creds.clone());
            }
            info!("Cached MFA session expiring soon — refreshing");
        }
        let creds = self.acquire_session_token(spinner).await?;
        self.cached = Some(creds.clone());
        Ok(creds)
    }

    async fn acquire_session_token(&self, spinner: Option<&Spinner>) -> Result<MfaCredentials> {
        info!("Requesting new AWS session token via MFA...");
        let otp = self.safe_totp(spinner).await?;

        let config = aws_config::defaults(BehaviorVersion::latest())
            .profile_name(&self.base_profile)
            .load()
            .await;

        let response = StsClient::new(&config)
            .get_session_token()
            .serial_number(&self.serial)
            .token_code(&otp)
            .send()
            .await
            .map_err(|e| AppError::Mfa(format!("STS GetSessionToken failed: {e}")))?;

        let creds = response
            .credentials()
            .ok_or_else(|| AppError::Mfa("STS response contained no credentials".into()))?;

        let expiry_secs = creds.expiration().secs();

        let expiration = DateTime::from_timestamp(expiry_secs, 0)
            .ok_or_else(|| AppError::Mfa("Invalid expiration timestamp from STS".into()))?;

        Ok(MfaCredentials {
            access_key_id: creds.access_key_id().to_owned(),
            secret_access_key: creds.secret_access_key().to_owned(),
            session_token: creds.session_token().to_owned(),
            expiration,
        })
    }

    /// Generate a TOTP code, waiting safely away from the window boundary.
    pub async fn safe_totp(&self, spinner: Option<&Spinner>) -> Result<String> {
        let totp = build_totp(self.secret.as_bytes())?;

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
