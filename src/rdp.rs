use std::process::Command;

use tracing::info;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptimizationProfile {
    #[default]
    None,
    Speed,
    Balanced,
    Quality,
}

impl OptimizationProfile {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "speed" | "fast" | "lan" => Self::Speed,
            "balanced" | "balance" => Self::Balanced,
            "quality" | "high" | "best" => Self::Quality,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptimizationSettings {
    pub profile: OptimizationProfile,
    pub audio: Option<bool>,
    pub wallpaper: Option<bool>,
    pub font_smoothing: Option<bool>,
}

impl OptimizationSettings {
    pub fn from_profile_name(name: Option<&str>) -> Self {
        let profile = name.map(OptimizationProfile::parse).unwrap_or_default();
        Self {
            profile,
            audio: None,
            wallpaper: None,
            font_smoothing: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RdpProfileOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub fullscreen: bool,
    pub username: Option<&'a str>,
    pub drives_to_redirect: Option<&'a str>,
    pub optimization: OptimizationSettings,
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
    lines.push("dynamic resolution:i:1".into());

    let drives = opts.drives_to_redirect.unwrap_or("");
    lines.push(format!("drivestoredirect:s:{drives}"));
    lines.push("redirectclipboard:i:1".into());

    if let Some(user) = opts.username {
        lines.push(format!("username:s:{user}"));
    }
    lines.push("prompt for credentials:i:0".into());

    match opts.optimization.profile {
        OptimizationProfile::None => {
            if let Some(audio) = opts.optimization.audio {
                lines.push(format!("audiomode:i:{}", if audio { 0 } else { 2 }));
            }
            if let Some(wallpaper) = opts.optimization.wallpaper {
                lines.push(format!("disable wallpaper:i:{}", if wallpaper { 0 } else { 1 }));
            }
            if let Some(fonts) = opts.optimization.font_smoothing {
                lines.push(format!("allow font smoothing:i:{}", if fonts { 1 } else { 0 }));
            }
        }
        OptimizationProfile::Speed => {
            lines.push("compression:i:1".into());
            lines.push("bitmapcachepersistenable:i:1".into());
            lines.push("connection type:i:6".into());
            lines.push("networkautodetect:i:0".into());
            lines.push("bandwidthautodetect:i:1".into());

            let audiomode = if opts.optimization.audio == Some(true) { 0 } else { 2 };
            lines.push(format!("audiomode:i:{audiomode}"));

            lines.push("redirectprinters:i:0".into());
            lines.push("redirectsmartcards:i:0".into());

            let disable_wallpaper = if opts.optimization.wallpaper == Some(true) { 0 } else { 1 };
            lines.push(format!("disable wallpaper:i:{disable_wallpaper}"));

            lines.push("disable full window drag:i:1".into());
            lines.push("disable menu anims:i:1".into());
            lines.push("disable themes:i:0".into());

            let font_smoothing = if opts.optimization.font_smoothing == Some(false) { 0 } else { 1 };
            lines.push(format!("allow font smoothing:i:{font_smoothing}"));
        }
        OptimizationProfile::Balanced => {
            lines.push("compression:i:1".into());
            lines.push("bitmapcachepersistenable:i:1".into());
            lines.push("connection type:i:6".into());
            lines.push("networkautodetect:i:1".into());
            lines.push("bandwidthautodetect:i:1".into());

            let audiomode = if opts.optimization.audio == Some(false) { 2 } else { 0 };
            lines.push(format!("audiomode:i:{audiomode}"));

            lines.push("redirectprinters:i:0".into());
            lines.push("redirectsmartcards:i:0".into());

            let disable_wallpaper = if opts.optimization.wallpaper == Some(false) { 1 } else { 0 };
            lines.push(format!("disable wallpaper:i:{disable_wallpaper}"));

            lines.push("disable full window drag:i:0".into());
            lines.push("disable menu anims:i:1".into());
            lines.push("disable themes:i:0".into());

            let font_smoothing = if opts.optimization.font_smoothing == Some(false) { 0 } else { 1 };
            lines.push(format!("allow font smoothing:i:{font_smoothing}"));
        }
        OptimizationProfile::Quality => {
            lines.push("compression:i:1".into());
            lines.push("bitmapcachepersistenable:i:1".into());
            lines.push("connection type:i:6".into());

            let audiomode = if opts.optimization.audio == Some(false) { 2 } else { 0 };
            lines.push(format!("audiomode:i:{audiomode}"));

            lines.push("disable wallpaper:i:0".into());
            lines.push("disable full window drag:i:0".into());
            lines.push("disable menu anims:i:0".into());
            lines.push("disable themes:i:0".into());

            let font_smoothing = if opts.optimization.font_smoothing == Some(false) { 0 } else { 1 };
            lines.push(format!("allow font smoothing:i:{font_smoothing}"));
        }
    }

    lines.join("\r\n") + "\r\n"
}

#[cfg(windows)]
fn get_logical_drives_mask() -> u32 {
    extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    unsafe { GetLogicalDrives() }
}

#[cfg(not(windows))]
fn get_logical_drives_mask() -> u32 {
    0
}

pub fn is_drive_letter_in_use(c: char) -> bool {
    let upper = c.to_ascii_uppercase();
    if !(b'A'..=b'Z').contains(&(upper as u8)) {
        return true;
    }
    let shift = (upper as u8) - b'A';
    let mask = get_logical_drives_mask();
    if mask != 0 && (mask & (1 << shift)) != 0 {
        return true;
    }
    std::path::Path::new(&format!("{}:\\", upper)).exists()
}

pub fn get_ready_local_drives() -> Vec<String> {
    let mask = get_logical_drives_mask();
    let mut drives = Vec::new();
    for c in b'A'..=b'Z' {
        let shift = c - b'A';
        let drive_char = c as char;
        let is_assigned = if mask != 0 {
            (mask & (1 << shift)) != 0
        } else {
            std::path::Path::new(&format!("{}:\\", drive_char)).exists()
        };

        if is_assigned {
            let path_str = format!("{}:\\", drive_char);
            // Verify path is actually ready/accessible (skips empty optical/CD drives)
            if std::path::Path::new(&path_str).exists() {
                drives.push(format!("{}:", drive_char));
            }
        }
    }
    drives
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
            } else if b == "*" {
                let ready = get_ready_local_drives();
                if ready.is_empty() {
                    Some("*".to_string())
                } else {
                    Some(ready.join(";"))
                }
            } else {
                Some(b.to_string())
            }
        }
        (None, Some(f)) => Some(f),
        (Some(b), Some(f)) => {
            if b.eq_ignore_ascii_case("none") {
                Some(f)
            } else if b == "*" {
                let mut ready = get_ready_local_drives();
                if !ready.iter().any(|p| p.eq_ignore_ascii_case(&f)) {
                    ready.push(f);
                }
                Some(ready.join(";"))
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
        if !is_drive_letter_in_use(c) {
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
    pub optimization: OptimizationSettings,
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
        optimization: opts.optimization.clone(),
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
        "Launching RDP → {}:{} (drives: {:?}, profile: {:?})",
        host, port, drives_to_redirect, opts.optimization.profile
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
