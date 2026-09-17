use std::{
    path::{Path, PathBuf},
    process::Command,
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

    let entries = [
        ("HKCU\\Software\\Classes\\.mfz", "", "mfz.session"),
        (
            "HKCU\\Software\\Classes\\mfz.session",
            "",
            "MFZ Remote Desktop Profile",
        ),
        (
            "HKCU\\Software\\Classes\\mfz.session\\DefaultIcon",
            "",
            &icon_path,
        ),
        (
            "HKCU\\Software\\Classes\\mfz.session\\shell\\open\\command",
            "",
            &command_str,
        ),
    ];

    for (key, val_name, val_data) in entries {
        let mut cmd = Command::new("reg");
        cmd.arg("add").arg(key);
        if val_name.is_empty() {
            cmd.arg("/ve");
        } else {
            cmd.arg("/v").arg(val_name);
        }
        cmd.arg("/d").arg(val_data).arg("/f");

        let output = cmd
            .output()
            .map_err(|e| AppError::Shell(format!("Failed to execute reg.exe: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Shell(format!(
                "Registry add failed for {key}: {}",
                stderr.trim()
            )));
        }
    }

    Ok(())
}

/// Unregisters the `.mfz` file extension association from `HKCU\Software\Classes`.
pub fn unregister_file_association() -> Result<()> {
    let keys = [
        "HKCU\\Software\\Classes\\.mfz",
        "HKCU\\Software\\Classes\\mfz.session",
    ];

    for key in keys {
        let mut cmd = Command::new("reg");
        cmd.arg("delete").arg(key).arg("/f");

        // We run reg delete and don't fail if the key doesn't exist
        let _ = cmd.output();
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
