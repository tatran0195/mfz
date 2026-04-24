mod cli;
mod config;
mod error;
mod hosts;
mod mfa;
mod qr;
mod rdp;
mod ssm;
mod ui;

use clap::Parser;
use cli::{Cli, HostsCommand, QrCommand, TopCommand};
use colored::Colorize;
use config::ConfigLoader;
use error::Result;
use hosts::HostsManager;
use mfa::MfaSessionManager;
use ssm::{start_with_retries, SsmSession};
use tracing::info;
use tracing_appender::rolling;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
use ui::*;

// ─── Logging ──────────────────────────────────────────────────────────────────

fn setup_logging() {
    let file_appender = rolling::daily(".", "mfz.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(false),
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_target(false)
                .with_filter(EnvFilter::new("rdp_launcher=debug,warn")),
        )
        .init();

    std::mem::forget(_guard); // keep flushing until process exits
}

// ─── Entry ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    setup_logging();
    print_banner();

    let cli = Cli::parse();

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
                    println!("      {:<12} {}", "Instance:".dimmed(), env.instance);
                    println!("      {:<12} {}", "Profile:".dimmed(), env.profile);
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
            remote_port,
            local_port,
            rdp_host,
            config,
        } => {
            print_section(&format!("Adding Environment: {name}"));
            let loader = ConfigLoader::new(&config);
            let new_env = crate::config::EnvironmentConfig {
                profile,
                serial,
                secret,
                instance,
                remote_port,
                local_port,
                rdp_host,
            };

            let spinner = Spinner::new(format!("Saving `{name}` to `{config}`…"));
            loader.save(&name, &new_env)?;
            spinner.success(format!("Environment `{name}` added successfully."));
        }
        cli::ConfigCommand::Edit {
            name,
            profile,
            serial,
            secret,
            instance,
            remote_port,
            local_port,
            rdp_host,
            config,
        } => {
            print_section(&format!("Editing Environment: {name}"));
            let loader = ConfigLoader::new(&config);
            let mut env = loader.load(&name)?;

            if let Some(p) = profile {
                env.profile = p;
            }
            if let Some(s) = serial {
                env.serial = s;
            }
            if let Some(s) = secret {
                env.secret = s;
            }
            if let Some(i) = instance {
                env.instance = i;
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

            let spinner = Spinner::new(format!("Updating `{name}` in `{config}`…"));
            loader.save(&name, &env)?;
            spinner.success(format!("Environment `{name}` updated successfully."));
        }
        cli::ConfigCommand::Remove { name, config } => {
            print_section(&format!("Removing Environment: {name}"));
            let loader = ConfigLoader::new(&config);

            let spinner = Spinner::new(format!("Deleting `{name}` from `{config}`…"));
            loader.delete(&name)?;
            spinner.success(format!("Environment `{name}` removed."));
        }
    }
    Ok(())
}

// ─── launch ───────────────────────────────────────────────────────────────────

async fn cmd_launch(args: cli::LaunchArgs) -> Result<()> {
    print_section("Configuration");
    let env = ConfigLoader::new(&args.config).load(&args.environment)?;

    print_kv("Environment", &args.environment);
    print_kv("Instance", &env.instance);
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

    print_section("MFA Authentication");
    let mut mfa =
        MfaSessionManager::new(env.profile.clone(), env.serial.clone(), env.secret.clone());
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
        &env.instance,
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
    rdp::launch(&env.rdp_host, env.local_port, args.fullscreen)?;
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
        HostsCommand::Add => {
            let spinner = Spinner::new("Adding default RDP host entries…");
            mgr.add_hosts(&[("127.0.0.1", "stg-rdp"), ("127.0.0.1", "prd-rdp")])?;
            spinner.success("Hosts file updated");
        }
        HostsCommand::Custom { ip, hostname } => {
            let spinner = Spinner::new(format!("Adding `{hostname}` → `{ip}`…"));
            mgr.ensure_writable()?;
            mgr.add_host(&ip, &hostname)?;
            spinner.success("Custom host entry added");
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
