use crate::{
    error::{AppError, Result},
    ui::{print_success, print_warning},
};
use std::{fs::{self, OpenOptions}, io::Write};

#[cfg(target_os = "windows")]
const HOSTS_PATH: &str = r"C:\Windows\System32\drivers\etc\hosts";
#[cfg(not(target_os = "windows"))]
const HOSTS_PATH: &str = "/etc/hosts";

pub struct HostsManager { path: &'static str }

impl HostsManager {
    pub fn new() -> Self { Self { path: HOSTS_PATH } }

    pub fn ensure_writable(&self) -> Result<()> {
        OpenOptions::new().append(true).open(self.path).map(|_| ()).map_err(|_| {
            AppError::Hosts(format!(
                "Cannot write to {} — run as administrator / sudo", self.path
            ))
        })
    }

    pub fn add_host(&self, ip: &str, hostname: &str) -> Result<()> {
        let content = fs::read_to_string(self.path)
            .map_err(|e| AppError::Hosts(format!("Cannot read hosts file: {e}")))?;

        let exists = content
            .lines()
            .filter(|l| !l.trim().starts_with('#'))
            .filter_map(|l| {
                let cols: Vec<&str> = l.split_whitespace().collect();
                (cols.len() >= 2).then_some(cols)
            })
            .any(|cols| cols[1..].contains(&hostname));

        if exists {
            print_warning(&format!("`{hostname}` already present — skipping"));
            return Ok(());
        }

        let mut file = OpenOptions::new().append(true).open(self.path)
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
}

impl Default for HostsManager {
    fn default() -> Self { Self::new() }
}