# Dynamic Profile Hosts Management & UAC Elevation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically manage system hosts aliases during profile addition, support automatic UAC elevation on Windows when admin rights are needed, and dynamically sync hosts for configured profiles without hardcoding.

**Architecture:** Extend `HostsManager` with write capability checking, duplicate detection, and Windows UAC self-elevation. Integrate `ensure_host` into `mfz config add`. Update `mfz hosts add` to parse environments dynamically from `mfz.ini` and batch-sync all `rdp_host` entries.

**Tech Stack:** Rust, `clap` (derive), `std::process::Command`, PowerShell `Start-Process -Verb RunAs`, `configparser`.

## Global Constraints
- Only request UAC elevation if target entry is missing AND hosts file is not writable.
- Do not hardcode any environment names or hostnames (`stg-rdp`, `prd-rdp`).
- Re-use `mfz` binary itself for elevated execution via `hosts custom` / `hosts add`.
- Keep existing CLI flags and tests backward compatible.

---

### Task 1: Extend `HostsManager` with Pre-checks, Auto-Elevation, and Batch Synchronization

**Files:**
- Modify: `src/hosts.rs`
- Test: `tests/hosts.rs`

**Interfaces:**
- Consumes: `AppError::Hosts(String)`
- Produces:
  - `HostsManager::has_host(&self, hostname: &str) -> Result<bool>`
  - `HostsManager::is_writable(&self) -> bool`
  - `HostsManager::elevate_and_run(&self, args: &[&str]) -> Result<()>`
  - `HostsManager::ensure_host(&self, ip: &str, hostname: &str) -> Result<()>`
  - `HostsManager::ensure_hosts(&self, entries: &[(&str, &str)], config_path: Option<&str>) -> Result<()>`

- [ ] **Step 1: Write unit tests for `has_host`, `ensure_host`, and `ensure_hosts`**

Add tests to `tests/hosts.rs`:
```rust
#[test]
fn test_hosts_has_host_and_ensure() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "hosts_test_ensure_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));

    fs::write(&file_path, "127.0.0.1\tlocalhost\n").unwrap();
    let mgr = HostsManager::with_path(file_path.clone());

    assert!(mgr.has_host("localhost").unwrap());
    assert!(!mgr.has_host("app-rdp").unwrap());

    mgr.ensure_host("127.0.0.1", "app-rdp").unwrap();
    assert!(mgr.has_host("app-rdp").unwrap());

    // Calling ensure_host again should succeed without duplicating
    mgr.ensure_host("127.0.0.1", "app-rdp").unwrap();

    let content = fs::read_to_string(&file_path).unwrap();
    let matches: Vec<_> = content.lines().filter(|l| l.contains("app-rdp")).collect();
    assert_eq!(matches.len(), 1);

    fs::remove_file(&file_path).unwrap();
}

#[test]
fn test_hosts_ensure_hosts_batch() {
    let temp_dir = std::env::temp_dir();
    let file_path = temp_dir.join(format!(
        "hosts_test_batch_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));

    fs::write(&file_path, "127.0.0.1\tlocalhost\n").unwrap();
    let mgr = HostsManager::with_path(file_path.clone());

    let entries = [("127.0.0.1", "host-a"), ("127.0.0.1", "host-b")];
    mgr.ensure_hosts(&entries, None).unwrap();

    assert!(mgr.has_host("host-a").unwrap());
    assert!(mgr.has_host("host-b").unwrap());

    fs::remove_file(&file_path).unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test hosts`
Expected: FAIL with missing methods on `HostsManager`.

- [ ] **Step 3: Implement `has_host`, `is_writable`, `elevate_and_run`, `ensure_host`, `ensure_hosts` in `src/hosts.rs`**

```rust
impl HostsManager {
    pub fn is_writable(&self) -> bool {
        OpenOptions::new()
            .append(true)
            .open(&self.path)
            .is_ok()
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
            let exe = std::env::current_exe()
                .map_err(|e| AppError::Hosts(format!("Failed to determine executable path: {e}")))?;

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
            Err(AppError::Hosts(format!(
                "Cannot write to {} — please run with sudo",
                self.path.display()
            )))
        }
    }

    pub fn ensure_host(&self, ip: &str, hostname: &str) -> Result<()> {
        if self.has_host(hostname)? {
            print_warning(&format!("`{hostname}` already present in hosts file — skipping"));
            return Ok(());
        }

        if self.is_writable() {
            self.add_host(ip, hostname)
        } else {
            print_info(&format!("Administrator privileges required to update hosts for `{hostname}`. Requesting elevation…"));
            self.elevate_and_run(&["hosts", "custom", ip, hostname])
        }
    }

    pub fn ensure_hosts(&self, entries: &[(&str, &str)], config_path: Option<&str>) -> Result<()> {
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
            print_info("Administrator privileges required to update hosts file. Requesting elevation…");
            let mut args = vec!["hosts", "add"];
            if let Some(cp) = config_path {
                args.push("--config");
                args.push(cp);
            }
            self.elevate_and_run(&args)
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test hosts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/hosts.rs tests/hosts.rs
git commit -m "feat(hosts): add elevation, existence checks, and ensure methods"
```

---

### Task 2: Update CLI Definition for `mfz hosts add`

**Files:**
- Modify: `src/cli.rs:168-180`

**Interfaces:**
- Produces: `HostsCommand::Add { config: String }`

- [ ] **Step 1: Update `HostsCommand::Add` in `src/cli.rs`**

```rust
#[derive(Debug, Subcommand)]
pub enum HostsCommand {
    /// Add RDP host entries for environments defined in config (defaults to mfz.ini)
    Add {
        /// Path to config file
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },

    /// Add a single custom hostname entry
    Custom {
        /// IP address to map
        ip: String,
        /// Hostname alias
        hostname: String,
    },
}
```

- [ ] **Step 2: Verify `cargo check` flags compiler update needed in `main.rs`**

Run: `cargo check`
Expected: Compile error in `src/main.rs:505` (`HostsCommand::Add` needs pattern update), confirming CLI definition is updated.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(cli): add --config option to hosts add subcommand"
```

---

### Task 3: Update `cmd_hosts` to Parse Environments Dynamically without Hardcoding

**Files:**
- Modify: `src/main.rs:500-518`

**Interfaces:**
- Consumes: `ConfigLoader`, `HostsManager`, `HostsCommand::Add { config }`

- [ ] **Step 1: Update `cmd_hosts` implementation in `src/main.rs`**

```rust
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

            let entries: Vec<(&str, &str)> = unique_hosts
                .iter()
                .map(|h| ("127.0.0.1", h.as_str()))
                .collect();

            let spinner = Spinner::new(format!("Updating hosts for {} environment(s)…", entries.len()));
            mgr.ensure_hosts(&entries, Some(&config))?;
            spinner.success("Hosts file updated");
        }
        HostsCommand::Custom { ip, hostname } => {
            let spinner = Spinner::new(format!("Adding `{hostname}` → `{ip}`…"));
            mgr.ensure_host(&ip, &hostname)?;
            spinner.success("Host entry processed");
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Run `cargo check` and `cargo test`**

Run: `cargo test`
Expected: All tests compile and pass.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(hosts): dynamically sync rdp_host entries from config"
```

---

### Task 4: Integrate Hosts Entry Creation into `mfz config add`

**Files:**
- Modify: `src/main.rs:155-202`

**Interfaces:**
- Consumes: `new_env.rdp_host`, `HostsManager::ensure_host`

- [ ] **Step 1: Update `cli::ConfigCommand::Add` in `src/main.rs`**

After saving to `mfz.ini`, invoke `HostsManager::new().ensure_host("127.0.0.1", &new_env.rdp_host)`:
```rust
            let spinner = Spinner::new(format!("Saving `{name}` to `{config}`…"));
            loader.save(&name, &new_env)?;
            spinner.success(format!("Environment `{name}` added successfully."));

            // Automatically ensure host alias is in hosts file
            let hosts_mgr = HostsManager::new();
            hosts_mgr.ensure_host("127.0.0.1", &new_env.rdp_host)?;
```

- [ ] **Step 2: Verify `cargo check` and `cargo test`**

Run: `cargo test`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(config): automatically add host entry on config add"
```

---

### Task 5: Sync Current Profiles, Integration Testing, and Verification

**Files:**
- Test: `tests/hosts.rs`
- Verify: `mfz.ini` current profiles (`stg` -> `stg-rdp`)

- [ ] **Step 1: Write integration test verifying dynamic profile reading and hosts resolution**

Add test to `tests/hosts.rs`:
```rust
#[test]
fn test_dynamic_profile_hosts_sync() {
    let temp_dir = std::env::temp_dir();
    let hosts_path = temp_dir.join(format!(
        "hosts_dyn_{}.txt",
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    fs::write(&hosts_path, "# hosts\n").unwrap();

    let mgr = HostsManager::with_path(hosts_path.clone());
    let entries = [("127.0.0.1", "stg-rdp")];
    mgr.ensure_hosts(&entries, None).unwrap();

    assert!(mgr.has_host("stg-rdp").unwrap());
    fs::remove_file(&hosts_path).unwrap();
}
```

- [ ] **Step 2: Run all tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 3: Build debug binary and test CLI help and `hosts add` output**

Run: `cargo build`
Run: `target/debug/mfz hosts add --help`
Expected: Output shows `--config <CONFIG>` option with default `mfz.ini`.

- [ ] **Step 4: Commit**

```bash
git add tests/hosts.rs
git commit -m "test: verify dynamic profile hosts syncing"
```
