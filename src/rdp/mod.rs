#[cfg(windows)]
mod platform;

use crate::{
    error::{AppError, Result},
    validation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptimizationProfile {
    #[default]
    None,
    Speed,
    Balanced,
    Quality,
}

impl OptimizationProfile {
    pub fn parse(s: &str) -> Result<Self> {
        validation::optimization(s)?;
        Ok(match s.trim().to_lowercase().as_str() {
            "speed" | "fast" | "lan" => Self::Speed,
            "balanced" | "balance" => Self::Balanced,
            "quality" | "high" | "best" => Self::Quality,
            _ => Self::None,
        })
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
    pub fn from_profile_name(name: Option<&str>) -> Result<Self> {
        let profile = name
            .map(OptimizationProfile::parse)
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            profile,
            audio: None,
            wallpaper: None,
            font_smoothing: None,
        })
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

pub fn build_rdp_file_content(opts: &RdpProfileOptions) -> Result<String> {
    validation::hostname(opts.host)?;
    if opts.port == 0 {
        return Err(AppError::Rdp("RDP port must be nonzero".into()));
    }
    if let Some(user) = opts.username {
        validation::single_line(user, "RDP username")?;
    }
    let normalized_drives = opts
        .drives_to_redirect
        .map(validation::drives)
        .transpose()?
        .flatten();
    let mut lines = Vec::new();

    lines.push(format!("full address:s:{}:{}", opts.host, opts.port));
    lines.push(format!(
        "screen mode id:i:{}",
        if opts.fullscreen { 2 } else { 1 }
    ));
    lines.push("use multimon:i:0".into());
    lines.push("smart sizing:i:1".into());
    lines.push("dynamic resolution:i:1".into());

    let drives = normalized_drives.as_deref().unwrap_or("");
    lines.push(format!("drivestoredirect:s:{drives}"));
    lines.push("redirectclipboard:i:1".into());

    if let Some(user) = opts.username {
        lines.push(format!("username:s:{user}"));
    }
    lines.push("prompt for credentials:i:0".into());

    // SSM forwards TCP only. These performance preferences are NOT a Windows
    // UDP policy switch; mstsc owns UDP probing/TCP fallback. Do not claim that
    // a connection-type hint disables UDP or edit machine-wide policy silently.
    lines.push("networkautodetect:i:0".into());
    lines.push("bandwidthautodetect:i:0".into());
    lines.push(format!(
        "connection type:i:{}",
        if opts.optimization.profile == OptimizationProfile::None {
            2
        } else {
            6
        }
    ));
    lines.push("enablecredsspsupport:i:1".into()); // keep NLA (cached creds are used)
    lines.push("authentication level:i:2".into()); // warn on unverified RDP server identity; retain NLA

    match opts.optimization.profile {
        OptimizationProfile::None => {
            if let Some(audio) = opts.optimization.audio {
                lines.push(format!("audiomode:i:{}", if audio { 0 } else { 2 }));
            }
            if let Some(wallpaper) = opts.optimization.wallpaper {
                lines.push(format!(
                    "disable wallpaper:i:{}",
                    if wallpaper { 0 } else { 1 }
                ));
            }
            if let Some(fonts) = opts.optimization.font_smoothing {
                lines.push(format!(
                    "allow font smoothing:i:{}",
                    if fonts { 1 } else { 0 }
                ));
            }
        }
        OptimizationProfile::Speed => {
            lines.push("compression:i:1".into());
            lines.push("bitmapcachepersistenable:i:1".into());

            let audiomode = if opts.optimization.audio == Some(true) {
                0
            } else {
                2
            };
            lines.push(format!("audiomode:i:{audiomode}"));

            lines.push("redirectprinters:i:0".into());
            lines.push("redirectsmartcards:i:0".into());

            let disable_wallpaper = if opts.optimization.wallpaper == Some(true) {
                0
            } else {
                1
            };
            lines.push(format!("disable wallpaper:i:{disable_wallpaper}"));

            lines.push("disable full window drag:i:1".into());
            lines.push("disable menu anims:i:1".into());
            lines.push("disable themes:i:0".into());

            let font_smoothing = if opts.optimization.font_smoothing == Some(false) {
                0
            } else {
                1
            };
            lines.push(format!("allow font smoothing:i:{font_smoothing}"));
        }
        OptimizationProfile::Balanced => {
            lines.push("compression:i:1".into());
            lines.push("bitmapcachepersistenable:i:1".into());

            let audiomode = if opts.optimization.audio == Some(false) {
                2
            } else {
                0
            };
            lines.push(format!("audiomode:i:{audiomode}"));

            lines.push("redirectprinters:i:0".into());
            lines.push("redirectsmartcards:i:0".into());

            let disable_wallpaper = if opts.optimization.wallpaper == Some(false) {
                1
            } else {
                0
            };
            lines.push(format!("disable wallpaper:i:{disable_wallpaper}"));

            lines.push("disable full window drag:i:0".into());
            lines.push("disable menu anims:i:1".into());
            lines.push("disable themes:i:0".into());

            let font_smoothing = if opts.optimization.font_smoothing == Some(false) {
                0
            } else {
                1
            };
            lines.push(format!("allow font smoothing:i:{font_smoothing}"));
        }
        OptimizationProfile::Quality => {
            lines.push("compression:i:1".into());
            lines.push("bitmapcachepersistenable:i:1".into());

            let audiomode = if opts.optimization.audio == Some(false) {
                2
            } else {
                0
            };
            lines.push(format!("audiomode:i:{audiomode}"));

            lines.push(format!(
                "disable wallpaper:i:{}",
                if opts.optimization.wallpaper == Some(false) {
                    1
                } else {
                    0
                }
            ));
            lines.push("disable full window drag:i:0".into());
            lines.push("disable menu anims:i:0".into());
            lines.push("disable themes:i:0".into());

            let font_smoothing = if opts.optimization.font_smoothing == Some(false) {
                0
            } else {
                1
            };
            lines.push(format!("allow font smoothing:i:{font_smoothing}"));
        }
    }

    Ok(lines.join("\r\n") + "\r\n")
}

fn get_logical_drives_mask() -> Result<u32> {
    #[cfg(windows)]
    {
        platform::logical_drives_mask()
    }
    #[cfg(not(windows))]
    {
        Ok(0)
    }
}

/// Local, accessible drive roots only. In particular do not probe mapped network
/// drives while expanding '*'. Explicit drive selections remain available.
pub fn get_ready_local_drives() -> Result<Vec<String>> {
    let mask = get_logical_drives_mask()?;
    let mut drives = Vec::new();
    for letter in 'A'..='Z' {
        if mask & (1 << (letter as u8 - b'A')) == 0 {
            continue;
        }
        #[cfg(windows)]
        if !platform::is_local_drive(letter) {
            continue;
        }
        let root = format!("{letter}:\\");
        if std::path::Path::new(&root).is_dir() {
            drives.push(format!("{letter}:"));
        }
    }
    Ok(drives)
}

pub fn is_drive_letter_in_use(letter: char) -> bool {
    if !letter.is_ascii_alphabetic() {
        return true;
    }
    let letter = letter.to_ascii_uppercase();
    get_logical_drives_mask().map_or(true, |mask| mask & (1 << (letter as u8 - b'A')) != 0)
}

pub fn resolve_drives_to_redirect(
    shared_drives: Option<&str>,
    folder_letter: Option<char>,
) -> Result<Option<String>> {
    resolve_drives_with(shared_drives, folder_letter, get_ready_local_drives)
}

fn resolve_drives_with(
    shared_drives: Option<&str>,
    folder_letter: Option<char>,
    ready: impl FnOnce() -> Result<Vec<String>>,
) -> Result<Option<String>> {
    let base = shared_drives.map(validation::drives).transpose()?.flatten();
    let mut drives = match base.as_deref() {
        Some("*") => ready()?,
        Some(drives) => drives.split(';').map(str::to_owned).collect(),
        None => Vec::new(),
    };
    if let Some(letter) = folder_letter {
        if !letter.is_ascii_alphabetic() {
            return Err(AppError::Rdp(
                "Virtual drive letter must be ASCII A–Z".into(),
            ));
        }
        let drive = format!("{}:", letter.to_ascii_uppercase());
        if !drives.contains(&drive) {
            drives.push(drive);
        }
    }
    Ok((!drives.is_empty()).then(|| drives.join(";")))
}

pub fn find_available_drive_letter() -> Result<char> {
    let mask = get_logical_drives_mask()?;
    ('A'..='Z')
        .rev()
        .find(|letter| mask & (1 << (*letter as u8 - b'A')) == 0)
        .ok_or_else(|| AppError::Rdp("No available drive letter for shared-folder mapping".into()))
}

#[derive(Clone)]
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

impl LaunchOptions<'_> {
    /// Preflight before creating credentials, DOS mappings or a child process.
    pub fn validate(&self) -> Result<()> {
        if self.host != "127.0.0.1" || self.port == 0 {
            return Err(AppError::Rdp("Native RDP launch requires its numeric 127.0.0.1 tunnel endpoint and a nonzero port".into()));
        }
        self.validate_credentials()?;
        if let Some(drives) = self.shared_drives {
            validation::drives(drives)?;
        }
        if let Some(folder) = self.shared_folder {
            if !std::fs::metadata(folder)?.is_dir() {
                return Err(AppError::Rdp("Shared folder is not a directory".into()));
            }
        }
        Ok(())
    }
    /// Validate a resolved vault password without repeating filesystem probes.
    pub fn validate_credentials(&self) -> Result<()> {
        if let Some(username) = self.username {
            validation::single_line(username, "RDP username")?;
            if username.trim().is_empty() {
                return Err(AppError::Rdp("RDP username must not be empty".into()));
            }
        }
        if let Some(password) = self.password {
            if self.username.is_none() || password.contains('\0') {
                return Err(AppError::Rdp(
                    "An RDP password requires a username and cannot contain NUL".into(),
                ));
            }
        }
        #[cfg(windows)]
        if self.password.is_some_and(|password| {
            password.encode_utf16().count() * 2
                > windows_sys::Win32::Security::Credentials::CRED_MAX_CREDENTIAL_BLOB_SIZE as usize
        }) {
            return Err(AppError::Rdp(
                "RDP password exceeds the Windows credential blob limit".into(),
            ));
        }
        Ok(())
    }
}

pub async fn launch(opts: LaunchOptions<'_>) -> Result<()> {
    opts.validate()?;
    #[cfg(windows)]
    {
        platform::launch(opts).await
    }
    #[cfg(not(windows))]
    {
        Err(AppError::Rdp("mstsc/RDP desktop launch is supported only on Windows; use --ssm-only for the native tunnel".into()))
    }
}

#[cfg(test)]
mod remediation_tests {
    use super::*;

    #[test]
    fn wildcard_expands_only_ready_drives_and_never_falls_back_to_unavailable_devices() {
        assert_eq!(
            resolve_drives_with(Some("*"), None, || Ok(vec!["C:".into(), "D:".into()]))
                .unwrap()
                .as_deref(),
            Some("C:;D:")
        );
        assert_eq!(
            resolve_drives_with(Some("*"), None, || Ok(vec![])).unwrap(),
            None
        );
        assert_eq!(
            resolve_drives_with(Some("*"), Some('z'), || Ok(vec![]))
                .unwrap()
                .as_deref(),
            Some("Z:")
        );
        assert!(resolve_drives_with(Some("*"), None, || Err(AppError::Rdp(
            "Enumeration failed".into()
        )))
        .is_err());
        assert_eq!(
            resolve_drives_with(Some("c:,d:"), None, || panic!(
                "Explicit drives must not enumerate devices"
            ))
            .unwrap()
            .as_deref(),
            Some("C:;D:")
        );
    }

    #[test]
    fn profile_settings_have_unique_keys_and_honor_wallpaper_and_audio_overrides() {
        for profile in [
            OptimizationProfile::None,
            OptimizationProfile::Speed,
            OptimizationProfile::Balanced,
            OptimizationProfile::Quality,
        ] {
            let options = RdpProfileOptions {
                host: "127.0.0.1",
                port: 13389,
                fullscreen: false,
                username: Some("Administrator"),
                drives_to_redirect: Some("c:, D:"),
                optimization: OptimizationSettings {
                    profile,
                    wallpaper: Some(false),
                    audio: Some(false),
                    font_smoothing: Some(false),
                },
            };
            let content = build_rdp_file_content(&options).unwrap();
            let mut keys = std::collections::BTreeSet::new();
            for line in content.lines() {
                assert!(
                    keys.insert(line.split(':').next().unwrap()),
                    "duplicate RDP setting: {line}"
                );
            }
            for expected in [
                "disable wallpaper:i:1",
                "audiomode:i:2",
                "allow font smoothing:i:0",
                "authentication level:i:2",
                "drivestoredirect:s:C:;D:",
            ] {
                assert!(content.contains(expected), "missing {expected}");
            }
        }
        assert!(OptimizationSettings::from_profile_name(Some("silently-ignored-typo")).is_err());
    }

    #[test]
    fn profile_injection_and_non_tunnel_launch_destinations_are_rejected_before_side_effects() {
        let mut profile = RdpProfileOptions {
            host: "127.0.0.1",
            port: 13389,
            fullscreen: true,
            username: Some("user\r\nredirectprinters:i:1"),
            drives_to_redirect: None,
            optimization: OptimizationSettings::default(),
        };
        assert!(build_rdp_file_content(&profile).is_err());
        profile.username = None;
        profile.drives_to_redirect = Some("C:\r\nredirectprinters:i:1");
        assert!(build_rdp_file_content(&profile).is_err());
        let options = LaunchOptions {
            host: "remote.example.org",
            port: 13389,
            fullscreen: true,
            username: None,
            password: None,
            shared_drives: None,
            shared_folder: None,
            optimization: OptimizationSettings::default(),
        };
        assert!(options.validate().is_err());
        let options = LaunchOptions {
            host: "127.0.0.1",
            username: None,
            password: Some("secret"),
            ..options
        };
        assert!(options.validate().is_err());
    }
}
