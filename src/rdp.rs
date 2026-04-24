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

    // If both user and pass are provided, cache them using cmdkey
    if let (Some(u), Some(p)) = (username, password) {
        info!("Caching credentials for TERMSRV/{}", host);
        Command::new("cmdkey")
            .args([
                &format!("/generic:TERMSRV/{host}"),
                &format!("/user:{u}"),
                &format!("/pass:{p}"),
            ])
            .status()
            .map_err(|e| AppError::Rdp(format!("Failed to cache credentials: {e}")))?;
    }

    let mut cmd = Command::new("mstsc");
    cmd.arg(format!("/v:{target}"));
    if fullscreen {
        cmd.arg("/f");
    }

    // Not strictly necessary if cmdkey worked, but helps mstsc fill the field
    // Note: mstsc doesn't have a /p (password) flag for security reasons.
    // It relies on Credential Manager (populated by cmdkey).

    info!("Launching RDP → {}", target);
    let status = cmd.status();

    // Cleanup credentials after RDP session ends
    if username.is_some() && password.is_some() {
        info!("Cleaning up cached credentials for TERMSRV/{}", host);
        let _ = Command::new("cmdkey")
            .arg(format!("/delete:TERMSRV/{host}"))
            .status();
    }

    status.map_err(|e| AppError::Rdp(format!("Failed to launch mstsc: {e}")))?;
    info!("RDP session ended");
    Ok(())
}
