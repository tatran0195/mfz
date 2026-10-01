use std::path::{Path, PathBuf};

#[cfg(windows)]
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS},
    RegKey,
};

use crate::{
    config::EnvironmentConfig,
    error::{AppError, Result},
};

/// Returns the user's Desktop directory path, if available.
pub fn get_desktop_dir() -> Option<PathBuf> {
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        let one_drive_desktop = PathBuf::from(&userprofile).join("OneDrive").join("Desktop");
        if one_drive_desktop.is_dir() {
            return Some(one_drive_desktop);
        }
        let std_desktop = PathBuf::from(&userprofile).join("Desktop");
        if std_desktop.is_dir() {
            return Some(std_desktop);
        }
    }
    None
}

/// Registers the `.mfz` file extension in the Windows Shell for the current user.
///
/// This creates file association keys in `HKCU\Software\Classes` without requiring
/// administrator privileges or triggering UAC prompts.
pub fn register_file_association() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_str().ok_or_else(|| {
        AppError::Shell("Current executable path contains invalid UTF-8 characters".to_string())
    })?;

    // Look for icon.ico next to executable or in current dir, otherwise point to mfz.exe
    let icon_path = exe_path
        .parent()
        .map(|p| p.join("icon.ico"))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| exe_str.to_string());

    let command_str = format!("\"{}\" launch \"%1\"", exe_str);

    #[cfg(windows)]
    {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // .mfz → mfz.session
        let (ext_key, _) = hkcu
            .create_subkey("Software\\Classes\\.mfz")
            .map_err(|e| AppError::Shell(format!("Failed to create .mfz key: {e}")))?;
        ext_key
            .set_value("", &"mfz.session")
            .map_err(|e| AppError::Shell(format!("Failed to set .mfz default value: {e}")))?;

        // mfz.session display name
        let (prog_key, _) = hkcu
            .create_subkey("Software\\Classes\\mfz.session")
            .map_err(|e| AppError::Shell(format!("Failed to create mfz.session key: {e}")))?;
        prog_key
            .set_value("", &"MFZ Remote Desktop Profile")
            .map_err(|e| AppError::Shell(format!("Failed to set mfz.session value: {e}")))?;

        // DefaultIcon
        let (icon_key, _) = hkcu
            .create_subkey("Software\\Classes\\mfz.session\\DefaultIcon")
            .map_err(|e| AppError::Shell(format!("Failed to create DefaultIcon key: {e}")))?;
        icon_key
            .set_value("", &icon_path.as_str())
            .map_err(|e| AppError::Shell(format!("Failed to set DefaultIcon value: {e}")))?;

        // shell\open\command
        let (cmd_key, _) = hkcu
            .create_subkey("Software\\Classes\\mfz.session\\shell\\open\\command")
            .map_err(|e| AppError::Shell(format!("Failed to create open command key: {e}")))?;
        cmd_key
            .set_value("", &command_str.as_str())
            .map_err(|e| AppError::Shell(format!("Failed to set open command value: {e}")))?;
    }

    #[cfg(not(windows))]
    let _ = (icon_path, command_str);

    Ok(())
}

/// Unregisters the `.mfz` file extension association from `HKCU\Software\Classes`.
pub fn unregister_file_association() -> Result<()> {
    #[cfg(windows)]
    {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let classes = hkcu
            .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
            .map_err(|e| AppError::Shell(format!("Failed to open Software\\Classes: {e}")))?;

        // Delete mfz.session subtree first (has children), then .mfz leaf
        for key in ["mfz.session", ".mfz"] {
            // delete_subkey_all is recursive
            match classes.delete_subkey_all(key) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(AppError::Shell(format!(
                        "Failed to delete registry key {key}: {e}"
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Exports an `.mfz` profile file.
///
/// If `standalone` is true and `config` is provided, writes all connection parameters.
/// Otherwise writes a pointer file `env = <env_name>` referencing the local `mfz.ini`.
pub fn export_mfz_file(
    env_name: &str,
    config: Option<&EnvironmentConfig>,
    standalone: bool,
    dest: &Path,
) -> Result<PathBuf> {
    let target_path = if dest.is_dir() || dest.extension().is_none() {
        if !dest.exists() {
            std::fs::create_dir_all(dest)?;
        }
        dest.join(format!("{env_name}.mfz"))
    } else {
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        dest.to_path_buf()
    };

    let content = if standalone && config.is_some() {
        let cfg = config.unwrap();
        let map = cfg.to_map();
        let mut lines = Vec::new();
        lines.push(format!("# MFZ Standalone Remote Profile: {env_name}"));
        lines.push(format!(
            "# Generated: {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        lines.push(format!("[{env_name}]"));

        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(v) = map.get(k) {
                lines.push(format!("{k} = {v}"));
            }
        }
        lines.join("\r\n") + "\r\n"
    } else {
        format!(
            "# MFZ Quick-Launch Profile Pointer\r\n\
             # Double-click to connect or run: mfz launch <file>\r\n\
             env = {env_name}\r\n"
        )
    };

    std::fs::write(&target_path, content)?;
    Ok(target_path)
}
