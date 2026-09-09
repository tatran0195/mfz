use clap::Parser;
use cli::{Cli, HostsCommand, QrCommand, TopCommand};
use colored::Colorize;
use config::ConfigLoader;
use error::Result;
use hosts::HostsManager;
use inquire::Select;
use mfa::MfaSessionManager;
use mfz::{aws, cli, config, credentials, error, hosts, mfa, qr, rdp, ssm, ui};
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
        .with_filter(EnvFilter::new("rdp_launcher=debug,warn"))
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
        // Force UTF-8 code page for better symbol support in CMD
        let _ = std::process::Command::new("cmd")
            .args(&["/C", "chcp 65001"])
            .output();
        // Attempt to set a modern font (Consolas) for better symbol rendering
        setup_console_font();
    }

    let cli = Cli::parse();
    setup_logging(cli.log);
    print_banner();

    if let Err(e) = dispatch(cli).await {
        print_error(&e.to_string());
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
                    println!("      {:<12} {}", "Profile:".dimmed(), env.profile);
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
                    println!();
                }
            }
        }
        cli::ConfigCommand::Add {
            name,
            profile,
            serial,
            secret,
            instance,
            instance_tag,
            remote_port,
            local_port,
            rdp_host,
            user,
            pass,
            config,
        } => {
            print_section(&format!("Adding Environment: {name}"));
            let loader = ConfigLoader::new(&config);

            if instance.is_none() && instance_tag.is_none() {
                return Err(crate::error::AppError::Config(
                    "Must specify either `--instance` or `--instance-tag`".into(),
                ));
            }

            let auth_type = if serial.is_some() {
                crate::config::AuthType::Mfa
            } else {
                crate::config::AuthType::Sso
            };

            let new_env = crate::config::EnvironmentConfig {
                profile,
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
            serial,
            secret,
            instance,
            instance_tag,
            remote_port,
            local_port,
            rdp_host,
            user,
            pass,
            config,
        } => {
            print_section(&format!("Editing Environment: {name}"));
            let loader = ConfigLoader::new(&config);
            let mut env = loader.load(&name)?;

            if let Some(p) = profile {
                env.profile = p;
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
    }
    Ok(())
}

// ─── launch ───────────────────────────────────────────────────────────────────

async fn cmd_launch(args: cli::LaunchArgs) -> Result<()> {
    let loader = ConfigLoader::new(&args.config);

    let env_name = match args.environment {
        Some(name) => name,
        None => {
            let envs = loader.list_environments()?;
            if envs.is_empty() {
                return Err(crate::error::AppError::Config(
                    "No environments found in config file".into(),
                ));
            }

            Select::new("Select environment to launch:", envs)
                .prompt()
                .map_err(|e| crate::error::AppError::Config(format!("Selection failed: {e}")))?
        }
    };

    print_section("Configuration");
    let env = loader.load(&env_name)?;

    print_kv("Environment", &env_name);
    if let Some(ref inst) = env.instance {
        print_kv("Instance (Fixed)", inst);
    }
    if let Some(ref tag) = env.instance_tag {
        print_kv("Instance Tag", tag);
    }
    print_kv("Profile", &env.profile);
    print_kv("Remote port", &env.remote_port.to_string());
    print_kv("Local port", &env.local_port.to_string());
    print_kv("RDP host", &env.rdp_host);
    print_kv(
        "Mode",
        if args.ssm_only {
            "SSM tunnel only"
        } else if args.fullscreen {
            "RDP fullscreen"
        } else {
            "RDP windowed"
        },
    );

    // Resolve Secret & Password from Keyring if missing in config
    let totp_secret = if env.auth_type == crate::config::AuthType::Mfa {
        if let Some(s) = env.secret {
            Some(s)
        } else {
            Some(
                crate::credentials::CredentialStore::get_totp_secret(&env_name)?.ok_or_else(
                    || {
                        crate::error::AppError::Config(
                            "No MFA secret found in config or keyring".into(),
                        )
                    },
                )?,
            )
        }
    } else {
        None
    };

    let rdp_password = if env.rdp_password.is_some() {
        env.rdp_password
    } else {
        crate::credentials::CredentialStore::get_rdp_password(&env_name)?
    };

    print_section("MFA/SSO Authentication");
    let mut mfa = MfaSessionManager::new(
        env.profile.clone(),
        args.region.clone(),
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

    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            println!(
                "\n\n  {} Interrupt received — shutting down",
                "⚡".yellow().bold()
            );
            std::process::exit(0);
        }
    });

    start_with_retries(&mut ssm, &mut mfa).await?;

    if args.ssm_only {
        print_separator();
        print_success(&format!(
            "Tunnel active on 127.0.0.1:{} — press Ctrl+C to stop",
            env.local_port
        ));
        println!();
        ssm.wait()?;
        return Ok(());
    }

    print_section("Remote Desktop");
    let spinner = Spinner::new(format!(
        "Launching mstsc → {}:{}…",
        env.rdp_host, env.local_port
    ));
    rdp::launch(
        &env.rdp_host,
        env.local_port,
        args.fullscreen,
        env.rdp_user.as_deref(),
        rdp_password.as_deref(),
    )?;
    spinner.success("RDP session ended");

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
        if handle == 0 || handle == -1isize as _ {
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

        // Check current font to avoid redundant sets (optional but good practice)
        let mut current_font = CONSOLE_FONT_INFOEX {
            cbSize: std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32,
            nFont: 0,
            dwFontSize: COORD { X: 0, Y: 0 },
            FontFamily: 0,
            FontWeight: 0,
            FaceName: [0; 32],
        };

        if GetCurrentConsoleFontEx(handle, 0, &mut current_font) != 0 {
            // If already Consolas, we might skip, but setting it anyway ensures size
        }

        let font_name = "Consolas";
        for (i, c) in font_name.encode_utf16().enumerate() {
            if i < 31 {
                font_info.FaceName[i] = c;
            }
        }

        SetCurrentConsoleFontEx(handle, 0, &font_info);
    }
}
