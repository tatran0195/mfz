use std::path::PathBuf;

use clap::Parser;
use cli::{Cli, HostsCommand, QrCommand, TopCommand};
use colored::Colorize;
use config::ConfigLoader;
use error::Result;
use hosts::HostsManager;
use inquire::Select;
use mfa::MfaSessionManager;
use mfz::{aws, cli, config, credentials, error, hosts, mfa, qr, rdp, shell, ssm, ui};
use ssm::{start_with_retries, SsmSession};
use tracing::info;
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use ui::*;

// ─── Logging ──────────────────────────────────────────────────────────────────

fn setup_logging(enable_file: bool) {
    let mut layers = Vec::new();

    // Console layer (always on)
    let console_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(false)
        .with_filter(EnvFilter::new("mfz=info,warn"))
        .boxed();
    layers.push(console_layer);

    // File layer (optional)
    if enable_file {
        let file_appender = rolling::daily(".", "mfz.log");
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        std::mem::forget(_guard); // keep flushing until process exits

        let file_layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(false)
            .boxed();
        layers.push(file_layer);
    }

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(layers)
        .init();
}

// ─── Entry ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    #[cfg(windows)]
    {
        // Enable ANSI support
        colored::control::set_virtual_terminal(true).ok();
        // Set UTF-8 code page natively — no `cmd /C chcp 65001` subprocess needed
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCP(65001);
            windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
        }
        // Attempt to set a modern font (Consolas) for better symbol rendering
        setup_console_font();
    }

    let cli = Cli::parse();
    setup_logging(cli.log);
    print_banner();

    if let Err(e) = dispatch(cli).await {
        print_error(&e.to_string());
        // If launched via .mfz file association, pause so user can inspect any error
        let is_mfz_launch = std::env::args().any(|a| a.ends_with(".mfz") || a.ends_with(".mfz\""));
        if is_mfz_launch {
            println!("\n  {} Press Enter to exit...", "ℹ".cyan());
            let mut buf = String::new();
            let _ = std::io::stdin().read_line(&mut buf);
        }
        std::process::exit(e.exit_code());
    }
}

async fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        TopCommand::Launch(args) => cmd_launch(args).await,
        TopCommand::List(args) => cmd_list(args),
        TopCommand::Hosts(sub) => cmd_hosts(sub),
        TopCommand::Qr(sub) => cmd_qr(sub).await,
        TopCommand::Config(sub) => cmd_config(sub).await,
        TopCommand::Register => cmd_register(),
        TopCommand::Unregister => cmd_unregister(),
        TopCommand::Export(args) => cmd_export(args).await,
    }
}

// ─── config ───────────────────────────────────────────────────────────────────

async fn cmd_config(sub: cli::ConfigCommand) -> Result<()> {
    match sub {
        cli::ConfigCommand::List { config } => {
            print_section("Environment Configurations");
            let loader = ConfigLoader::new(&config);
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
                    } else if let Some(ref p) = env.profile {
                        println!("      {:<12} {}", "Profile:".dimmed(), p);
                    }
                    if let Some(ref reg) = env.region {
                        println!("      {:<12} {}", "Region:".dimmed(), reg);
                    }
                    println!("      {:<12} {:?}", "Auth Type:".dimmed(), env.auth_type);

                    if env.auth_type == crate::config::AuthType::Mfa {
                        let has_secret = env.secret.is_some()
                            || crate::credentials::CredentialStore::get_totp_secret(&name)
                                .map(|s| s.is_some())
                                .unwrap_or(false);
                        let secret_loc = if env.secret.is_some() {
                            "mfz.ini (Plaintext)".yellow()
                        } else if has_secret {
                            "System Keyring (Secure)".green()
                        } else {
                            "Missing".red()
                        };
                        println!("      {:<12} {}", "MFA Secret:".dimmed(), secret_loc);
                    }

                    let has_pass = env.rdp_password.is_some()
                        || crate::credentials::CredentialStore::get_rdp_password(&name)
                            .map(|s| s.is_some())
                            .unwrap_or(false);
                    if has_pass {
                        let pass_loc = if env.rdp_password.is_some() {
                            "mfz.ini (Plaintext)".yellow()
                        } else {
                            "System Keyring (Secure)".green()
                        };
                        println!("      {:<12} {}", "RDP Pass:".dimmed(), pass_loc);
                    }

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
                    if let Some(fs) = env.fullscreen {
                        println!("      {:<12} {}", "Fullscreen:".dimmed(), fs);
                    }
                    if let Some(opt) = env.optimize {
                        println!("      {:<12} {}", "Optimize:".dimmed(), opt);
                    } else if let Some(opt) = env.optimize_connection {
                        println!("      {:<12} {}", "Optimize:".dimmed(), opt);
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
            let loader = ConfigLoader::new(&config);

            if instance.is_none() && instance_tag.is_none() {
                return Err(crate::error::AppError::Config(
                    "Must specify either `--instance` or `--instance-tag`".into(),
                ));
            }

            if profile.is_none() && aws_access_key_id.is_none() {
                return Err(crate::error::AppError::Config(
                    "Must specify either `--aws-access-key-id` (standalone) or `--profile`".into(),
                ));
            }

            let auth_type = if serial.is_some() {
                crate::config::AuthType::Mfa
            } else {
                crate::config::AuthType::Sso
            };

            let opt_profile = optimize.or_else(|| {
                optimize_connection.map(|b| {
                    if b {
                        "speed".to_string()
                    } else {
                        "none".to_string()
                    }
                })
            });

            let new_env = crate::config::EnvironmentConfig {
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

            let spinner = Spinner::new(format!("Saving `{name}` to `{config}`…"));
            loader.save(&name, &new_env)?;
            spinner.success(format!("Environment `{name}` added successfully."));

            // Automatically ensure host alias is in hosts file
            let hosts_mgr = HostsManager::new();
            hosts_mgr.ensure_host("127.0.0.1", &new_env.rdp_host)?;
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
            let loader = ConfigLoader::new(&config);
            let mut env = loader.load(&name)?;

            if let Some(p) = profile {
                env.profile = Some(p);
            }
            if let Some(key_id) = aws_access_key_id {
                env.aws_access_key_id = Some(key_id);
            }
            if let Some(sec_key) = aws_secret_access_key {
                env.aws_secret_access_key = Some(sec_key);
            }
            if let Some(r) = region {
                env.region = Some(r);
            }
            if let Some(s) = serial {
                env.serial = Some(s);
                env.auth_type = crate::config::AuthType::Mfa;
            }
            if let Some(s) = secret {
                env.secret = Some(s);
            }
            if let Some(i) = instance {
                env.instance = Some(i);
            }
            if let Some(it) = instance_tag {
                env.instance_tag = Some(it);
            }
            if let Some(rp) = remote_port {
                env.remote_port = rp;
            }
            if let Some(lp) = local_port {
                env.local_port = lp;
            }
            if let Some(rh) = rdp_host {
                env.rdp_host = rh;
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
                env.optimize = optimize_connection.map(|b| {
                    if b {
                        "speed".to_string()
                    } else {
                        "none".to_string()
                    }
                });
            }
            if optimize_connection.is_some() {
                env.optimize_connection = optimize_connection;
            }

            let spinner = Spinner::new(format!("Updating `{name}` in `{config}`…"));
            loader.save(&name, &env)?;
            spinner.success(format!("Environment `{name}` updated successfully."));
        }
        cli::ConfigCommand::Remove { name, config } => {
            print_section(&format!("Removing Environment: {name}"));
            let loader = ConfigLoader::new(&config);

            let spinner = Spinner::new(format!("Deleting `{name}` from `{config}`…"));
            loader.delete(&name)?;
            let _ = crate::credentials::CredentialStore::delete_secrets(&name);
            spinner.success(format!("Environment `{name}` removed."));
        }
        cli::ConfigCommand::Secure { name, config } => {
            print_section(&format!("Securing Environment: {name}"));
            let loader = ConfigLoader::new(&config);
            let mut env = loader.load(&name)?;
            let mut changed = false;

            if let Some(aws_secret) = env.aws_secret_access_key.take() {
                let spinner = Spinner::new(format!("Moving AWS secret key for `{name}` to keyring…"));
                crate::credentials::CredentialStore::set_aws_secret_key(&name, &aws_secret)?;
                spinner.success("AWS secret key secured.");
                changed = true;
            }

            if let Some(secret) = env.secret.take() {
                let spinner = Spinner::new(format!("Moving TOTP secret for `{name}` to keyring…"));
                crate::credentials::CredentialStore::set_totp_secret(&name, &secret)?;
                spinner.success("TOTP secret secured.");
                changed = true;
            }

            if let Some(pass) = env.rdp_password.take() {
                let spinner = Spinner::new(format!("Moving RDP password for `{name}` to keyring…"));
                crate::credentials::CredentialStore::set_rdp_password(&name, &pass)?;
                spinner.success("RDP password secured.");
                changed = true;
            }

            if changed {
                let spinner = Spinner::new(format!("Updating `{config}`…"));
                loader.save(&name, &env)?;
                spinner.success("Config file updated (secrets removed).");
            } else {
                print_warning("No plain-text secrets found to secure.");
            }
        }
        cli::ConfigCommand::SetAwsSecret { name, secret } => {
            print_section(&format!("Set AWS Secret Key: {name}"));
            let key = if let Some(s) = secret {
                s
            } else {
                inquire::Password::new("Enter AWS Secret Access Key:")
                    .without_confirmation()
                    .prompt()
                    .map_err(|e| crate::error::AppError::Config(e.to_string()))?
            };
            crate::credentials::CredentialStore::set_aws_secret_key(&name, &key)?;
            println!("{} AWS secret access key saved to keyring for `{name}`.", "✔".green());
        }
    }
    Ok(())
}

// ─── shell & export ──────────────────────────────────────────────────────────

fn cmd_register() -> Result<()> {
    print_section("Registering Shell Association");
    let spinner = Spinner::new("Registering .mfz file association in Windows Shell…");
    shell::register_file_association()?;
    spinner.success("Successfully registered .mfz file extension in HKCU\\Software\\Classes.");
    println!();
    println!(
        "    {} Double-clicking any .mfz file will now launch MFZ automatically.",
        "✓".green().bold()
    );
    println!(
        "    {} You can export desktop shortcuts with: {}",
        "TIP:".cyan().bold(),
        "mfz export --all --desktop".yellow()
    );
    Ok(())
}

fn cmd_unregister() -> Result<()> {
    print_section("Unregistering Shell Association");
    let spinner = Spinner::new("Removing .mfz file association from Windows Shell…");
    shell::unregister_file_association()?;
    spinner.success("Successfully unregistered .mfz file extension.");
    Ok(())
}

async fn cmd_export(args: cli::ExportArgs) -> Result<()> {
    print_section("Exporting MFZ Profiles");
    let loader = ConfigLoader::new(&args.config);

    // Determine target directory or file path
    let dest: PathBuf = if args.desktop {
        shell::get_desktop_dir().ok_or_else(|| {
            crate::error::AppError::Shell("Could not locate Desktop directory".to_string())
        })?
    } else if let Some(ref out) = args.out {
        PathBuf::from(out)
    } else {
        std::env::current_dir()?
    };

    let all_envs = loader.get_all()?;
    if all_envs.is_empty() {
        return Err(crate::error::AppError::Config(format!(
            "No environments found in config file `{}`",
            args.config
        )));
    }

    let env_names: Vec<String> = if args.all {
        let mut names: Vec<String> = all_envs.keys().cloned().collect();
        names.sort();
        names
    } else if let Some(ref env) = args.environment {
        vec![env.clone()]
    } else {
        let mut available: Vec<String> = all_envs.keys().cloned().collect();
        available.sort();
        let chosen = Select::new("Select environment to export:", available)
            .prompt()
            .map_err(|e| crate::error::AppError::Config(format!("Selection failed: {e}")))?;
        vec![chosen]
    };

    let standalone = !args.pointer;

    for name in &env_names {
        let env_cfg = all_envs.get(name);
        if env_cfg.is_none() && standalone {
            return Err(crate::error::AppError::Config(format!(
                "Environment `{name}` not found in `{}`",
                args.config
            )));
        }

        let spinner = Spinner::new(format!("Exporting profile `{name}`…"));
        let exported_path = shell::export_mfz_file(name, env_cfg, standalone, &dest)?;
        spinner.success(format!("Exported: {}", exported_path.display()));
    }

    println!();
    print_success(&format!(
        "Successfully exported {} profile(s).",
        env_names.len()
    ));

    Ok(())
}

// ─── launch ───────────────────────────────────────────────────────────────────

async fn cmd_launch(args: cli::LaunchArgs) -> Result<()> {
    let loader = ConfigLoader::new(&args.config);

    let (env_name, env) = match args.environment {
        Some(name) => loader.load_from_file_or_name(&name)?,
        None => {
            let envs = loader.list_environments()?;
            if envs.is_empty() {
                return Err(crate::error::AppError::Config(
                    "No environments found in config file".into(),
                ));
            }

            let chosen = Select::new("Select environment to launch:", envs)
                .prompt()
                .map_err(|e| crate::error::AppError::Config(format!("Selection failed: {e}")))?;
            let env = loader.load(&chosen)?;
            (chosen, env)
        }
    };

    print_section("Configuration");

    print_kv("Environment", &env_name);
    if let Some(ref inst) = env.instance {
        print_kv("Instance (Fixed)", inst);
    }
    if let Some(ref tag) = env.instance_tag {
        print_kv("Instance Tag", tag);
    }
    if let Some(ref key_id) = env.aws_access_key_id {
        print_kv("AWS Auth", &format!("Direct Key ({})", key_id));
    } else if let Some(ref p) = env.profile {
        print_kv("AWS Profile", p);
    }
    let resolved_region = args
        .region
        .clone()
        .or_else(|| env.region.clone())
        .unwrap_or_else(|| "ap-northeast-1".to_string());
    print_kv("Region", &resolved_region);
    print_kv("Remote port", &env.remote_port.to_string());
    print_kv("Local port", &env.local_port.to_string());
    print_kv("RDP host", &env.rdp_host);

    let resolved_folder = args.share_folder.as_ref().or(env.shared_folder.as_ref());
    let shared_drives = args
        .share_drives
        .as_deref()
        .or(env.shared_drives.as_deref());

    let fullscreen = if args.windowed {
        false
    } else {
        env.fullscreen.unwrap_or(true)
    };

    let opt_profile_str = args.optimize.as_deref().or(env.optimize.as_deref());

    let mut opt_settings = rdp::OptimizationSettings::from_profile_name(opt_profile_str);
    if env.opt_audio.is_some() {
        opt_settings.audio = env.opt_audio;
    }
    if env.opt_wallpaper.is_some() {
        opt_settings.wallpaper = env.opt_wallpaper;
    }
    if env.opt_font_smoothing.is_some() {
        opt_settings.font_smoothing = env.opt_font_smoothing;
    }

    if let Some(folder) = resolved_folder {
        print_kv("Shared folder", folder);
    }
    print_kv(
        "Shared drives",
        match (shared_drives, resolved_folder) {
            (Some(d), _) => d,
            (None, Some(_)) => "Virtual drive only",
            (None, None) => "none",
        },
    );
    print_kv(
        "Optimization",
        match opt_settings.profile {
            rdp::OptimizationProfile::None => "Disabled (Windows defaults)",
            rdp::OptimizationProfile::Speed => "Speed (LAN, compression, caching, no audio)",
            rdp::OptimizationProfile::Balanced => "Balanced (caching, audio enabled, wallpaper)",
            rdp::OptimizationProfile::Quality => "Quality (full desktop effects)",
        },
    );

    print_kv(
        "Mode",
        if args.ssm_only {
            "SSM tunnel only"
        } else if fullscreen {
            "RDP fullscreen"
        } else {
            "RDP windowed (dynamic resolution)"
        },
    );

    // Resolve Secret & Password (falling back to keyring if omitted from config)
    let totp_secret = env.resolve_secret(&env_name)?;
    let rdp_password = env.resolve_rdp_password(&env_name)?;

    let auth_source = if let Some(ref key_id) = env.aws_access_key_id {
        let secret_key = env.resolve_aws_secret_key(&env_name)?;
        let secret_key = secret_key.ok_or_else(|| {
            error::AppError::Config(format!(
                "Missing AWS secret access key for environment `{env_name}`"
            ))
        })?;
        mfa::AwsAuthSource::Direct {
            access_key_id: key_id.clone(),
            secret_access_key: secret_key,
        }
    } else if let Some(ref p) = env.profile {
        mfa::AwsAuthSource::Profile(p.clone())
    } else {
        return Err(error::AppError::Config(format!(
            "Environment `{env_name}` must configure either `aws_access_key_id` or `profile`"
        )));
    };

    let resolved_region = args
        .region
        .clone()
        .or_else(|| env.region.clone())
        .unwrap_or_else(|| "ap-northeast-1".to_string());

    print_section("MFA/SSO Authentication");
    let mut mfa = MfaSessionManager::new(
        auth_source,
        Some(resolved_region),
        env.serial.clone(),
        totp_secret,
    );

    // Resolve instance ID if needed
    let instance_id = if let Some(id) = env.instance {
        id
    } else if let Some(tag) = env.instance_tag {
        print_section("Instance Resolution");
        let spinner = Spinner::new(format!("Resolving tag `{}`…", tag));
        // Split tag Key=Value
        let parts: Vec<&str> = tag.split('=').collect();
        if parts.len() != 2 {
            spinner.failure("Invalid tag format — must be Key=Value");
            return Err(crate::error::AppError::Config("Invalid tag format".into()));
        }
        let id = match aws::resolve_instance_by_tag(
            &mut mfa,
            parts[0],
            parts[1],
            args.region.as_deref(),
            Some(&spinner),
        )
        .await
        {
            Ok(id) => {
                spinner.success(format!("Resolved to {}", id));
                id
            }
            Err(e) => {
                spinner.failure(e.to_string());
                return Err(e);
            }
        };
        id
    } else {
        return Err(crate::error::AppError::Config(
            "No instance ID or tag".into(),
        ));
    };

    let spinner = Spinner::new("Acquiring MFA session token…");
    let creds = match mfa.get_credentials(Some(&spinner)).await {
        Ok(c) => {
            spinner.success(format!(
                "Session valid until {} UTC",
                c.expiration.format("%H:%M:%S")
            ));
            c
        }
        Err(e) => {
            spinner.failure(e.to_string());
            return Err(e);
        }
    };
    info!("MFA session acquired (expires {})", creds.expiration);

    print_section("SSM Port Forwarding");
    let mut ssm = SsmSession::new(
        instance_id,
        env.remote_port,
        env.local_port,
        args.region.clone(),
    );

    // Verify hosts file contains rdp_host before connecting
    let hosts_mgr = HostsManager::new();
    if !hosts_mgr.has_host(&env.rdp_host).unwrap_or(false) {
        print_warning(&format!(
            "Host `{}` not found in hosts file — attempting to add…",
            env.rdp_host
        ));
        let _ = hosts_mgr.ensure_host("127.0.0.1", &env.rdp_host);
    }

    start_with_retries(&mut ssm, &mut mfa).await?;

    if args.ssm_only {
        print_separator();
        print_success(&format!(
            "Tunnel active on 127.0.0.1:{} — press Ctrl+C to stop",
            env.local_port
        ));
        println!();
        let _ = tokio::signal::ctrl_c().await;
        println!(
            "\n\n  {} Interrupt received — shutting down tunnel…",
            "⚡".yellow().bold()
        );
        ssm.stop();
        return Ok(());
    }

    print_section("Remote Desktop");
    let spinner = Spinner::new(format!(
        "Stabilizing SSM tunnel for {}:{}…",
        env.rdp_host, env.local_port
    ));
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    spinner.success(format!(
        "Launching mstsc → {}:{}…",
        env.rdp_host, env.local_port
    ));
    let shared_folder_path = resolved_folder.map(std::path::Path::new);
    rdp::launch(rdp::LaunchOptions {
        host: &env.rdp_host,
        port: env.local_port,
        fullscreen,
        username: env.rdp_user.as_deref(),
        password: rdp_password.as_deref(),
        shared_drives,
        shared_folder: shared_folder_path,
        optimization: opt_settings,
    })?;
    print_info("Remote Desktop session ended");
    ssm.stop();

    Ok(())
}

// ─── list ─────────────────────────────────────────────────────────────────────

fn cmd_list(args: cli::ListArgs) -> Result<()> {
    print_section("Available Environments");
    let envs = ConfigLoader::new(&args.config).list_environments()?;

    if envs.is_empty() {
        print_warning("No environments found — check your config file path");
    } else {
        for name in &envs {
            println!("    {} {}", "▸".cyan(), name.white().bold());
        }
        println!();
        print_info(&format!(
            "Run: mfz launch {} [--fullscreen] [--ssm-only]",
            envs.first().map(String::as_str).unwrap_or("<env>")
        ));
    }
    Ok(())
}

// ─── hosts ────────────────────────────────────────────────────────────────────

fn cmd_hosts(sub: HostsCommand) -> Result<()> {
    print_section("Hosts File Manager");
    let mgr = HostsManager::new();

    match sub {
        HostsCommand::Add { config } => {
            let loader = ConfigLoader::new(&config);
            let envs = loader.get_all()?;

            if envs.is_empty() {
                print_warning(&format!("No environments found in `{config}`."));
                return Ok(());
            }

            // Extract unique rdp_hosts mapped to 127.0.0.1
            let mut unique_hosts: Vec<String> = Vec::new();
            for (_, env) in envs {
                if !env.rdp_host.trim().is_empty() && !unique_hosts.contains(&env.rdp_host) {
                    unique_hosts.push(env.rdp_host);
                }
            }

            if unique_hosts.is_empty() {
                print_warning(&format!("No RDP hosts configured in `{config}`."));
                return Ok(());
            }

            let entries: Vec<(&str, &str)> = unique_hosts
                .iter()
                .map(|h| ("127.0.0.1", h.as_str()))
                .collect();

            let spinner = Spinner::new(format!(
                "Updating hosts for {} environment(s)…",
                entries.len()
            ));
            mgr.ensure_hosts(&entries, Some(&config))?;
            spinner.success("Hosts file updated");
        }
        HostsCommand::Custom { ip, hostname } => {
            let spinner = Spinner::new(format!("Adding `{hostname}` → `{ip}`…"));
            mgr.ensure_host(&ip, &hostname)?;
            spinner.success("Custom host entry processed");
        }
    }
    Ok(())
}

// ─── qr ───────────────────────────────────────────────────────────────────────

async fn cmd_qr(sub: QrCommand) -> Result<()> {
    match sub {
        // ── decode ────────────────────────────────────────────────────────
        QrCommand::Decode { image } => {
            print_section("QR Code Decoder");
            let spinner = Spinner::new(format!("Scanning `{image}`…"));
            let secret = match qr::decode(&image) {
                Ok(s) => {
                    spinner.success("Decoded successfully");
                    s
                }
                Err(e) => {
                    spinner.failure(e.to_string());
                    return Err(e);
                }
            };
            println!();
            print_kv("OTP URI", &secret.uri);
            println!();
            println!(
                "  {}  {}",
                "SECRET".cyan().bold(),
                secret.secret.green().bold()
            );
            println!();
        }

        // ── generate ──────────────────────────────────────────────────────
        QrCommand::Generate(args) => {
            print_section("QR Code Generator");

            let spinner = Spinner::new("Generating TOTP secret and QR code…");
            let generated = match qr::generate(args.secret.as_deref(), &args.account, &args.issuer)
            {
                Ok(g) => {
                    spinner.success("QR code generated");
                    g
                }
                Err(e) => {
                    spinner.failure(e.to_string());
                    return Err(e);
                }
            };

            // Always show inline terminal QR
            qr::print_terminal(&generated);

            // Print key details
            print_kv("Account", &args.account);
            print_kv("Issuer", &args.issuer);
            println!();
            println!(
                "  {}  {}",
                "SECRET (Base-32)".cyan().bold(),
                generated.secret_b32.green().bold()
            );
            println!();
            print_kv("OTP URI", &generated.uri);
            println!();

            // Optionally save PNG
            if let Some(ref path) = args.out {
                qr::save_png(&generated, path)?;
            } else {
                print_info("Tip: pass --out totp.png to also save a scannable PNG image");
            }
        }

        // ── register ──────────────────────────────────────────────────────
        QrCommand::Register(args) => {
            let account = args
                .account
                .clone()
                .unwrap_or_else(|| args.username.clone());

            qr::register(
                &args.profile,
                &args.username,
                &account,
                &args.issuer,
                args.out.as_deref(),
                args.region.as_deref(),
                args.save.as_deref(),
            )
            .await?;
        }
    }

    Ok(())
}

#[cfg(windows)]
fn setup_console_font() {
    use windows_sys::Win32::System::Console::{
        GetCurrentConsoleFontEx, GetStdHandle, SetCurrentConsoleFontEx, CONSOLE_FONT_INFOEX, COORD,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle == 0 || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return;
        }

        let mut font_info = CONSOLE_FONT_INFOEX {
            cbSize: std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32,
            nFont: 0,
            dwFontSize: COORD { X: 0, Y: 18 },
            FontFamily: 54, // FF_MODERN
            FontWeight: 400,
            FaceName: [0; 32],
        };

        // Query current font settings (optional pre-flight check)
        let mut current_font = CONSOLE_FONT_INFOEX {
            cbSize: std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32,
            nFont: 0,
            dwFontSize: COORD { X: 0, Y: 0 },
            FontFamily: 0,
            FontWeight: 0,
            FaceName: [0; 32],
        };

        let _ = GetCurrentConsoleFontEx(handle, 0, &mut current_font);

        let font_name = "Consolas";
        for (i, c) in font_name.encode_utf16().enumerate() {
            if i < 31 {
                font_info.FaceName[i] = c;
            }
        }

        SetCurrentConsoleFontEx(handle, 0, &font_info);
    }
}
