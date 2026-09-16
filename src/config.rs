use std::{collections::HashSet, path::PathBuf};

use configparser::ini::Ini;

use crate::error::{AppError, Result};

const REQUIRED_KEYS: &[&str] = &["profile", "remote_port", "local_port", "rdp_host"];

#[derive(Debug, Clone, PartialEq)]
pub enum AuthType {
    /// IAM user + TOTP MFA device. Requires `serial` and `secret`.
    Mfa,
    /// AWS IAM Identity Center (SSO). SDK resolves creds from the SSO token cache.
    Sso,
}

#[derive(Debug, Clone)]
pub struct EnvironmentConfig {
    pub profile: String,
    pub auth_type: AuthType,
    pub serial: Option<String>,
    pub secret: Option<String>,
    pub instance: Option<String>,
    pub instance_tag: Option<String>,
    pub remote_port: u16,
    pub local_port: u16,
    pub rdp_host: String,
    pub rdp_user: Option<String>,
    pub rdp_password: Option<String>,
    pub shared_drives: Option<String>,
    pub shared_folder: Option<String>,
    pub optimize_connection: Option<bool>,
}

impl EnvironmentConfig {
    fn from_section(name: &str, ini: &Ini) -> Result<Self> {
        let get = |key: &str| -> Result<String> {
            ini.get(name, key)
                .map(|v| v.trim().to_owned())
                .ok_or_else(|| AppError::Config(format!("[{name}] missing required key: `{key}`")))
        };

        let get_opt =
            |key: &str| -> Option<String> { ini.get(name, key).map(|v| v.trim().to_owned()) };

        let present: HashSet<String> = ini
            .get_map()
            .and_then(|m| m.get(name).cloned())
            .unwrap_or_default()
            .into_keys()
            .map(|k| k.to_lowercase())
            .collect();

        let missing: Vec<&str> = REQUIRED_KEYS
            .iter()
            .filter(|&&k| !present.contains(k))
            .copied()
            .collect();

        if !missing.is_empty() {
            return Err(AppError::Config(format!(
                "[{name}] missing keys: {}",
                missing.join(", ")
            )));
        }

        if ini.get(name, "instance").is_none() && ini.get(name, "instance_tag").is_none() {
            return Err(AppError::Config(format!(
                "[{name}] must specify either `instance` or `instance_tag`"
            )));
        }

        // We check for secret later as it might be in keyring,
        // but we can't check keyring here easily without breaking purity.
        // Actually, we'll let cmd_launch handle the "missing secret" error.

        let parse_port = |key: &str| -> Result<u16> {
            get(key)?
                .parse::<u16>()
                .map_err(|_| AppError::Config(format!("`{key}` must be a valid port (1–65535)")))
        };

        let serial = get_opt("serial");
        let auth_type = if serial.is_some() {
            AuthType::Mfa
        } else {
            AuthType::Sso
        };

        let parse_bool = |key: &str| -> Option<bool> {
            ini.get(name, key)
                .and_then(|v| match v.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" => Some(true),
                    "false" | "0" | "no" => Some(false),
                    _ => None,
                })
        };

        let shared_drives = get_opt("shared_drives");
        let shared_folder = get_opt("shared_folder");
        let optimize_connection = parse_bool("optimize_connection");

        Ok(Self {
            profile: get("profile")?,
            auth_type,
            serial,
            secret: get_opt("secret"),
            instance: get_opt("instance"),
            instance_tag: get_opt("instance_tag"),
            remote_port: parse_port("remote_port")?,
            local_port: parse_port("local_port")?,
            rdp_host: get("rdp_host")?,
            rdp_user: get_opt("rdp_user"),
            rdp_password: get_opt("rdp_password"),
            shared_drives,
            shared_folder,
            optimize_connection,
        })
    }

    pub fn to_map(&self) -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("profile".into(), self.profile.clone());
        if let Some(serial) = &self.serial {
            m.insert("serial".into(), serial.clone());
        }
        if let Some(sec) = &self.secret {
            m.insert("secret".into(), sec.clone());
        }
        if let Some(inst) = &self.instance {
            m.insert("instance".into(), inst.clone());
        }
        if let Some(tag) = &self.instance_tag {
            m.insert("instance_tag".into(), tag.clone());
        }
        m.insert("remote_port".into(), self.remote_port.to_string());
        m.insert("local_port".into(), self.local_port.to_string());
        m.insert("rdp_host".into(), self.rdp_host.clone());

        if let Some(user) = &self.rdp_user {
            m.insert("rdp_user".into(), user.clone());
        }
        if let Some(pass) = &self.rdp_password {
            m.insert("rdp_password".into(), pass.clone());
        }
        if let Some(drives) = &self.shared_drives {
            m.insert("shared_drives".into(), drives.clone());
        }
        if let Some(folder) = &self.shared_folder {
            m.insert("shared_folder".into(), folder.clone());
        }
        if let Some(opt) = self.optimize_connection {
            m.insert("optimize_connection".into(), opt.to_string());
        }
        m
    }
}

pub struct ConfigLoader {
    path: PathBuf,
}

impl ConfigLoader {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self, env_name: &str) -> Result<EnvironmentConfig> {
        let ini = self.parse()?;
        if !ini.sections().contains(&env_name.to_string()) {
            let available = ini.sections().join(", ");
            return Err(AppError::Config(format!(
                "Environment `{env_name}` not found. Available: {}",
                if available.is_empty() {
                    "(none)".into()
                } else {
                    available
                }
            )));
        }
        EnvironmentConfig::from_section(env_name, &ini)
    }

    pub fn list_environments(&self) -> Result<Vec<String>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        Ok(self.parse()?.sections())
    }

    pub fn get_all(&self) -> Result<std::collections::HashMap<String, EnvironmentConfig>> {
        let ini = self.parse()?;
        let mut all = std::collections::HashMap::new();
        for section in ini.sections() {
            if let Ok(config) = EnvironmentConfig::from_section(&section, &ini) {
                all.insert(section, config);
            }
        }
        Ok(all)
    }

    pub fn save(&self, env_name: &str, config: &EnvironmentConfig) -> Result<()> {
        let mut ini = if self.path.exists() {
            self.parse()?
        } else {
            Ini::new()
        };

        let map = config.to_map();
        for (k, v) in map {
            ini.set(env_name, &k, Some(v));
        }

        ini.write(self.path.to_str().unwrap_or_default())
            .map_err(|e| AppError::Config(format!("Failed to save config: {e}")))?;

        Ok(())
    }

    pub fn delete(&self, env_name: &str) -> Result<()> {
        let ini = self.parse()?;
        if !ini.sections().contains(&env_name.to_string()) {
            return Err(AppError::Config(format!(
                "Environment `{env_name}` not found"
            )));
        }

        // configparser doesn't have a direct section delete?
        // Let's check docs or just rebuild the ini without that section.
        let mut new_ini = Ini::new();
        for section in ini.sections() {
            if section != env_name {
                if let Some(map) = ini.get_map() {
                    if let Some(sec_map) = map.get(&section) {
                        for (k, v) in sec_map {
                            new_ini.set(&section, k, v.clone());
                        }
                    }
                }
            }
        }

        new_ini
            .write(self.path.to_str().unwrap_or_default())
            .map_err(|e| AppError::Config(format!("Failed to save config after delete: {e}")))?;

        Ok(())
    }

    fn parse(&self) -> Result<Ini> {
        if !self.path.exists() {
            return Err(AppError::Config(format!(
                "Config file not found: {}",
                self.path.display()
            )));
        }
        let mut ini = Ini::new();
        ini.load(self.path.to_str().unwrap_or_default())
            .map_err(|e| AppError::Config(format!("Failed to parse config file: {e}")))?;
        Ok(ini)
    }
}
