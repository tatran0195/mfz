use std::process::Command;

use tracing::info;

use crate::error::{AppError, Result};

#[derive(Debug, Clone)]
pub struct RdpProfileOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub fullscreen: bool,
    pub username: Option<&'a str>,
    pub drives_to_redirect: Option<&'a str>,
    pub optimize_connection: bool,
}

pub fn build_rdp_file_content(opts: &RdpProfileOptions) -> String {
    let mut lines = Vec::new();

    lines.push(format!("full address:s:{}:{}", opts.host, opts.port));
    lines.push(format!(
        "screen mode id:i:{}",
        if opts.fullscreen { 2 } else { 1 }
    ));
    lines.push("use multimon:i:0".into());
    lines.push("smart sizing:i:1".into());

    let drives = opts.drives_to_redirect.unwrap_or("");
    lines.push(format!("drivestoredirect:s:{drives}"));
    lines.push("redirectclipboard:i:1".into());

    if let Some(user) = opts.username {
        lines.push(format!("username:s:{user}"));
    }
    lines.push("prompt for credentials:i:0".into());

    if opts.optimize_connection {
        lines.push("compression:i:1".into());
        lines.push("bitmapcachepersistenable:i:1".into());
        lines.push("connection type:i:6".into());
        lines.push("networkautodetect:i:0".into());
        lines.push("bandwidthautodetect:i:1".into());
        lines.push("audiomode:i:2".into());
        lines.push("redirectprinters:i:0".into());
        lines.push("redirectsmartcards:i:0".into());
        lines.push("disable wallpaper:i:1".into());
        lines.push("disable full window drag:i:1".into());
        lines.push("disable menu anims:i:1".into());
        lines.push("disable themes:i:0".into());
        lines.push("allow font smoothing:i:1".into());
    }

    lines.join("\r\n") + "\r\n"
}

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
