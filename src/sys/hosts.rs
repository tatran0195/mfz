use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::IpAddr,
    path::PathBuf,
};

use crate::{
    error::{AppError, Result},
    ui::{print_info, print_success},
    validation,
};

#[cfg(target_os = "windows")]
mod elevation {
    use std::{io, os::windows::ffi::OsStrExt, path::Path};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        UI::{
            Shell::{
                ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            },
            WindowsAndMessaging::SW_SHOW,
        },
    };

    pub fn shell_execute_runas(exe: &Path, args: &[&str]) -> io::Result<()> {
        let params = crate::validation::windows_command_line(args)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        if params.encode_utf16().count() + exe.as_os_str().encode_wide().count() + 4 > 32767 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Elevated hosts request exceeds the Windows command-line limit; run the command as administrator directly"));
        }
        let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
        let file: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let params: Vec<u16> = params.encode_utf16().chain(Some(0)).collect();
        // SAFETY: zero is valid for the optional fields; all referenced UTF-16
        // buffers remain live until ShellExecuteExW has consumed them.
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOW;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.hProcess == 0 {
            return Err(io::Error::other(
                "Elevation returned no owned process handle",
            ));
        }
        // A launch is not a completed update. Finite wait and checked child exit;
        // never delete/kill an elevated child on timeout or silently claim success.
        let result = match unsafe { WaitForSingleObject(info.hProcess, 120_000) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(info.hProcess, &mut code) } == 0 {
                    Err(io::Error::last_os_error())
                } else if code != 0 {
                    Err(io::Error::other(format!(
                        "Elevated hosts update exited with status {code}"
                    )))
                } else {
                    Ok(())
                }
            }
            WAIT_TIMEOUT => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Elevated hosts update is still running; verify the result before retrying",
            )),
            _ => Err(io::Error::last_os_error()),
        };
        // SAFETY: SEE_MASK_NOCLOSEPROCESS transfers ownership of this handle.
        unsafe {
            CloseHandle(info.hProcess);
        }
        result
    }
}

#[cfg(not(target_os = "windows"))]
const HOSTS_PATH: &str = "/etc/hosts";

pub struct HostsManager {
    path: PathBuf,
}

fn mappings(content: &str, hostname: &str) -> Vec<IpAddr> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let mut columns = line.split_whitespace();
            let ip = columns.next()?.parse::<IpAddr>().ok()?;
            columns
                .any(|alias| alias.eq_ignore_ascii_case(hostname))
                .then_some(ip)
        })
        .collect()
}

fn validate_entry(ip: &str, hostname: &str) -> Result<IpAddr> {
    validation::hostname(hostname)?;
    ip.parse::<IpAddr>()
        .map_err(|_| AppError::Hosts("Host address must be a literal IPv4/IPv6 address".into()))
}

fn missing(content: &str, ip: IpAddr, hostname: &str) -> Result<bool> {
    let existing = mappings(content, hostname);
    if existing.iter().any(|existing| *existing != ip) {
        return Err(AppError::Hosts(format!(
            "Host alias `{hostname}` already maps to a different address; review it explicitly"
        )));
    }
    Ok(existing.is_empty())
}

fn validated_entries(
    entries: &[(&str, &str)],
) -> Result<std::collections::BTreeMap<String, IpAddr>> {
    let mut desired = std::collections::BTreeMap::new();
    for &(ip, hostname) in entries {
        let ip = validate_entry(ip, hostname)?;
        if desired
            .insert(hostname.to_ascii_lowercase(), ip)
            .is_some_and(|old| old != ip)
        {
            return Err(AppError::Hosts(
                "Conflicting addresses in the requested hosts batch".into(),
            ));
        }
    }
    Ok(desired)
}

impl HostsManager {
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        let path = super::windows::system_directory()?
            .join("drivers")
            .join("etc")
            .join("hosts");
        #[cfg(not(windows))]
        let path = PathBuf::from(HOSTS_PATH);
        Ok(Self { path })
    }
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn is_writable(&self) -> bool {
        fs::metadata(&self.path).is_ok_and(|meta| !meta.permissions().readonly())
            && OpenOptions::new().append(true).open(&self.path).is_ok()
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

    fn read(&self) -> Result<String> {
        const MAX_HOSTS_BYTES: u64 = 16 * 1024 * 1024;
        let mut content = String::new();
        std::fs::File::open(&self.path)?
            .take(MAX_HOSTS_BYTES + 1)
            .read_to_string(&mut content)?;
        if content.len() as u64 > MAX_HOSTS_BYTES {
            return Err(AppError::Hosts(
                "Hosts file exceeds the 16 MiB limit".into(),
            ));
        }
        Ok(content)
    }

    pub fn has_host(&self, hostname: &str) -> Result<bool> {
        validation::hostname(hostname)?;
        Ok(!mappings(&self.read()?, hostname).is_empty())
    }

    pub fn elevate_and_run(&self, args: &[&str]) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            if self.path != Self::new()?.path {
                return Err(AppError::Hosts(
                    "Automatic elevation is only supported for the system hosts file".into(),
                ));
            }
            let exe = std::env::current_exe()?;
            elevation::shell_execute_runas(&exe, args)
                .map_err(|error| AppError::Hosts(format!("Elevated hosts update failed: {error}")))
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
        self.add_hosts(&[(ip, hostname)])
    }

    pub fn add_hosts(&self, entries: &[(&str, &str)]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let desired = validated_entries(entries)?;
        self.ensure_writable()?;
        // Lock the actual hosts inode, retaining its ACL/owner. Replacing a Windows
        // system file with a user-created temporary file would change those ACLs.
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.path)?;
        file.try_lock()
            .map_err(|error| AppError::Hosts(format!("Hosts file is being modified: {error}")))?;
        let mut content = String::new();
        (&mut file)
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut content)?;
        if content.len() > 16 * 1024 * 1024 {
            return Err(AppError::Hosts("Hosts file is too large".into()));
        }
        let mut append = String::new();
        for (hostname, ip) in desired {
            if missing(&content, ip, &hostname)? {
                append.push_str(&format!("{ip}\t{hostname}\n"));
            }
        }
        if append.is_empty() {
            return Ok(());
        }
        if !content.is_empty() && !content.ends_with('\n') {
            append.insert(0, '\n');
        }
        let original_length = file.metadata()?.len();
        if let Err(write_error) = file
            .write_all(append.as_bytes())
            .and_then(|_| file.sync_all())
        {
            if let Err(rollback_error) = file.set_len(original_length).and_then(|_| file.sync_all())
            {
                return Err(AppError::Hosts(format!("Hosts append failed ({write_error}) and rollback failed ({rollback_error}); inspect the file before retrying")));
            }
            return Err(write_error.into());
        }
        print_success("Hosts entries updated");
        Ok(())
    }

    pub fn ensure_host(&self, ip: &str, hostname: &str) -> Result<()> {
        let parsed = validate_entry(ip, hostname)?;
        if !missing(&self.read()?, parsed, hostname)? {
            return Ok(());
        }
        if self.is_writable() {
            return self.add_host(ip, hostname);
        }
        print_info("Administrator privileges required for the explicit hosts update");
        self.elevate_and_run(&["hosts", "custom", ip, hostname])?;
        if missing(&self.read()?, parsed, hostname)? {
            return Err(AppError::Hosts(
                "Elevated process did not add the requested host mapping".into(),
            ));
        }
        Ok(())
    }

    pub fn ensure_hosts(&self, entries: &[(&str, &str)]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let desired = validated_entries(entries)?;
        let content = self.read()?;
        let mut any_missing = false;
        for (hostname, ip) in &desired {
            any_missing |= missing(&content, *ip, hostname)?;
        }
        if !any_missing {
            return Ok(());
        }
        if self.is_writable() {
            return self.add_hosts(entries);
        }
        // Pass the already validated snapshot as arguments. An elevated child
        // must not reread a user-writable config that can change after approval.
        let mut arguments = vec!["hosts".to_owned(), "apply".to_owned()];
        for (hostname, ip) in &desired {
            arguments.extend(["--entry".to_owned(), format!("{ip}={hostname}")]);
        }
        let arguments: Vec<_> = arguments.iter().map(String::as_str).collect();
        self.elevate_and_run(&arguments)?;
        let updated = self.read()?;
        for &(ip, hostname) in entries {
            if missing(&updated, validate_entry(ip, hostname)?, hostname)? {
                return Err(AppError::Hosts(
                    "Elevated process did not complete the requested hosts mappings".into(),
                ));
            }
        }
        Ok(())
    }
}
