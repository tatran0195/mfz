use crate::error::{AppError, Result};
use configparser::ini::Ini;
use std::{collections::HashSet, path::PathBuf};

const REQUIRED_KEYS: &[&str] = &[
    "profile", "serial", "secret", "instance",
    "remote_port", "local_port", "rdp_host",
];

#[derive(Debug, Clone)]
pub struct EnvironmentConfig {
    pub profile:     String,
    pub serial:      String,
    pub secret:      String,
    pub instance:    String,
    pub remote_port: u16,
    pub local_port:  u16,
    pub rdp_host:    String,
}

impl EnvironmentConfig {
    fn from_section(name: &str, ini: &Ini) -> Result<Self> {
        let get = |key: &str| -> Result<String> {
            ini.get(name, key)
                .map(|v| v.trim().to_owned())
                .ok_or_else(|| {
                    AppError::Config(format!("[{name}] missing required key: `{key}`"))
                })
        };

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

        let parse_port = |key: &str| -> Result<u16> {
            get(key)?
                .parse::<u16>()
                .map_err(|_| AppError::Config(format!("`{key}` must be a valid port (1–65535)")))
        };

        Ok(Self {
            profile:     get("profile")?,
            serial:      get("serial")?,
            secret:      get("secret")?,
            instance:    get("instance")?,
            remote_port: parse_port("remote_port")?,
            local_port:  parse_port("local_port")?,
            rdp_host:    get("rdp_host")?,
        })
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
                if available.is_empty() { "(none)".into() } else { available }
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