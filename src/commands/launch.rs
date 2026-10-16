use crate::{
    auth::mfa::{self, MfaSessionManager},
    aws,
    cli::LaunchArgs,
    config::ConfigLoader,
    error::{self, Result},
    rdp,
    transport::ssm::{self, SsmSession},
    ui::*,
};
use colored::Colorize;
use inquire::Select;
use tracing::info;

pub async fn run(args: LaunchArgs) -> Result<()> {
    if !cfg!(windows) && !args.ssm_only {
        return Err(error::AppError::Rdp("Desktop launch requires Windows; use --ssm-only for the native tunnel on this platform".into()));
    }
    let loader = ConfigLoader::from_cli(args.config.as_deref());

    let (env_name, mut env, trusted_profile, config_source) = match args.environment {
        Some(name) => {
            let path = std::path::Path::new(&name);
            let is_file = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mfz"))
                || name.contains(['/', '\\', ':'])
                || path.is_file();
            if is_file && !args.trust_profile {
                if !console::user_attended() {
                    return Err(error::AppError::Config("External profile requires prior review; use --trust-profile only after independently verifying the file and any configuration it references".into()));
                }
                let accepted = inquire::Confirm::new(&format!("Allow reading `{name}` and any config paths it references (which may include network shares)?"))
                    .with_default(false).prompt().map_err(|_| error::AppError::Config("External profile review cancelled".into()))?;
                if !accepted {
                    return Err(error::AppError::Config("External profile not trusted; no referenced config read or credential lookup performed".into()));
                }
            }
            let loaded = loader.load_for_launch(&name)?;
            (
                loaded.name,
                loaded.config,
                loaded.allow_keyring && !is_file,
                loaded.source,
            )
        }
        None => {
            let envs = loader.list_environments()?;
            if envs.is_empty() {
                return Err(error::AppError::Config(
                    "No environments found in config file".into(),
                ));
            }
            let chosen = Select::new("Select environment to launch:", envs)
                .prompt()
                .map_err(|error| error::AppError::Config(format!("Selection failed: {error}")))?;
            let env = loader.load(&chosen)?;
            (chosen, env, true, loader.path().to_owned())
        }
    };
    if let Some(region) = &args.region {
        env.region = Some(region.clone());
    }
    if let Some(drives) = &args.share_drives {
        env.shared_drives = Some(drives.clone());
    }
    if let Some(folder) = &args.share_folder {
        env.shared_folder = Some(folder.clone());
    }
    if let Some(optimize) = &args.optimize {
        env.optimize = Some(optimize.clone());
    }
    if args.windowed {
        env.fullscreen = Some(false);
    }
    env.validate()?;
    let resolved_region = env.region.clone();

    print_section("Configuration");
    print_kv("Environment", &env_name);
    print_kv("Source", &format!("{config_source:?}"));
    if let Some(ref instance) = env.instance {
        print_kv("Instance (Fixed)", instance);
    }
    if let Some(ref tag) = env.instance_tag {
        print_kv("Instance Tag", tag);
    }
    if let Some(ref key_id) = env.aws_access_key_id {
        print_kv("AWS Auth", &format!("Direct Key ({})", key_id));
    } else if let Some(ref profile) = env.profile {
        print_kv("AWS Profile", profile);
    }
    print_kv(
        "Region",
        resolved_region
            .as_deref()
            .unwrap_or("Selected AWS profile/provider chain"),
    );
    print_kv("Remote port", &env.remote_port.to_string());
    print_kv("Local port", &env.local_port.to_string());
    print_kv("RDP host", &env.rdp_host);

    let resolved_folder = env.shared_folder.as_ref();
    let shared_drives = env.shared_drives.as_deref();
    let fullscreen = env.fullscreen.unwrap_or(true);
    let mut opt_settings = rdp::OptimizationSettings::from_profile_name(env.optimize.as_deref())?;
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
            (Some(drives), _) => drives,
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

    if !trusted_profile && !args.trust_profile {
        if !console::user_attended() {
            return Err(error::AppError::Config("External profile requires review/confirmation; use --trust-profile only after independently verifying it".into()));
        }
        let accepted = inquire::Confirm::new("Allow this external profile to use the displayed AWS target, local credentials and shared resources?")
            .with_default(false).prompt().map_err(|error| error::AppError::Config(format!("Profile confirmation failed: {error}")))?;
        if !accepted {
            return Err(error::AppError::Config(
                "External profile was not trusted; no credential lookup or connection performed"
                    .into(),
            ));
        }
    }
    if !args.ssm_only {
        rdp::LaunchOptions {
            host: "127.0.0.1",
            port: env.local_port,
            fullscreen,
            username: env.rdp_user.as_deref(),
            password: env.rdp_password.as_deref(),
            shared_drives,
            shared_folder: resolved_folder.map(std::path::Path::new),
            optimization: opt_settings.clone(),
        }
        .validate()?;
    }
    let totp_secret = if env.serial.is_some() {
        env.resolve_secret(&env_name)?
    } else {
        None
    };
    let rdp_password = if args.ssm_only || env.rdp_user.is_none() {
        None
    } else {
        env.resolve_rdp_password(&env_name)?
    };

    if !args.ssm_only {
        rdp::LaunchOptions {
            host: "127.0.0.1",
            port: env.local_port,
            fullscreen,
            username: env.rdp_user.as_deref(),
            password: rdp_password.as_deref(),
            shared_drives,
            shared_folder: resolved_folder.map(std::path::Path::new),
            optimization: opt_settings.clone(),
        }
        .validate_credentials()?;
    }
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
    } else if let Some(ref profile) = env.profile {
        mfa::AwsAuthSource::Profile(profile.clone())
    } else {
        return Err(error::AppError::Config(format!(
            "Environment `{env_name}` must configure either `aws_access_key_id` or `profile`"
        )));
    };

    print_section("MFA/SSO Authentication");
    let mut mfa = MfaSessionManager::new(
        auth_source,
        resolved_region.clone(),
        env.serial.clone(),
        totp_secret,
    );

    let instance_id = if let Some(instance_id) = env.instance {
        instance_id
    } else if let Some(tag) = env.instance_tag {
        print_section("Instance Resolution");
        let spinner = Spinner::new(format!("Resolving tag `{}`…", tag));
        let (tag_key, tag_value) = tag
            .split_once('=')
            .ok_or_else(|| error::AppError::Config("Invalid tag format; use Key=Value".into()))?;
        let instance_id = match aws::resolve_instance_by_tag(
            &mut mfa,
            tag_key,
            tag_value,
            resolved_region.as_deref(),
            Some(&spinner),
        )
        .await
        {
            Ok(instance_id) => {
                spinner.success(format!("Resolved to {}", instance_id));
                instance_id
            }
            Err(error) => {
                spinner.failure(error.to_string());
                return Err(error);
            }
        };
        instance_id
    } else {
        return Err(error::AppError::Config("No instance ID or tag".into()));
    };

    let spinner = Spinner::new("Acquiring MFA session token…");
    let credentials = match mfa.get_credentials(Some(&spinner)).await {
        Ok(credentials) => {
            spinner.success(format!(
                "Credentials cached until {} UTC",
                credentials.expiration.format("%H:%M:%S")
            ));
            credentials
        }
        Err(error) => {
            spinner.failure(error.to_string());
            return Err(error);
        }
    };
    info!(
        "AWS credentials acquired (cache refresh due {})",
        credentials.expiration
    );

    print_section("SSM Port Forwarding");
    let mut ssm = SsmSession::new(
        instance_id,
        env.remote_port,
        env.local_port,
        resolved_region.clone(),
    );
    ssm::start_with_retries(&mut ssm, &mut mfa).await?;

    if args.ssm_only {
        print_separator();
        print_success(&format!(
            "Tunnel active on 127.0.0.1:{} — press Ctrl+C to stop",
            env.local_port
        ));
        println!();
        let end = tokio::select! {
            signal = tokio::signal::ctrl_c() => signal.map_err(error::AppError::Io),
            _ = ssm.closed() => Err(error::AppError::Ssm("Native SSM transport exited unexpectedly".into())),
        };
        println!(
            "\n\n  {} Shutting down native tunnel…",
            "⚡".yellow().bold()
        );
        let cleanup = ssm.shutdown().await;
        end?;
        cleanup?;
        return Ok(());
    }

    print_section("Remote Desktop");
    let spinner = Spinner::new(format!(
        "Launching RDP through authenticated native tunnel 127.0.0.1:{}…",
        env.local_port
    ));
    spinner.success(format!("Launching mstsc → 127.0.0.1:{}…", env.local_port));
    let rdp_options = rdp::LaunchOptions {
        host: "127.0.0.1",
        port: env.local_port,
        fullscreen,
        username: env.rdp_user.as_deref(),
        password: rdp_password.as_deref(),
        shared_drives,
        shared_folder: resolved_folder.map(std::path::Path::new),
        optimization: opt_settings,
    };
    let rdp_result = tokio::select! {
        result = rdp::launch(rdp_options) => result,
        result = tokio::signal::ctrl_c() => result.map_err(error::AppError::Io),
        _ = ssm.closed() => Err(error::AppError::Ssm("Native transport ended while Remote Desktop was active".into())),
    };
    let cleanup_result = ssm.shutdown().await;
    rdp_result?;
    print_info("Remote Desktop session ended");
    cleanup_result?;

    Ok(())
}
