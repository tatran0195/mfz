use std::path::{Path, PathBuf};

#[cfg(windows)]
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS},
    RegKey,
};

use crate::{
    config::{encode_ini_value, EnvironmentConfig},
    error::{AppError, Result},
    sys::storage,
    validation,
};

/// Resolve the actual Windows Known Folder, including redirected/localized
/// Desktops. Do not guess OneDrive/Desktop paths from ambient environment vars.
pub fn get_desktop_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        super::windows::desktop_directory()
    }
    #[cfg(not(windows))]
    {
        Err(AppError::Shell("Desktop export requires Windows Known Folders; select --out explicitly on this platform".into()))
    }
}

#[cfg(any(windows, test))]
fn association_command(executable: &str) -> Result<String> {
    validation::windows_command_line(&[executable, "launch", "--", "%1"])
}

#[cfg(any(windows, test))]
fn owns_association_command(command: &str, executable: &str) -> Result<bool> {
    let current = association_command(executable)?;
    let previous = format!("\"{executable}\" launch -- \"%1\"");
    let legacy = format!("\"{executable}\" launch \"%1\"");
    Ok([current, previous, legacy]
        .iter()
        .any(|expected| expected.eq_ignore_ascii_case(command)))
}

#[cfg(windows)]
fn optional_value(key: &RegKey, name: &str) -> Result<Option<String>> {
    match key.get_value(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn notify_association_change() {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

/// Per-user registration. Install a complete ProgID BEFORE changing the extension
/// default; remember a prior default rather than destroying another app's data.
#[cfg(windows)]
pub fn register_file_association() -> Result<()> {
    let executable = std::env::current_exe()?;
    let executable = executable
        .to_str()
        .ok_or_else(|| AppError::Shell("Executable path must be valid UTF-8".into()))?;
    let command = association_command(executable)?;
    let classes = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey("Software\\Classes")?
        .0;
    let previous = match classes.open_subkey(".mfz") {
        Ok(key) => optional_value(&key, "")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let prog = classes.create_subkey("mfz.session")?.0;
    if !previous
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("mfz.session"))
    {
        if let Some(previous) = previous {
            prog.set_value("PreviousExtensionDefault", &previous)?;
        } else {
            match prog.delete_value("PreviousExtensionDefault") {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    prog.set_value("", &"MFZ Remote Desktop Profile")?;
    let icon = Path::new(executable)
        .parent()
        .map(|parent| parent.join("icon.ico"))
        .filter(|icon| icon.is_file())
        .map(|icon| icon.to_string_lossy().into_owned())
        .unwrap_or_else(|| executable.to_owned());
    prog.create_subkey("DefaultIcon")?
        .0
        .set_value("", &format!("\"{icon}\",0"))?;
    prog.create_subkey("shell\\open\\command")?
        .0
        .set_value("", &command)?;
    // Existing .mfz values/subkeys (OpenWith, another app's metadata) are retained.
    classes
        .create_subkey(".mfz")?
        .0
        .set_value("", &"mfz.session")?;
    notify_association_change();
    Ok(())
}

/// Remove only this executable's owned ProgID/default, not another installation
/// or an unrelated .mfz registry subtree. Normal uninstall is idempotent.
#[cfg(windows)]
pub fn unregister_file_association() -> Result<()> {
    let executable = std::env::current_exe()?;
    let executable = executable
        .to_str()
        .ok_or_else(|| AppError::Shell("Executable path must be valid UTF-8".into()))?;
    let classes = match RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags("Software\\Classes", KEY_ALL_ACCESS)
    {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let prog = match classes.open_subkey("mfz.session") {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Repair an orphaned MFZ default from an older partial install,
            // without deleting the extension key or any other application's data.
            match classes.open_subkey_with_flags(".mfz", KEY_ALL_ACCESS) {
                Ok(extension) => {
                    if optional_value(&extension, "")?
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case("mfz.session"))
                    {
                        extension.delete_value("")?;
                        notify_association_change();
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let command = match prog.open_subkey("shell\\open\\command") {
        Ok(key) => optional_value(&key, "")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if !command
        .as_deref()
        .map(|command| owns_association_command(command, executable))
        .transpose()?
        .unwrap_or(false)
    {
        return Err(AppError::Shell("MFZ ProgID is missing an owned command or belongs to another executable; no association was removed".into()));
    }
    let previous = optional_value(&prog, "PreviousExtensionDefault")?
        .filter(|value| !value.eq_ignore_ascii_case("mfz.session"));
    match classes.open_subkey_with_flags(".mfz", KEY_ALL_ACCESS) {
        Ok(extension) => {
            if optional_value(&extension, "")?
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("mfz.session"))
            {
                if let Some(previous) = previous {
                    extension.set_value("", &previous)?;
                } else {
                    extension.delete_value("")?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    drop(prog);
    classes.delete_subkey_all("mfz.session").map_err(|error| {
        AppError::Shell(format!(
            "Extension default was restored/preserved, but MFZ ProgID cleanup failed: {error}"
        ))
    })?;
    notify_association_change();
    Ok(())
}

#[cfg(not(windows))]
pub fn register_file_association() -> Result<()> {
    Err(AppError::Shell("File associations require Windows".into()))
}

#[cfg(not(windows))]
pub fn unregister_file_association() -> Result<()> {
    Err(AppError::Shell("File associations require Windows".into()))
}

/// Explicit policy for distributing a profile. Ordinary exports never include
/// secrets and never replace an existing file. Secret exports require a private
/// destination directory/file ACLs. New secret-bearing files are private; existing
/// destination DACLs are preserved rather than silently rewritten.
#[derive(Default)]
pub struct ExportOptions<'a> {
    pub include_secrets: bool,
    pub overwrite: bool,
    pub config_path: Option<&'a Path>,
}

/// Backwards-compatible safe-default entry point; secret inclusion is opt-in via
/// the explicit policy function, not an accidental consequence of standalone mode.
pub fn export_mfz_file(
    env_name: &str,
    config: Option<&EnvironmentConfig>,
    standalone: bool,
    dest: &Path,
) -> Result<PathBuf> {
    export_mfz_file_with_options(
        env_name,
        config,
        standalone,
        dest,
        &ExportOptions::default(),
    )
}

pub fn export_mfz_file_with_options(
    env_name: &str,
    config: Option<&EnvironmentConfig>,
    standalone: bool,
    dest: &Path,
    options: &ExportOptions<'_>,
) -> Result<PathBuf> {
    let name = validation::environment_name(env_name)?;
    if options.include_secrets && !standalone {
        return Err(AppError::Shell(
            "A pointer cannot contain exported secrets".into(),
        ));
    }
    let content = if standalone {
        let config = config.ok_or_else(|| {
            AppError::Shell("Standalone export requires an environment configuration".into())
        })?;
        config.validate()?;
        let mut map = config.to_map();
        if !options.include_secrets {
            for key in ["aws_secret_access_key", "secret", "rdp_password"] {
                map.remove(key);
            }
        }
        let mut content = format!("# MFZ standalone connection configuration\r\n# Secrets omitted unless explicitly requested.\r\n[{name}]\r\n");
        let sorted: std::collections::BTreeMap<_, _> = map.into_iter().collect();
        for (key, value) in sorted {
            content.push_str(&format!("{key} = {}\r\n", encode_ini_value(&value)));
        }
        content
    } else {
        let mut content = format!("# MFZ quick-launch pointer\r\nenv = {name}\r\n");
        if let Some(config) = options.config_path {
            let absolute = storage::absolute_path(config)?;
            let path = absolute.to_str().ok_or_else(|| {
                AppError::Shell("Pointer config paths must be valid UTF-8".into())
            })?;
            validation::single_line(path, "Pointer configuration path")?;
            content.push_str(&format!("config = {path}\r\n"));
        }
        content
    };
    let target = if dest.is_dir() || dest.extension().is_none() {
        std::fs::create_dir_all(dest)?;
        dest.join(format!("{name}.mfz"))
    } else {
        if let Some(parent) = dest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        dest.to_owned()
    };
    storage::atomic_write(
        &target,
        content.as_bytes(),
        options.overwrite,
        options.include_secrets,
    )?;
    Ok(target)
}

#[cfg(test)]
mod remediation_tests {
    use super::*;

    #[test]
    fn association_command_has_an_option_separator_and_uninstall_does_not_claim_another_executable()
    {
        let executable = r"C:\Program Files\MFZ\mfz.exe";
        let command = association_command(executable).unwrap();
        assert!(command.contains("\"--\" \"%1\""));
        assert!(owns_association_command(&command, executable).unwrap());
        assert!(
            owns_association_command(&format!("\"{executable}\" launch \"%1\""), executable)
                .unwrap()
        );
        assert!(
            !owns_association_command(r#""C:\Other\mfz.exe" launch -- "%1""#, executable).unwrap()
        );
        assert!(!owns_association_command("unrelated-program %1", executable).unwrap());
    }

    #[test]
    fn export_defaults_are_no_clobber_and_explicit_secret_inclusion_is_reversible() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mfz.ini");
        std::fs::write(&path, "[test]\nprofile=selected\ninstance=i-test\nremote_port=3389\nlocal_port=13389\nrdp_host=test-rdp\nserial=arn:aws:iam::123456789012:mfa/test\nsecret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\nrdp_password=punctuation#; password\n").unwrap();
        let loader = crate::config::ConfigLoader::new(&path);
        let environment = loader.load("test").unwrap();
        let destination = directory.path().join("test.mfz");
        export_mfz_file("test", Some(&environment), true, &destination).unwrap();
        let safe = std::fs::read(&destination).unwrap();
        assert!(export_mfz_file("test", Some(&environment), true, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), safe);
        export_mfz_file_with_options(
            "test",
            Some(&environment),
            true,
            &destination,
            &ExportOptions {
                include_secrets: true,
                overwrite: true,
                config_path: None,
            },
        )
        .unwrap();
        let loaded = loader
            .load_for_launch(destination.to_str().unwrap())
            .unwrap();
        assert_eq!(loaded.config.secret, environment.secret);
        assert_eq!(loaded.config.rdp_password, environment.rdp_password);
        assert!(!loaded.allow_keyring);
    }
}
