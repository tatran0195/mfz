use crate::{cli::ListArgs, config::ConfigLoader, error::Result, ui::*};
use colored::Colorize;

pub fn run(args: ListArgs) -> Result<()> {
    print_section("Available Environments");
    let environments = ConfigLoader::from_cli(args.config.as_deref()).list_environments()?;

    if environments.is_empty() {
        print_warning("No environments found — check your config file path");
    } else {
        for name in &environments {
            println!("    {} {}", "▸".cyan(), name.white().bold());
        }
        println!();
        print_info(&format!(
            "Run: mfz launch {} [--windowed] [--ssm-only]",
            environments.first().map(String::as_str).unwrap_or("<env>")
        ));
    }
    Ok(())
}
