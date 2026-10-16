use crate::{
    cli::HostsCommand,
    config::ConfigLoader,
    error::{self, Result},
    sys::hosts::HostsManager,
    ui::*,
};

pub fn run(sub: HostsCommand) -> Result<()> {
    print_section("Hosts File Manager");
    let manager = HostsManager::new()?;

    match sub {
        HostsCommand::Add { config } => {
            let loader = ConfigLoader::from_cli(config.as_deref());
            let environments = loader.get_all()?;

            if environments.is_empty() {
                print_warning(&format!(
                    "No environments found in `{}`.",
                    loader.path().display()
                ));
                return Ok(());
            }

            let mut unique_hosts: Vec<String> = Vec::new();
            for (_, environment) in environments {
                if !environment.rdp_host.trim().is_empty()
                    && !unique_hosts.contains(&environment.rdp_host)
                {
                    unique_hosts.push(environment.rdp_host);
                }
            }

            if unique_hosts.is_empty() {
                print_warning(&format!(
                    "No RDP hosts configured in `{}`.",
                    loader.path().display()
                ));
                return Ok(());
            }

            let entries: Vec<(&str, &str)> = unique_hosts
                .iter()
                .map(|host| ("127.0.0.1", host.as_str()))
                .collect();
            let spinner = Spinner::new(format!(
                "Updating hosts for {} environment(s)…",
                entries.len()
            ));
            manager.ensure_hosts(&entries)?;
            spinner.success("Hosts file updated");
        }
        HostsCommand::Apply { entries } => {
            let parsed: Vec<_> = entries
                .iter()
                .map(|entry| {
                    entry.split_once('=').ok_or_else(|| {
                        error::AppError::Hosts("Invalid elevated hosts snapshot".into())
                    })
                })
                .collect::<Result<_>>()?;
            manager.add_hosts(&parsed)?;
        }
        HostsCommand::Custom { ip, hostname } => {
            let spinner = Spinner::new(format!("Adding `{hostname}` → `{ip}`…"));
            manager.ensure_host(&ip, &hostname)?;
            spinner.success("Custom host entry processed");
        }
    }
    Ok(())
}
