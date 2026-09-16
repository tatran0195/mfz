use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use crate::{
    error::{AppError, Result},
    ui::{print_info, print_success, print_warning},
};

#[cfg(target_os = "windows")]
const HOSTS_PATH: &str = r"C:\Windows\System32\drivers\etc\hosts";
#[cfg(not(target_os = "windows"))]
const HOSTS_PATH: &str = "/etc/hosts";

pub struct HostsManager {
    path: PathBuf,
}

impl HostsManager {
    pub fn new() -> Self {
        Self {
            path: PathBuf::from(HOSTS_PATH),
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn is_writable(&self) -> bool {
        OpenOptions::new().append(true).open(&self.path).is_ok()
    }

    pub fn ensure_writable(&self) -> Result<()> {
        if self.is_writable() {
            Ok(())
        } else {
            Err(AppError::Hosts(format!(
                "Cannot write to {} — run as administrator / sudo",
                self.path.display()
            )))
        }
    }

    pub fn has_host(&self, hostname: &str) -> Result<bool> {
        let content = fs::read_to_string(&self.path)
            .map_err(|e| AppError::Hosts(format!("Cannot read hosts file: {e}")))?;

        let exists = content
            .lines()
            .filter(|l| !l.trim().starts_with('#'))
            .filter_map(|l| {
                let cols: Vec<&str> = l.split_whitespace().collect();
                (cols.len() >= 2).then_some(cols)
            })
            .any(|cols| cols[1..].contains(&hostname));

        Ok(exists)
    }

    pub fn elevate_and_run(&self, args: &[&str]) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            let exe = std::env::current_exe().map_err(|e| {
                AppError::Hosts(format!("Failed to determine executable path: {e}"))
            })?;

            let formatted_args = args
                .iter()
                .map(|a| format!("'{}'", a.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");

            let ps_cmd = format!(
                "Start-Process -FilePath '{}' -ArgumentList @({}) -Verb RunAs -Wait -PassThru | ForEach-Object {{ exit $_.ExitCode }}",
                exe.to_string_lossy().replace('\'', "''"),
                formatted_args
            );

            let status = std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
                .status()
                .map_err(|e| AppError::Hosts(format!("Failed to execute elevated process: {e}")))?;

            if status.success() {
                Ok(())
            } else {
                Err(AppError::Hosts(format!(
                    "Elevated operation failed or was cancelled (exit code: {})",
                    status.code().unwrap_or(-1)
                )))
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = args;
            Err(AppError::Hosts(format!(
                "Cannot write to {} — please run with sudo",
                self.path.display()
            )))
        }
    }

    pub fn add_host(&self, ip: &str, hostname: &str) -> Result<()> {
        if self.has_host(hostname)? {
            print_warning(&format!("`{hostname}` already present — skipping"));
            return Ok(());
        }

        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|e| AppError::Hosts(format!("Cannot open hosts file: {e}")))?;

        writeln!(file, "\n{ip}\t{hostname}")
            .map_err(|e| AppError::Hosts(format!("Write failed: {e}")))?;

        print_success(&format!("Added: {ip}  →  {hostname}"));
        Ok(())
    }

    pub fn add_hosts(&self, entries: &[(&str, &str)]) -> Result<()> {
        self.ensure_writable()?;
        for &(ip, hostname) in entries {
            self.add_host(ip, hostname)?;
        }
        Ok(())
    }

    pub fn ensure_host(&self, ip: &str, hostname: &str) -> Result<()> {
        if self.has_host(hostname)? {
            print_warning(&format!(
                "`{hostname}` already present in hosts file — skipping"
            ));
            return Ok(());
        }

        if self.is_writable() {
            self.add_host(ip, hostname)
        } else {
            print_info(&format!(
                "Administrator privileges required to update hosts for `{hostname}`. Requesting elevation…"
            ));
            self.elevate_and_run(&["hosts", "custom", ip, hostname])
        }
    }

    pub fn ensure_hosts(&self, entries: &[(&str, &str)], config_path: Option<&str>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let missing: Vec<_> = entries
            .iter()
            .copied()
            .filter(|&(_, hostname)| !self.has_host(hostname).unwrap_or(false))
            .collect();

        if missing.is_empty() {
            print_warning("All host entries are already present in hosts file — skipping");
            return Ok(());
        }

        if self.is_writable() {
            for &(ip, hostname) in &missing {
                self.add_host(ip, hostname)?;
            }
            Ok(())
        } else {
            print_info(
                "Administrator privileges required to update hosts file. Requesting elevation…",
            );
            let mut args = vec!["hosts", "add"];
            if let Some(cp) = config_path {
                args.push("--config");
                args.push(cp);
            }
            self.elevate_and_run(&args)
        }
    }
}

impl Default for HostsManager {
    fn default() -> Self {
        Self::new()
    }
}
