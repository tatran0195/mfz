use std::{net::TcpStream, time::Duration};

use aws_config::BehaviorVersion;
use aws_sdk_ssm::Client as SsmClient;
use aws_sdk_sts::config::Credentials as StsCredentials;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::{
    error::{AppError, Result},
    mfa::MfaSessionManager,
    ssm_tunnel::{start_port_forward_tunnel, SessionCredentials, SsmTunnelHandle},
    ui::Spinner,
};

const MAX_RETRIES: u32 = 3;
const PORT_WAIT_TIMEOUT: u64 = 30;
const PORT_POLL_INTERVAL_MS: u64 = 500;
const RETRY_BACKOFF_SECS: u64 = 3;

// ── SsmSession ─────────────────────────────────────────────────────────────────

pub struct SsmSession {
    pub instance: String,
    pub remote_port: u16,
    pub local_port: u16,
    region: Option<String>,
    tunnel: Option<SsmTunnelHandle>,
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
            self.stop();
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
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(ref r) = self.region {
            loader = loader.region(aws_config::Region::new(r.clone()));
        }
        let base = loader.load().await;

        let ssm_cfg = aws_sdk_ssm::config::Builder::from(&base)
            .credentials_provider(StsCredentials::new(
                &creds.access_key_id,
                &creds.secret_access_key,
                Some(creds.session_token.clone()),
                Some(std::time::SystemTime::from(creds.expiration)),
                "mfz-ssm",
            ))
            .build();

        let ssm = SsmClient::from_conf(ssm_cfg);

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

        let stream_url = resp.stream_url().unwrap_or_default().to_string();
        let token_value = resp.token_value().unwrap_or_default().to_string();
        let session_id = resp.session_id().unwrap_or_default().to_string();
        info!("SSM session created: {session_id}");

        // ── Launch native Rust tunnel ────────────────────────────────────────
        let region = self
            .region
            .clone()
            .unwrap_or_else(|| "ap-northeast-1".to_string());
        let tunnel_creds = SessionCredentials {
            access_key_id: creds.access_key_id.clone(),
            secret_access_key: creds.secret_access_key.clone(),
            session_token: creds.session_token.clone(),
            region,
        };

        let handle = start_port_forward_tunnel(
            stream_url,
            token_value,
            self.local_port,
            self.remote_port,
            tunnel_creds,
        )
        .await?;

        self.tunnel = Some(handle);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut t) = self.tunnel.take() {
            info!("Stopping SSM native tunnel");
            t.stop();
        }
    }

    pub fn is_running(&mut self) -> bool {
        self.tunnel
            .as_ref()
            .map(|t| t.is_running())
            .unwrap_or(false)
    }

    /// Block (async) until the tunnel signals its TCP listener is bound, or
    /// `PORT_WAIT_TIMEOUT` seconds elapse.
    ///
    /// Uses the `bound_tx` oneshot that `start_port_forward_tunnel` fires once
    /// `TcpListener::bind` succeeds — **not** a TCP probe connect.  A probe
    /// connect would open a real smux stream (sid=1) that immediately drops and
    /// sends `cmdFIN` to the EC2 SSM agent, destabilising the session before
    /// mstsc has a chance to connect.
    pub async fn wait_for_port(&mut self, spinner: Option<&Spinner>) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(PORT_WAIT_TIMEOUT);
        if let Some(t) = self.tunnel.as_mut() {
            if let Some(mut rx) = t.bound_rx.take() {
                loop {
                    if std::time::Instant::now() >= deadline {
                        return false;
                    }
                    if !t.is_running() {
                        error!("SSM tunnel exited unexpectedly");
                        return false;
                    }
                    tokio::select! {
                        res = &mut rx => {
                            return res.is_ok();
                        }
                        _ = sleep(Duration::from_millis(PORT_POLL_INTERVAL_MS)) => {
                            if let Some(s) = spinner {
                                s.set_message("Waiting for port forwarder to bind…");
                            }
                        }
                    }
                }
            }
        }

        // bound_rx already consumed (e.g. called twice) — treat as ready if
        // the tunnel task is still alive.
        self.is_running()
    }

    /// Kept for compatibility — wait returns -1 immediately since there is no
    /// child process; use `wait_for_port` + `is_running` instead.
    pub fn wait(&mut self) -> Result<i32> {
        Ok(-1)
    }

    /// Returns `true` if `port` is already bound by *another* process.
    /// Used only as a pre-flight guard before starting the tunnel; never
    /// call this against our own tunnel port while the tunnel is running.
    pub fn is_port_in_use(port: u16) -> bool {
        TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(500),
        )
        .is_ok()
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

        ssm.start(mfa, Some(&spinner)).await?;

        if ssm.wait_for_port(Some(&spinner)).await {
            spinner.success(format!(
                "Port forwarding ready → 127.0.0.1:{}",
                ssm.local_port
            ));
            return Ok(());
        }

        spinner.failure(format!("Attempt {attempt} failed"));
        ssm.stop();

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
