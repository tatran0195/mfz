use crate::{
    auth::CredentialStore,
    cli,
    config::{AuthType, ConfigLoader, EnvironmentConfig},
    error::{self, Result},
    ui::*,
};
use colored::Colorize;

pub async fn run(sub: cli::ConfigCommand) -> Result<()> {
    match sub {
        cli::ConfigCommand::List { config } => {
            print_section("Environment Configurations");
            let loader = ConfigLoader::from_cli(config.as_deref());
            let envs = loader.get_all()?;

            if envs.is_empty() {
                print_warning("No environments configured.");
            } else {
                for (name, env) in envs {
                    println!("    {} {}", "▸".cyan(), name.white().bold());
                    if let Some(inst) = env.instance {
                        println!("      {:<12} {}", "Instance:".dimmed(), inst);
                    }
                    if let Some(tag) = env.instance_tag {
                        println!("      {:<12} {}", "Inst Tag:".dimmed(), tag);
                    }
                    if let Some(ref key_id) = env.aws_access_key_id {
                        println!("      {:<12} Direct Key ({})", "AWS Auth:".dimmed(), key_id);
                    } else if let Some(ref profile) = env.profile {
                        println!("      {:<12} {}", "Profile:".dimmed(), profile);
                    }
                    if let Some(ref region) = env.region {
                        println!("      {:<12} {}", "Region:".dimmed(), region);
                    }
                    println!("      {:<12} {:?}", "Auth Type:".dimmed(), env.auth_type);

                    let config_source_name = loader
                        .path()
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Config");

                    if env.auth_type == AuthType::Mfa {
                        let secret_loc = if env.secret.is_some() {
                            format!("{config_source_name} (Plaintext)").yellow()
                        } else {
                            match CredentialStore::get_totp_secret(&name) {
                                Ok(Some(_)) => "Windows Credential Manager".green(),
                                Ok(None) => "Missing".red(),
                                Err(error) => format!("Unavailable: {error}").yellow(),
                            }
                        };
                        println!("      {:<12} {}", "MFA Secret:".dimmed(), secret_loc);
                    }

                    let pass_loc = if env.rdp_password.is_some() {
                        format!("{config_source_name} (Plaintext)").yellow()
                    } else {
                        match CredentialStore::get_rdp_password(&name) {
                            Ok(Some(_)) => "Windows Credential Manager".green(),
                            Ok(None) => "Not configured".dimmed(),
                            Err(error) => format!("Unavailable: {error}").yellow(),
                        }
                    };
                    println!("      {:<12} {}", "RDP Pass:".dimmed(), pass_loc);

                    println!(
                        "      {:<12} {}:{}",
                        "Remote:".dimmed(),
                        env.rdp_host,
                        env.remote_port
                    );
                    println!(
                        "      {:<12} 127.0.0.1:{}",
                        "Local:".dimmed(),
                        env.local_port
                    );
                    if let Some(user) = env.rdp_user {
                        println!("      {:<12} {}", "RDP User:".dimmed(), user);
                    }
                    if let Some(drives) = env.shared_drives {
                        println!("      {:<12} {}", "Drives:".dimmed(), drives);
                    }
                    if let Some(folder) = env.shared_folder {
                        println!("      {:<12} {}", "Folder:".dimmed(), folder);
                    }
                    if let Some(fullscreen) = env.fullscreen {
                        println!("      {:<12} {}", "Fullscreen:".dimmed(), fullscreen);
                    }
                    if let Some(optimize) = env.optimize {
                        println!("      {:<12} {}", "Optimize:".dimmed(), optimize);
                    } else if let Some(optimize) = env.optimize_connection {
                        println!("      {:<12} {}", "Optimize:".dimmed(), optimize);
                    }
                    println!();
                }
            }
        }
        cli::ConfigCommand::Add {
            name,
            profile,
            aws_access_key_id,
            aws_secret_access_key,
            region,
            serial,
            secret,
            instance,
            instance_tag,
            remote_port,
            local_port,
            rdp_host,
            user,
            pass,
            shared_drives,
            shared_folder,
            fullscreen,
            optimize,
            optimize_connection,
            config,
        } => {
            print_section(&format!("Adding Environment: {name}"));
            let loader = ConfigLoader::from_cli(config.as_deref());

            if instance.is_none() && instance_tag.is_none() {
                return Err(error::AppError::Config(
                    "Must specify either `--instance` or `--instance-tag`".into(),
                ));
            }

            if profile.is_none() && aws_access_key_id.is_none() {
                return Err(error::AppError::Config(
                    "Must specify either `--aws-access-key-id` (standalone) or `--profile`".into(),
                ));
            }

            let auth_type = if serial.is_some() {
                AuthType::Mfa
            } else {
                AuthType::Provider
            };

            let opt_profile = optimize.or_else(|| {
                optimize_connection.map(|enabled| {
                    if enabled {
                        "speed".to_string()
                    } else {
                        "none".to_string()
                    }
                })
            });

            let new_env = EnvironmentConfig {
                profile,
                aws_access_key_id,
                aws_secret_access_key,
                region,
                auth_type,
                serial,
                secret,
                instance,
                instance_tag,
                remote_port,
                local_port,
                rdp_host,
                rdp_user: user,
                rdp_password: pass,
                shared_drives,
                shared_folder,
                optimize_connection,
                fullscreen,
                optimize: opt_profile,
                opt_audio: None,
                opt_wallpaper: None,
                opt_font_smoothing: None,
            };

            let spinner =
                Spinner::new(format!("Saving `{name}` to `{}`…", loader.path().display()));
            loader.insert(&name, &new_env)?;
            spinner.success(format!("Environment `{name}` added successfully."));

            print_info("Tunneled RDP uses a numeric loopback endpoint; manage optional aliases with `mfz hosts add`.");
        }
        cli::ConfigCommand::Edit {
            name,
            profile,
            aws_access_key_id,
            aws_secret_access_key,
            region,
            serial,
            secret,
            instance,
            instance_tag,
            remote_port,
            local_port,
            rdp_host,
            user,
            pass,
            shared_drives,
            shared_folder,
            fullscreen,
            optimize,
            optimize_connection,
            config,
        } => {
            print_section(&format!("Editing Environment: {name}"));
            let loader = ConfigLoader::from_cli(config.as_deref());
            let spinner = Spinner::new(format!(
                "Updating `{name}` in `{}`…",
                loader.path().display()
            ));
            loader.update(&name, |env| {
                if let Some(profile) = profile {
                    env.profile = Some(profile);
                }
                if let Some(key_id) = aws_access_key_id {
                    env.aws_access_key_id = Some(key_id);
                }
                if let Some(secret_key) = aws_secret_access_key {
                    env.aws_secret_access_key = Some(secret_key);
                }
                if let Some(region) = region {
                    env.region = Some(region);
                }
                if let Some(serial) = serial {
                    env.serial = Some(serial);
                    env.auth_type = AuthType::Mfa;
                }
                if let Some(secret) = secret {
                    env.secret = Some(secret);
                }
                if let Some(instance) = instance {
                    env.instance = Some(instance);
                    env.instance_tag = None;
                }
                if let Some(instance_tag) = instance_tag {
                    env.instance_tag = Some(instance_tag);
                    env.instance = None;
                }
                if let Some(remote_port) = remote_port {
                    env.remote_port = remote_port;
                }
                if let Some(local_port) = local_port {
                    env.local_port = local_port;
                }
                if let Some(rdp_host) = rdp_host {
                    env.rdp_host = rdp_host;
                }
                if user.is_some() {
                    env.rdp_user = user;
                }
                if pass.is_some() {
                    env.rdp_password = pass;
                }
                if shared_drives.is_some() {
                    env.shared_drives = shared_drives;
                }
                if shared_folder.is_some() {
                    env.shared_folder = shared_folder;
                }
                if fullscreen.is_some() {
                    env.fullscreen = fullscreen;
                }
                if optimize.is_some() {
                    env.optimize = optimize;
                } else if optimize_connection.is_some() {
                    env.optimize = optimize_connection.map(|enabled| {
                        if enabled {
                            "speed".to_string()
                        } else {
                            "none".to_string()
                        }
                    });
                }
                if optimize_connection.is_some() {
                    env.optimize_connection = optimize_connection;
                }

                Ok(())
            })?;
            spinner.success(format!("Environment `{name}` updated successfully."));
        }
        cli::ConfigCommand::Remove { name, config } => {
            print_section(&format!("Removing Environment: {name}"));
            let loader = ConfigLoader::from_cli(config.as_deref());
            let spinner = Spinner::new(format!(
                "Deleting `{name}` from `{}`…",
                loader.path().display()
            ));
            loader.delete(&name)?;
            CredentialStore::delete_secrets(&name).map_err(|error| {
                error::AppError::Config(format!(
                    "Configuration removed, but secure-store cleanup failed: {error}"
                ))
            })?;
            spinner.success(format!("Environment `{name}` removed."));
        }
        cli::ConfigCommand::Secure { name, config } => {
            print_section(&format!("Securing Environment: {name}"));
            let loader = ConfigLoader::from_cli(config.as_deref());
            if loader.secure(&name)? {
                print_success("Secrets verified in the secure store and removed from the atomically saved config.");
            } else {
                print_warning("No plaintext secrets found to secure.");
            }
        }
        cli::ConfigCommand::SetAwsSecret { name, secret } => {
            print_section(&format!("Set AWS Secret Key: {name}"));
            let key = if let Some(secret) = secret {
                secret
            } else {
                inquire::Password::new("Enter AWS Secret Access Key:")
                    .without_confirmation()
                    .prompt()
                    .map_err(|error| error::AppError::Config(error.to_string()))?
            };
            CredentialStore::set_aws_secret_key(&name, &key)?;
            println!(
                "{} AWS secret access key saved to keyring for `{name}`.",
                "✔".green()
            );
        }
    }
    Ok(())
}
