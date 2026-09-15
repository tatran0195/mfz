use std::process::Command;

use tracing::info;

use crate::error::{AppError, Result};

pub fn launch(
    host: &str,
    port: u16,
    fullscreen: bool,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<()> {
    let target = format!("{host}:{port}");

    let generic_targets = [
        format!("TERMSRV/{host}:{port}"),
        format!("TERMSRV/{host}"),
        format!("TERMSRV/127.0.0.1:{port}"),
    ];

    // If both user and pass are provided, cache them using cmdkey
    if let (Some(u), Some(p)) = (username, password) {
        for target_name in &generic_targets {
            info!("Caching credentials for {target_name}");
            let _ = Command::new("cmdkey")
                .args([
                    &format!("/generic:{target_name}"),
                    &format!("/user:{u}"),
                    &format!("/pass:{p}"),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    let mut cmd = Command::new("mstsc");
    cmd.arg(format!("/v:{target}"));
    if fullscreen {
        cmd.arg("/f");
    }

    info!("Launching RDP → {}", target);
    let status = cmd.status();

    // Cleanup credentials after RDP session ends
    if username.is_some() && password.is_some() {
        for target_name in &generic_targets {
            info!("Cleaning up cached credentials for {target_name}");
            let _ = Command::new("cmdkey")
                .arg(format!("/delete:{target_name}"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    status.map_err(|e| AppError::Rdp(format!("Failed to launch mstsc: {e}")))?;
    info!("RDP session ended");
    Ok(())
}
