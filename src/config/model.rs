use configparser::ini::Ini;

use crate::{
    auth::CredentialStore,
    error::{AppError, Result},
    validation,
};

const REQUIRED_KEYS: &[&str] = &["remote_port", "local_port", "rdp_host"];

#[derive(Debug, Clone, PartialEq)]
pub enum AuthType {
    /// IAM user + TOTP MFA device. Requires `serial` and `secret`.
    Mfa,
    /// Non-MFA SDK/direct credentials, including static keys, roles and cached SSO.
    Provider,
}

#[derive(Clone)]
pub struct EnvironmentConfig {
    pub profile: Option<String>,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub region: Option<String>,
    pub auth_type: AuthType,
    pub serial: Option<String>,
    /// TOTP Base-32 secret. If omitted from configuration, resolved from
    /// the system keyring at session launch.
    pub secret: Option<String>,
    pub instance: Option<String>,
    pub instance_tag: Option<String>,
    pub remote_port: u16,
    pub local_port: u16,
    pub rdp_host: String,
    pub rdp_user: Option<String>,
    /// RDP session password. If omitted from configuration, resolved from
    /// the system keyring at session launch.
    pub rdp_password: Option<String>,
    pub shared_drives: Option<String>,
    pub shared_folder: Option<String>,
    pub optimize_connection: Option<bool>,
    pub fullscreen: Option<bool>,
    pub optimize: Option<String>,
    pub opt_audio: Option<bool>,
    pub opt_wallpaper: Option<bool>,
    pub opt_font_smoothing: Option<bool>,
}

impl std::fmt::Debug for EnvironmentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentConfig")
            .field("profile", &self.profile)
            .field("region", &self.region)
            .field("auth_type", &self.auth_type)
            .field("instance", &self.instance)
            .field("instance_tag", &self.instance_tag)
            .field("rdp_host", &self.rdp_host)
            .field("local_port", &self.local_port)
            .field("remote_port", &self.remote_port)
            .field(
                "aws_secret_access_key",
                &self.aws_secret_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .field(
                "rdp_password",
                &self.rdp_password.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

impl EnvironmentConfig {
    pub(super) fn from_section(name: &str, ini: &Ini) -> Result<Self> {
        validation::environment_name(name)?;
        let get_opt = |key: &str| ini.get(name, key).filter(|value| !value.is_empty());
        let get = |key: &str| -> Result<String> {
            get_opt(key)
                .ok_or_else(|| AppError::Config(format!("[{name}] missing required key: `{key}`")))
        };
        let missing: Vec<_> = REQUIRED_KEYS
            .iter()
            .filter(|key| get_opt(key).is_none())
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
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| {
                    AppError::Config(format!("[{name}] `{key}` must be a valid port (1–65535)"))
                })
        };
        let parse_bool = |key: &str| -> Result<Option<bool>> {
            get_opt(key)
                .map(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" => Ok(true),
                    "false" | "0" | "no" => Ok(false),
                    _ => Err(AppError::Config(format!(
                        "[{name}] `{key}` must be a boolean"
                    ))),
                })
                .transpose()
        };
        let serial = get_opt("serial");
        let optimize_connection = parse_bool("optimize_connection")?;
        let env = Self {
            profile: get_opt("profile"),
            aws_access_key_id: get_opt("aws_access_key_id").or_else(|| get_opt("access_key_id")),
            aws_secret_access_key: get_opt("aws_secret_access_key")
                .or_else(|| get_opt("secret_access_key")),
            region: get_opt("region"),
            auth_type: if serial.is_some() {
                AuthType::Mfa
            } else {
                AuthType::Provider
            },
            serial,
            secret: get_opt("secret"),
            instance: get_opt("instance"),
            instance_tag: get_opt("instance_tag"),
            remote_port: parse_port("remote_port")?,
            local_port: parse_port("local_port")?,
            rdp_host: get("rdp_host")?,
            rdp_user: get_opt("rdp_user"),
            rdp_password: ini.get(name, "rdp_password"),
            shared_drives: get_opt("shared_drives").or_else(|| get_opt("share_drives")),
            shared_folder: get_opt("shared_folder").or_else(|| get_opt("share_folder")),
            optimize_connection,
            fullscreen: parse_bool("fullscreen")?,
            optimize: get_opt("optimize")
                .or_else(|| get_opt("optimize_profile"))
                .or_else(|| {
                    optimize_connection.map(|enabled| {
                        if enabled {
                            "speed".into()
                        } else {
                            "none".into()
                        }
                    })
                }),
            opt_audio: parse_bool("audio")?.or(parse_bool("opt_audio")?),
            opt_wallpaper: parse_bool("wallpaper")?.or(parse_bool("opt_wallpaper")?),
            opt_font_smoothing: parse_bool("font_smoothing")?.or(parse_bool("opt_font_smoothing")?),
        };
        env.validate()?;
        Ok(env)
    }

    /// Also applied to programmatically constructed models before persistence or export.
    pub fn validate(&self) -> Result<()> {
        if self.profile.is_none() && self.aws_access_key_id.is_none() {
            return Err(AppError::Config(
                "must specify either `aws_access_key_id` (standalone) or `profile` (AWS profile)"
                    .into(),
            ));
        }
        for (label, value) in [
            ("profile", &self.profile),
            ("AWS access key ID", &self.aws_access_key_id),
            ("region", &self.region),
            ("MFA serial", &self.serial),
            ("instance", &self.instance),
            ("instance tag", &self.instance_tag),
            ("RDP user", &self.rdp_user),
        ] {
            if let Some(value) = value {
                if value.trim().is_empty() {
                    return Err(AppError::Config(format!("{label} must not be empty")));
                }
                validation::single_line(value, label)?;
            }
        }
        if self.instance.is_some() == self.instance_tag.is_some() {
            return Err(AppError::Config(
                "Specify exactly one of `instance` or `instance_tag`".into(),
            ));
        }
        if let Some(tag) = &self.instance_tag {
            let (key, value) = tag
                .split_once('=')
                .ok_or_else(|| AppError::Config("Instance tag must be Key=Value".into()))?;
            if key.trim().is_empty() || value.is_empty() {
                return Err(AppError::Config(
                    "Instance tag key/value must not be empty".into(),
                ));
            }
        }
        if self.remote_port == 0 || self.local_port == 0 {
            return Err(AppError::Config("Ports must be in 1–65535".into()));
        }
        validation::hostname(&self.rdp_host)?;
        if let Some(drives) = &self.shared_drives {
            validation::drives(drives)?;
        }
        if let Some(optimize) = &self.optimize {
            validation::optimization(optimize)?;
        }
        if let Some(folder) = &self.shared_folder {
            validation::single_line(folder, "Shared folder")?;
        }
        if self.secret.is_some() && self.serial.is_none() {
            return Err(AppError::Config(
                "An MFA secret requires an MFA serial; refusing non-MFA fallback".into(),
            ));
        }
        if self.auth_type
            != if self.serial.is_some() {
                AuthType::Mfa
            } else {
                AuthType::Provider
            }
        {
            return Err(AppError::Config(
                "Authentication type disagrees with the MFA serial".into(),
            ));
        }
        for (label, value) in [
            ("AWS secret key", &self.aws_secret_access_key),
            ("MFA secret", &self.secret),
        ] {
            if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
                return Err(AppError::Config(format!("{label} must not be empty")));
            }
        }
        Ok(())
    }

    pub fn to_map(&self) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        if let Some(profile) = &self.profile {
            map.insert("profile".into(), profile.clone());
        }
        if let Some(key_id) = &self.aws_access_key_id {
            map.insert("aws_access_key_id".into(), key_id.clone());
        }
        if let Some(secret_key) = &self.aws_secret_access_key {
            map.insert("aws_secret_access_key".into(), secret_key.clone());
        }
        if let Some(region) = &self.region {
            map.insert("region".into(), region.clone());
        }
        if let Some(serial) = &self.serial {
            map.insert("serial".into(), serial.clone());
        }
        if let Some(secret) = &self.secret {
            map.insert("secret".into(), secret.clone());
        }
        if let Some(instance) = &self.instance {
            map.insert("instance".into(), instance.clone());
        }
        if let Some(tag) = &self.instance_tag {
            map.insert("instance_tag".into(), tag.clone());
        }
        map.insert("remote_port".into(), self.remote_port.to_string());
        map.insert("local_port".into(), self.local_port.to_string());
        map.insert("rdp_host".into(), self.rdp_host.clone());

        if let Some(user) = &self.rdp_user {
            map.insert("rdp_user".into(), user.clone());
        }
        if let Some(password) = &self.rdp_password {
            map.insert("rdp_password".into(), password.clone());
        }
        if let Some(drives) = &self.shared_drives {
            map.insert("shared_drives".into(), drives.clone());
        }
        if let Some(folder) = &self.shared_folder {
            map.insert("shared_folder".into(), folder.clone());
        }
        if let Some(optimize_connection) = self.optimize_connection {
            map.insert(
                "optimize_connection".into(),
                optimize_connection.to_string(),
            );
        }
        if let Some(fullscreen) = self.fullscreen {
            map.insert("fullscreen".into(), fullscreen.to_string());
        }
        if let Some(optimize) = &self.optimize {
            map.insert("optimize".into(), optimize.clone());
        }
        if let Some(audio) = self.opt_audio {
            map.insert("audio".into(), audio.to_string());
        }
        if let Some(wallpaper) = self.opt_wallpaper {
            map.insert("wallpaper".into(), wallpaper.to_string());
        }
        if let Some(font_smoothing) = self.opt_font_smoothing {
            map.insert("font_smoothing".into(), font_smoothing.to_string());
        }
        map
    }

    /// Resolves the TOTP secret, checking the config file first, then the system keyring.
    pub fn resolve_secret(&self, env_name: &str) -> Result<Option<String>> {
        if let Some(secret) = &self.secret {
            return Ok(Some(secret.clone()));
        }
        if self.auth_type == AuthType::Mfa {
            let secret = CredentialStore::get_totp_secret(env_name)?.ok_or_else(|| {
                AppError::Config(format!(
                    "No MFA secret found for [{env_name}] in config or keyring"
                ))
            })?;
            Ok(Some(secret))
        } else {
            Ok(None)
        }
    }

    /// Resolves the RDP password, checking config first, then the system keyring.
    pub fn resolve_rdp_password(&self, env_name: &str) -> Result<Option<String>> {
        if let Some(password) = &self.rdp_password {
            return Ok(Some(password.clone()));
        }
        CredentialStore::get_rdp_password(env_name)
    }

    /// Resolves the AWS secret access key, checking config first, then the system keyring.
    pub fn resolve_aws_secret_key(&self, env_name: &str) -> Result<Option<String>> {
        if let Some(secret_key) = &self.aws_secret_access_key {
            return Ok(Some(secret_key.clone()));
        }
        if self.aws_access_key_id.is_some() {
            let secret = CredentialStore::get_aws_secret_key(env_name)?.ok_or_else(|| {
                AppError::Config(format!(
                    "No AWS secret access key found for [{env_name}] in config or keyring"
                ))
            })?;
            return Ok(Some(secret));
        }
        Ok(None)
    }
}
