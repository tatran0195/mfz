use crate::{
    error::{AppError, Result},
    mfa::MfaSessionManager,
    ui::Spinner,
};
use std::{
    net::TcpStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::time::sleep;
use tracing::{error, info, warn};

const MAX_RETRIES:           u32 = 3;
const PORT_WAIT_TIMEOUT:     u64 = 30;
const PORT_POLL_INTERVAL_MS: u64 = 500;
const RETRY_BACKOFF_SECS:    u64 = 3;

pub struct SsmSession {
    pub instance:    String,
    pub remote_port: u16,
    pub local_port:  u16,
    region:          Option<String>,
    process:         Option<Child>,
}

impl SsmSession {
    pub fn new(
        instance:    impl Into<String>,
        remote_port: u16,
        local_port:  u16,
        region:      Option<String>,
    ) -> Self {
        Self { instance: instance.into(), remote_port, local_port, region, process: None }
    }

    pub async fn start(&mut self, mfa: &mut MfaSessionManager) -> Result<()> {
        if self.is_running() {
            warn!("SSM session already running — stopping first");
            self.stop();
        }

        info!("Starting SSM: {} → 127.0.0.1:{}", self.instance, self.local_port);

        let creds = mfa.get_credentials().await?;

        let mut cmd = Command::new("aws");
        cmd.args([
            "ssm", "start-session",
            "--target",        &self.instance,
            "--document-name", "AWS-StartPortForwardingSession",
            "--parameters",
            &format!(
                "portNumber={},localPortNumber={}",
                self.remote_port, self.local_port
            ),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        for (k, v) in creds.as_env_pairs() {
            cmd.env(k, v);
        }
        cmd.env_remove("AWS_PROFILE");
        cmd.env_remove("AWS_DEFAULT_PROFILE");
        if let Some(ref r) = self.region {
            cmd.env("AWS_DEFAULT_REGION", r);
        }

        self.process = Some(
            cmd.spawn()
                .map_err(|e| AppError::Ssm(format!("Failed to spawn `aws ssm`: {e}")))?,
        );
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            info!("Terminating SSM session (PID {})", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn wait(&mut self) -> Result<i32> {
        let child = self
            .process
            .as_mut()
            .ok_or_else(|| AppError::Ssm("No active SSM process".into()))?;
        child
            .wait()
            .map(|s| s.code().unwrap_or(-1))
            .map_err(|e| AppError::Ssm(format!("Wait failed: {e}")))
    }

    pub fn is_running(&mut self) -> bool {
        self.process
            .as_mut()
            .map(|c| c.try_wait().ok().flatten().is_none())
            .unwrap_or(false)
    }

    pub async fn wait_for_port(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(PORT_WAIT_TIMEOUT);
        while Instant::now() < deadline {
            if !self.is_running() {
                let rc = self
                    .process
                    .as_mut()
                    .and_then(|c| c.try_wait().ok().flatten())
                    .and_then(|s| s.code())
                    .unwrap_or(-1);
                error!("SSM process exited unexpectedly (rc={})", rc);
                return false;
            }
            if self.port_open() {
                return true;
            }
            sleep(Duration::from_millis(PORT_POLL_INTERVAL_MS)).await;
        }
        false
    }

    fn port_open(&self) -> bool {
        TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", self.local_port).parse().unwrap(),
            Duration::from_secs(1),
        )
        .is_ok()
    }
}

impl Drop for SsmSession {
    fn drop(&mut self) { self.stop(); }
}

pub async fn start_with_retries(
    ssm: &mut SsmSession,
    mfa: &mut MfaSessionManager,
) -> Result<()> {
    for attempt in 1..=MAX_RETRIES {
        let spinner = Spinner::new(format!(
            "SSM attempt {attempt}/{MAX_RETRIES} — forwarding 127.0.0.1:{}…",
            ssm.local_port
        ));

        ssm.start(mfa).await?;

        if ssm.wait_for_port().await {
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
            mfa.get_credentials().await?;
            ws.warning("Retrying…");
        }
    }

    Err(AppError::Ssm(format!("SSM failed after {MAX_RETRIES} attempts")))
}