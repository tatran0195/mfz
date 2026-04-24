use crate::error::{AppError, Result};
use std::process::Command;
use tracing::info;

pub fn launch(host: &str, port: u16, fullscreen: bool) -> Result<()> {
    let target = format!("{host}:{port}");
    let mut cmd = Command::new("mstsc");
    cmd.arg(format!("/v:{target}"));
    if fullscreen { cmd.arg("/f"); }

    info!("Launching RDP → {}", target);
    cmd.status()
        .map_err(|e| AppError::Rdp(format!("Failed to launch mstsc: {e}")))?;
    info!("RDP session ended");
    Ok(())
}