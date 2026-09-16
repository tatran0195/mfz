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

pub fn resolve_drives_to_redirect(
    shared_drives: Option<&str>,
    folder_letter: Option<char>,
) -> Option<String> {
    let base = shared_drives.map(|s| s.trim()).filter(|s| !s.is_empty());
    let folder_drive = folder_letter.map(|c| format!("{}:", c.to_ascii_uppercase()));

    match (base, folder_drive) {
        (None, None) => None,
        (Some(b), None) => {
            if b.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(b.to_string())
            }
        }
        (None, Some(f)) => Some(f),
        (Some(b), Some(f)) => {
            if b.eq_ignore_ascii_case("none") {
                Some(f)
            } else if b == "*" {
                // All drives already covers all drives including mapped virtual drive
                Some("*".to_string())
            } else {
                let mut parts: Vec<String> = b
                    .split(';')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
                if !parts.iter().any(|p| p.eq_ignore_ascii_case(&f)) {
                    parts.push(f);
                }
                Some(parts.join(";"))
            }
        }
    }
}

pub fn find_available_drive_letter() -> Result<char> {
    for c in ('E'..='Z').rev() {
        let path_str = format!("{}:\\", c);
        if !std::path::Path::new(&path_str).exists() {
            return Ok(c);
        }
    }
    Err(AppError::Rdp(
        "No available drive letter found (E-Z are all occupied)".into(),
    ))
}

pub struct FolderDriveGuard {
    drive_letter: char,
    unmounted: bool,
}

impl FolderDriveGuard {
    pub fn mount(folder: &std::path::Path) -> Result<Self> {
        if !folder.exists() {
            return Err(AppError::Rdp(format!(
                "Shared folder does not exist: {}",
                folder.display()
            )));
        }

        let drive_letter = find_available_drive_letter()?;
        let folder_str = folder
            .to_str()
            .ok_or_else(|| AppError::Rdp("Invalid UTF-8 path for shared folder".into()))?;

        info!(
            "Mapping virtual drive {}: → {}",
            drive_letter,
            folder.display()
        );

        let status = Command::new("subst")
            .args([&format!("{drive_letter}:"), folder_str])
            .status()
            .map_err(|e| AppError::Rdp(format!("Failed to execute `subst`: {e}")))?;

        if !status.success() {
            return Err(AppError::Rdp(format!(
                "Failed to map virtual drive {}: for folder {}",
                drive_letter,
                folder.display()
            )));
        }

        Ok(Self {
            drive_letter,
            unmounted: false,
        })
    }

    pub fn drive_letter(&self) -> char {
        self.drive_letter
    }

    pub fn unmount(&mut self) {
        if !self.unmounted {
            info!("Unmounting virtual drive {}:", self.drive_letter);
            let _ = Command::new("subst")
                .args([&format!("{}:", self.drive_letter), "/d"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            self.unmounted = true;
        }
    }
}

impl Drop for FolderDriveGuard {
    fn drop(&mut self) {
        self.unmount();
    }
}

#[derive(Debug, Clone)]
pub struct LaunchOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub fullscreen: bool,
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    pub shared_drives: Option<&'a str>,
    pub shared_folder: Option<&'a std::path::Path>,
    pub optimize_connection: bool,
}

pub fn launch(opts: LaunchOptions) -> Result<()> {
    let host = opts.host;
    let port = opts.port;
    let fullscreen = opts.fullscreen;
    let username = opts.username;
    let password = opts.password;

    let mut folder_guard = if let Some(folder) = opts.shared_folder {
        Some(FolderDriveGuard::mount(folder)?)
    } else {
        None
    };

    let folder_letter = folder_guard.as_ref().map(|g| g.drive_letter());
    let drives_to_redirect = resolve_drives_to_redirect(opts.shared_drives, folder_letter);

    let rdp_content = build_rdp_file_content(&RdpProfileOptions {
        host,
        port,
        fullscreen,
        username,
        drives_to_redirect: drives_to_redirect.as_deref(),
        optimize_connection: opts.optimize_connection,
    });

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_rdp_path = std::env::temp_dir().join(format!("mfz-{host}-{timestamp}.rdp"));

    std::fs::write(&temp_rdp_path, &rdp_content)
        .map_err(|e| AppError::Rdp(format!("Failed to write temporary RDP profile: {e}")))?;

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
    cmd.arg(&temp_rdp_path);
    if fullscreen {
        cmd.arg("/f");
    }

    info!(
        "Launching RDP → {}:{} (drives: {:?}, optimize: {})",
        host, port, drives_to_redirect, opts.optimize_connection
    );
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

    // Cleanup temporary RDP profile
    let _ = std::fs::remove_file(&temp_rdp_path);

    // Explicitly unmount virtual drive if any
    if let Some(mut guard) = folder_guard.take() {
        guard.unmount();
    }

    status.map_err(|e| AppError::Rdp(format!("Failed to launch mstsc: {e}")))?;
    info!("RDP session ended");
    Ok(())
}
