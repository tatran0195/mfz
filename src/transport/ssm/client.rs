use std::time::Duration;

use aws_sdk_ssm::Client as SsmClient;
use tokio::time::sleep;
use tracing::{error, info, warn};

use super::{
    lifecycle::{port_is_in_use, SessionCleanup},
    tunnel::{start_port_forward_tunnel, SessionCredentials, SsmTunnelHandle},
};
use crate::{
    auth::mfa::{MfaCredentials, MfaSessionManager},
    error::{AppError, Result},
    ui::Spinner,
};

const MAX_RETRIES: u32 = 3;
const RETRY_BACKOFF_SECS: u64 = 3;

// ── SsmSession ─────────────────────────────────────────────────────────────────

pub struct SsmSession {
    pub instance: String,
    pub remote_port: u16,
    pub local_port: u16,
    region: Option<String>,
    tunnel: Option<SsmTunnelHandle>,
    cleanup: Option<SessionCleanup>,
}

impl SsmSession {
    pub fn new(
        instance: impl Into<String>,
        remote_port: u16,
        local_port: u16,
        region: Option<String>,
    ) -> Self {
        Self {
            instance: instance.into(),
            remote_port,
            local_port,
            region,
            tunnel: None,
            cleanup: None,
        }
    }

    /// Open the SSM data channel via the AWS SDK and start the native Rust
    /// port-forwarding tunnel.  No `aws` CLI or `session-manager-plugin`
    /// binary is required.
    pub async fn start(
        &mut self,
        mfa: &mut MfaSessionManager,
        spinner: Option<&Spinner>,
    ) -> Result<()> {
        if self.is_running() {
            warn!("SSM session already running — stopping first");
        }
        // Also release a failed previous start, not just a currently live tunnel.
        self.shutdown().await?;

        if self.local_port == 0 || self.remote_port == 0 {
            return Err(AppError::Config(
                "SSM local and remote ports must be nonzero".into(),
            ));
        }
        if Self::is_port_in_use(self.local_port) {
            return Err(AppError::Ssm(format!(
                "Local port {} is already in use. Close any lingering session or pick another port.",
                self.local_port
            )));
        }

        info!(
            "Starting SSM: {} → 127.0.0.1:{}",
            self.instance, self.local_port
        );

        let creds = mfa.get_credentials(spinner).await?;

        // ── Build SDK client with MFA session credentials ────────────────────
        let base = mfa.service_config(&creds).await;
        let mut builder = aws_sdk_ssm::config::Builder::from(&base);
        if let Some(region) = &self.region {
            builder = builder.region(aws_config::Region::new(region.clone()));
        }
        let ssm_cfg = builder.build();

        let ssm = SsmClient::from_conf(ssm_cfg);
        self.start_with_client(ssm, creds, spinner).await
    }

    // Shared by the production MFA path and real-SDK loopback integration tests.
    // Tests inject a configured SDK endpoint; no SDK calls/codecs are substituted.
    async fn start_with_client(
        &mut self,
        ssm: SsmClient,
        creds: MfaCredentials,
        spinner: Option<&Spinner>,
    ) -> Result<()> {
        // Use the SDK's resolved region (env/profile/provider chain included),
        // never a guessed ap-northeast-1 signing region for another endpoint.
        let region = ssm
            .config()
            .region()
            .map(|region| region.as_ref().to_owned())
            .ok_or_else(|| AppError::Config("No AWS region was resolved for SSM".into()))?;

        // ── Call SSM:StartSession ────────────────────────────────────────────
        if let Some(s) = spinner {
            s.set_message(format!("Requesting SSM session for {}…", self.instance));
        }

        let resp = ssm
            .start_session()
            .target(&self.instance)
            .document_name("AWS-StartPortForwardingSession")
            .parameters("portNumber", vec![self.remote_port.to_string()])
            .parameters("localPortNumber", vec![self.local_port.to_string()])
            .send()
            .await
            .map_err(|e| AppError::Ssm(format!("SSM StartSession failed: {e}")))?;

        let session_id = resp
            .session_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AppError::Ssm("StartSession returned no session ID".into()))?
            .to_owned();
        info!("SSM session created: {session_id}");
        // Install ownership BEFORE any further fallible/awaited operation, so
        // missing response fields, cancelled starts and failed binds all clean up.
        let cleanup_client = ssm.clone();
        let cleanup = SessionCleanup::new(async move {
            cleanup_client
                .terminate_session()
                .session_id(session_id)
                .send()
                .await
                .map(|_| ())
                .map_err(|error| AppError::Ssm(format!("SSM TerminateSession failed: {error}")))
        });
        let stream_url = resp.stream_url().filter(|url| !url.is_empty());
        let token_value = resp.token_value().filter(|token| !token.is_empty());
        let (Some(stream_url), Some(token_value)) = (stream_url, token_value) else {
            cleanup.finish().await?;
            return Err(AppError::Ssm(
                "StartSession returned no stream URL or token".into(),
            ));
        };
        let stream_url = stream_url.to_owned();
        let token_value = token_value.to_owned();

        let tunnel_creds = SessionCredentials {
            credentials: creds.to_sdk_credentials(),
            region,
        };

        let handle = start_port_forward_tunnel(
            stream_url,
            token_value,
            self.local_port,
            self.remote_port,
            tunnel_creds,
        )
        .await;
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                cleanup.finish().await?;
                return Err(error);
            }
        };
        cleanup.follow_transport(handle.closed_token());
        self.cleanup = Some(cleanup);
        self.tunnel = Some(handle);
        Ok(())
    }

    /// Compatibility/emergency stop; Drop requests bounded best-effort cleanup.
    /// Normal CLI paths await shutdown(), including remote SDK termination.
    pub fn stop(&mut self) {
        if let Some(mut tunnel) = self.tunnel.take() {
            tunnel.stop();
        }
        if let Some(mut cleanup) = self.cleanup.take() {
            cleanup.request();
        }
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        // Start termination alongside local cleanup rather than blocking either.
        if let Some(cleanup) = self.cleanup.as_mut() {
            cleanup.request();
        }
        let local = if let Some(mut tunnel) = self.tunnel.take() {
            tunnel.shutdown().await
        } else {
            Ok(())
        };
        let remote = if let Some(cleanup) = self.cleanup.take() {
            cleanup.finish().await
        } else {
            Ok(())
        };
        // Remote cleanup still ran even if the local transport reported an error.
        local.and(remote)
    }
    pub fn is_running(&self) -> bool {
        self.tunnel
            .as_ref()
            .is_some_and(SsmTunnelHandle::is_running)
    }
    pub async fn closed(&self) {
        if let Some(tunnel) = &self.tunnel {
            tunnel.closed().await;
        }
    }
    /// Wait on handshake+bind readiness, not a TCP probe or a polling timer.
    pub async fn wait_for_port(&mut self, spinner: Option<&Spinner>) -> bool {
        if let Some(spinner) = spinner {
            spinner.set_message("Waiting for authenticated native port forwarding…");
        }
        let Some(tunnel) = self.tunnel.as_mut() else {
            return false;
        };
        match tunnel.wait_until_ready().await {
            Ok(()) => true,
            Err(error) => {
                error!("SSM tunnel readiness failed: {error}");
                false
            }
        }
    }

    /// Returns `true` if `port` is already bound by *another* process.
    /// Used only as a pre-flight guard before starting the tunnel; never
    /// call this against our own tunnel port while the tunnel is running.
    pub fn is_port_in_use(port: u16) -> bool {
        port_is_in_use(port)
    }
}

impl Drop for SsmSession {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Retry wrapper ──────────────────────────────────────────────────────────────

pub async fn start_with_retries(ssm: &mut SsmSession, mfa: &mut MfaSessionManager) -> Result<()> {
    for attempt in 1..=MAX_RETRIES {
        let spinner = Spinner::new(format!(
            "SSM attempt {attempt}/{MAX_RETRIES} — forwarding 127.0.0.1:{}…",
            ssm.local_port
        ));

        if let Err(error) = ssm.start(mfa, Some(&spinner)).await {
            // SDK transient StartSession retries are already configured by the
            // SDK. Do not turn permission/MFA/config failures into a tight loop.
            spinner.failure("SSM API/start failed");
            ssm.shutdown().await?;
            return Err(error);
        }

        if ssm.wait_for_port(Some(&spinner)).await {
            spinner.success(format!(
                "Port forwarding ready → 127.0.0.1:{}",
                ssm.local_port
            ));
            return Ok(());
        }

        spinner.failure(format!("Attempt {attempt} failed"));
        ssm.shutdown().await?;

        if attempt < MAX_RETRIES {
            let ws = Spinner::new(format!(
                "Waiting {RETRY_BACKOFF_SECS}s before retry {}…",
                attempt + 1
            ));
            sleep(Duration::from_secs(RETRY_BACKOFF_SECS)).await;
            mfa.get_credentials(Some(&ws)).await?;
            ws.warning("Retrying…");
        }
    }

    Err(AppError::Ssm(format!(
        "SSM failed after {MAX_RETRIES} attempts"
    )))
}

#[cfg(test)]
#[path = "tests/sdk_tests.rs"]
mod sdk_tests;
