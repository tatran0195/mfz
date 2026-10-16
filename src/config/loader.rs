use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use configparser::ini::Ini;

use crate::{
    auth::CredentialStore,
    error::{AppError, Result},
    sys::storage::{self, MutationLock},
    validation,
};

use super::{parse_ini, serialize_ini, EnvironmentConfig};

const KNOWN_KEYS: &[&str] = &[
    "profile",
    "aws_access_key_id",
    "access_key_id",
    "aws_secret_access_key",
    "secret_access_key",
    "region",
    "serial",
    "secret",
    "instance",
    "instance_tag",
    "remote_port",
    "local_port",
    "rdp_host",
    "rdp_user",
    "rdp_password",
    "shared_drives",
    "share_drives",
    "shared_folder",
    "share_folder",
    "optimize_connection",
    "fullscreen",
    "optimize",
    "optimize_profile",
    "audio",
    "opt_audio",
    "wallpaper",
    "opt_wallpaper",
    "font_smoothing",
    "opt_font_smoothing",
];

/// Preserve profile provenance: arbitrary portable files must not silently look
/// up a local secure-store entry just because they copy a trusted environment name.
pub struct LoadedEnvironment {
    pub name: String,
    pub config: EnvironmentConfig,
    pub allow_keyring: bool,
    /// Actual file supplying this model (not an unrelated config search result).
    pub source: PathBuf,
}

pub struct ConfigLoader {
    path: PathBuf,
}

impl ConfigLoader {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn resolve_cli_path(input: impl AsRef<Path>) -> PathBuf {
        let path = input.as_ref().to_path_buf();
        if path.is_file() {
            return path;
        }
        if path.extension().is_none() {
            let with_ini = path.with_extension("ini");
            if with_ini.is_file() {
                return with_ini;
            }
        }
        path
    }

    /// CLI omission means portable/user default discovery; an explicit path
    /// without extension auto-detects a local `.ini` if present.
    pub fn from_cli(path: Option<&str>) -> Self {
        path.map(Self::resolve_cli_path)
            .map(Self::new)
            .unwrap_or_else(|| Self {
                path: Self::resolve_default_config_path(),
            })
    }

    pub fn resolve_default_config_path() -> PathBuf {
        // 1. Current working directory
        for name in ["mfz", "mfz.ini", ".mfz"] {
            let path = PathBuf::from(name);
            if path.is_file() {
                return path;
            }
        }

        // 2. Application executable directory (portable deployment)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                for name in ["mfz", "mfz.ini"] {
                    let candidate = parent.join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }

        // 3. User & platform config locations
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            for name in ["config", "mfz", "mfz.ini"] {
                let candidate = PathBuf::from(&xdg).join("mfz").join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }

        for home_var in ["USERPROFILE", "HOME"] {
            if let Some(base) = std::env::var_os(home_var) {
                let base_path = PathBuf::from(base);
                for name in ["config", "mfz", "mfz.ini"] {
                    let candidate = base_path.join(".config").join("mfz").join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
                for name in ["config", "mfz", "mfz.ini"] {
                    let candidate = base_path.join(".mfz").join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }

        if let Some(appdata) = std::env::var_os("APPDATA") {
            let base_path = PathBuf::from(appdata);
            for name in ["config", "mfz", "mfz.ini"] {
                let candidate = base_path.join("mfz").join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }

        PathBuf::from("mfz.ini")
    }

    pub fn load(&self, env_name: &str) -> Result<EnvironmentConfig> {
        let name = validation::environment_name(env_name)?;
        let ini = self.parse()?;
        self.load_section(&name, &ini)
    }

    fn load_section(&self, name: &str, ini: &Ini) -> Result<EnvironmentConfig> {
        if !ini.sections().iter().any(|section| section == name) {
            return Err(AppError::Config(format!("Environment `{name}` not found")));
        }
        EnvironmentConfig::from_section(name, ini)
    }

    pub fn load_from_file_or_name(&self, input: &str) -> Result<(String, EnvironmentConfig)> {
        let loaded = self.load_for_launch(input)?;
        Ok((loaded.name, loaded.config))
    }

    pub fn load_for_launch(&self, input: &str) -> Result<LoadedEnvironment> {
        let path = Path::new(input);
        let is_profile = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mfz"))
            || path.is_file();
        if !is_profile {
            let name = validation::environment_name(input)?;
            return Ok(LoadedEnvironment {
                config: self.load(&name)?,
                name,
                allow_keyring: true,
                source: self.path.clone(),
            });
        }
        let content = storage::read_profile(path)?;
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::Config("Profile filename must be valid UTF-8".into()))?;
        let lines: Vec<_> = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(['#', ';']))
            .collect();
        if lines.is_empty() {
            let name = validation::environment_name(stem)?;
            return Ok(LoadedEnvironment {
                config: self.load(&name)?,
                name,
                allow_keyring: false,
                source: self.path.clone(),
            });
        }
        let mut target: Option<String> = None;
        let mut config_path: Option<PathBuf> = None;
        for line in lines.iter().take_while(|line| !line.starts_with('[')) {
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim();
                let value = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .or_else(|| {
                        value
                            .strip_prefix('\'')
                            .and_then(|value| value.strip_suffix('\''))
                    })
                    .unwrap_or(value)
                    .to_owned();
                match key.trim().to_ascii_lowercase().as_str() {
                    "env" | "environment" => {
                        let value = validation::environment_name(&value)?;
                        if target.as_ref().is_some_and(|old| old != &value) {
                            return Err(AppError::Config(
                                "Conflicting profile pointer targets".into(),
                            ));
                        }
                        target = Some(value);
                    }
                    "config" => {
                        if value.is_empty() || config_path.is_some() {
                            return Err(AppError::Config(
                                "Invalid or repeated pointer config path".into(),
                            ));
                        }
                        config_path = Some(PathBuf::from(value));
                    }
                    _ => {}
                }
            }
        }
        if let Some(name) = target {
            if lines.iter().any(|line| line.starts_with('[')) {
                return Err(AppError::Config(
                    "A profile must be a pointer OR a standalone section, not both".into(),
                ));
            }
            let selected = match config_path {
                Some(config) if config.is_relative() => {
                    path.parent().unwrap_or_else(|| Path::new(".")).join(config)
                }
                Some(config) => config,
                None => self.path.clone(),
            };
            let loader = Self { path: selected };
            let config = loader.load(&name)?;
            let allow_keyring = std::fs::canonicalize(&loader.path)?
                == std::fs::canonicalize(&self.path).unwrap_or_default();
            return Ok(LoadedEnvironment {
                name,
                config,
                allow_keyring,
                source: loader.path,
            });
        }
        if config_path.is_some() {
            return Err(AppError::Config(
                "A config pointer requires an exact env/environment key".into(),
            ));
        }
        let wrapped = if lines.iter().any(|line| line.starts_with('[')) {
            content
        } else {
            format!("[session]\n{content}")
        };
        let ini = parse_ini(wrapped)?;
        let sections = ini.sections();
        if sections.len() != 1 {
            return Err(AppError::Config(
                "Standalone profiles must contain exactly one environment section".into(),
            ));
        }
        let section = &sections[0];
        let config = EnvironmentConfig::from_section(section, &ini)?;
        let name = validation::environment_name(if section == "session" { stem } else { section })?;
        Ok(LoadedEnvironment {
            name,
            config,
            allow_keyring: false,
            source: path.to_owned(),
        })
    }

    pub fn list_environments(&self) -> Result<Vec<String>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let mut names = self.parse()?.sections();
        names.sort();
        Ok(names)
    }

    pub fn get_all(&self) -> Result<BTreeMap<String, EnvironmentConfig>> {
        let ini = self.parse()?;
        let mut result = BTreeMap::new();
        for section in ini.sections() {
            result.insert(
                section.clone(),
                EnvironmentConfig::from_section(&section, &ini)?,
            );
        }
        Ok(result)
    }

    fn replace_fields(ini: &mut Ini, name: &str, config: &EnvironmentConfig) {
        for key in KNOWN_KEYS {
            ini.remove_key(name, key);
        }
        for (key, value) in config.to_map() {
            ini.set(name, &key, Some(value));
        }
    }

    fn persist(&self, ini: &Ini) -> Result<()> {
        storage::atomic_write(&self.path, serialize_ini(ini).as_bytes(), true, true)
    }

    pub fn save(&self, env_name: &str, config: &EnvironmentConfig) -> Result<()> {
        let name = validation::environment_name(env_name)?;
        config.validate()?;
        let _lock = MutationLock::acquire(&self.path)?;
        let mut ini = if self.path.exists() {
            self.parse()?
        } else {
            parse_ini(String::new())?
        };
        Self::replace_fields(&mut ini, &name, config);
        self.persist(&ini)
    }

    pub fn insert(&self, env_name: &str, config: &EnvironmentConfig) -> Result<()> {
        let name = validation::environment_name(env_name)?;
        config.validate()?;
        let _lock = MutationLock::acquire(&self.path)?;
        let mut ini = if self.path.exists() {
            self.parse()?
        } else {
            parse_ini(String::new())?
        };
        if ini.sections().contains(&name) {
            return Err(AppError::Config(format!(
                "Environment `{name}` already exists; use config edit"
            )));
        }
        Self::replace_fields(&mut ini, &name, config);
        self.persist(&ini)
    }

    /// Read/modify/write one current model while holding the same mutation lock.
    pub fn update(
        &self,
        env_name: &str,
        edit: impl FnOnce(&mut EnvironmentConfig) -> Result<()>,
    ) -> Result<()> {
        let name = validation::environment_name(env_name)?;
        let _lock = MutationLock::acquire(&self.path)?;
        let mut ini = self.parse()?;
        let mut config = self.load_section(&name, &ini)?;
        edit(&mut config)?;
        config.validate()?;
        Self::replace_fields(&mut ini, &name, &config);
        self.persist(&ini)
    }

    /// Remove plaintext only after a durable backend can read back the same secret.
    /// If any backend operation or persistence fails, the original config remains.
    pub fn secure(&self, env_name: &str) -> Result<bool> {
        self.secure_with(env_name, |kind, value| match kind {
            "AWS" => {
                CredentialStore::set_aws_secret_key(env_name, value)?;
                CredentialStore::get_aws_secret_key(env_name)
            }
            "TOTP" => {
                CredentialStore::set_totp_secret(env_name, value)?;
                CredentialStore::get_totp_secret(env_name)
            }
            "RDP" => {
                CredentialStore::set_rdp_password(env_name, value)?;
                CredentialStore::get_rdp_password(env_name)
            }
            _ => unreachable!("closed set of migration secret kinds"),
        })
    }

    pub(super) fn secure_with(
        &self,
        env_name: &str,
        mut store: impl FnMut(&str, &str) -> Result<Option<String>>,
    ) -> Result<bool> {
        let mut changed = false;
        self.update(env_name, |config| {
            for (kind, field) in [
                ("AWS", &mut config.aws_secret_access_key),
                ("TOTP", &mut config.secret),
                ("RDP", &mut config.rdp_password),
            ] {
                if let Some(value) = field.as_ref() {
                    if store(kind, value)?.as_deref() != Some(value.as_str()) {
                        return Err(AppError::Config(format!(
                            "{kind} secure-store verification failed; plaintext was not removed"
                        )));
                    }
                    *field = None;
                    changed = true;
                }
            }
            Ok(())
        })?;
        Ok(changed)
    }

    pub fn delete(&self, env_name: &str) -> Result<()> {
        let name = validation::environment_name(env_name)?;
        let _lock = MutationLock::acquire(&self.path)?;
        let mut ini = self.parse()?;
        if ini.remove_section(&name).is_none() {
            return Err(AppError::Config(format!("Environment `{name}` not found")));
        }
        self.persist(&ini)
    }

    fn parse(&self) -> Result<Ini> {
        parse_ini(storage::read_profile(&self.path)?)
    }
}
