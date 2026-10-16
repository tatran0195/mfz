use std::path::PathBuf;

use crate::{
    cli::ExportArgs,
    config::ConfigLoader,
    error::{self, Result},
    sys::shell,
    ui::*,
};
use inquire::Select;

pub async fn run(args: ExportArgs) -> Result<()> {
    print_section("Exporting MFZ Profiles");
    let loader = ConfigLoader::from_cli(args.config.as_deref());

    let dest: PathBuf = if args.desktop {
        shell::get_desktop_dir()?
    } else if let Some(ref out) = args.out {
        PathBuf::from(out)
    } else {
        std::env::current_dir()?
    };

    let all_envs = loader.get_all()?;
    if all_envs.is_empty() {
        return Err(error::AppError::Config(format!(
            "No environments found in config file `{}`",
            loader.path().display()
        )));
    }

    let env_names: Vec<String> = if args.all {
        let mut names: Vec<String> = all_envs.keys().cloned().collect();
        names.sort();
        names
    } else if let Some(ref env) = args.environment {
        vec![env.to_lowercase()]
    } else {
        let mut available: Vec<String> = all_envs.keys().cloned().collect();
        available.sort();
        let chosen = Select::new("Select environment to export:", available)
            .prompt()
            .map_err(|error| error::AppError::Config(format!("Selection failed: {error}")))?;
        vec![chosen]
    };

    let standalone = args.standalone || !args.pointer;
    if env_names.len() > 1 && !dest.is_dir() && dest.extension().is_some() {
        return Err(error::AppError::Shell(
            "Multiple exports require a directory, not one output filename".into(),
        ));
    }
    if args.include_secrets {
        print_warning("Explicit plaintext-secret export: protect the destination directory/ACLs and never share or commit these files.");
    }
    for name in &env_names {
        if !all_envs.contains_key(name) {
            return Err(error::AppError::Config(format!(
                "Environment `{name}` not found"
            )));
        }
    }
    for name in &env_names {
        let mut env = all_envs
            .get(name)
            .cloned()
            .ok_or_else(|| error::AppError::Config("Export plan changed unexpectedly".into()))?;
        if args.include_secrets {
            env.aws_secret_access_key = env.resolve_aws_secret_key(name)?;
            if env.serial.is_some() {
                env.secret = env.resolve_secret(name)?;
            }
            if env.rdp_user.is_some() {
                env.rdp_password = env.resolve_rdp_password(name)?;
            }
        }
        let spinner = Spinner::new(format!("Exporting profile `{name}`…"));
        let exported_path = shell::export_mfz_file_with_options(
            name,
            Some(&env),
            standalone,
            &dest,
            &shell::ExportOptions {
                include_secrets: args.include_secrets,
                overwrite: args.force,
                config_path: Some(loader.path()),
            },
        )?;
        spinner.success(format!("Exported: {}", exported_path.display()));
    }

    println!();
    print_success(&format!(
        "Successfully exported {} profile(s).",
        env_names.len()
    ));

    Ok(())
}
