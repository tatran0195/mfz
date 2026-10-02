# Design Spec: Dynamic Hosts Management with UAC Self-Elevation

- **Date**: 2026-09-09
- **Status**: Approved

## 1. Overview

MFZ uses hostname shortcuts (e.g. `stg-rdp`) mapped to `127.0.0.1` so that Windows Remote Desktop (`mstsc`) can connect through SSM port forwarding tunnels and avoid credential conflicts in Windows Credential Manager (`cmdkey`).

Currently:

1. `mfz config add` creates an environment in `mfz.ini`, but does not automatically add the hostname alias to the system hosts file (`C:\Windows\System32\drivers\etc\hosts`).
2. `mfz hosts add` hardcodes `stg-rdp` and `prd-rdp`.
3. Writing to the hosts file on Windows fails with an error if the terminal is not run as Administrator, requiring users to manually restart their terminal with elevation.

This design introduces:

- Automated hosts entry creation when running `mfz config add`.
- Automatic Windows UAC self-elevation whenever admin permissions are required to modify the hosts file.
- Dynamic environment parsing in `mfz hosts add` to read all `rdp_host` entries from `mfz.ini` without hardcoding.
- Synchronization for current configured profiles.

---

## 2. Architecture & Elevation

### 2.1 Elevation Detection & Handling (`src/hosts.rs`)

- `HostsManager::is_writable(&self) -> bool`:
  Tests write permission to the target hosts file by opening it in append mode.
- `HostsManager::elevate_and_run(&self, args: &[&str]) -> Result<()>`:
  - On Windows: Uses `powershell Start-Process -Verb RunAs -Wait -FilePath <current_exe> -ArgumentList <args>` (or Windows `ShellExecuteExW` with `runas`). This displays the standard Windows UAC prompt dialog to the user without prior terminal prompting.
  - Waits for child process termination and verifies a successful zero exit code.
  - On non-Windows: Returns an informative `AppError::Hosts` error explaining that root / `sudo` privileges are required.

### 2.2 Reusable Host Addition

- `HostsManager::ensure_host(&self, ip: &str, hostname: &str) -> Result<()>`:
  1. Checks if the host entry already exists (ignoring comments).
  2. If it already exists, skips writing and prints an informative notice.
  3. If not present:
     - If `is_writable()` is true, appends the entry directly.
     - If `is_writable()` is false, invokes `elevate_and_run(&["hosts", "custom", ip, hostname])`.
- `HostsManager::ensure_hosts(&self, entries: &[(&str, &str)], config_path: Option<&str>) -> Result<()>`:
  - If writable: adds all entries directly.
  - If not writable: invokes `elevate_and_run(&["hosts", "add", "--config", config_path])` to batch-add all missing entries in a single elevated prompt.

---

## 3. CLI & Command Improvements

### 3.1 `mfz config add` (`src/cli.rs` & `src/main.rs`)

- After writing the new profile to `mfz.ini`, `cmd_config` immediately calls `HostsManager::ensure_host("127.0.0.1", &rdp_host)`.
- If permission is required, the UAC prompt appears automatically, the host entry is added, and the user receives confirmation for both config creation and hosts file update.

### 3.2 `mfz hosts add` (`src/cli.rs` & `src/main.rs`)

- Changes `HostsCommand::Add` in `cli.rs`:

  ```rust
  Add {
      #[arg(short, long, default_value = "mfz.ini")]
      config: String,
  }
  ```

- In `main.rs`:
  1. Loads `ConfigLoader::new(&config).get_all()?`.
  2. Extracts all unique `rdp_host` values from environments.
  3. If none found, prints a warning that no environments were found in the config.
  4. Calls `HostsManager` to ensure all `(127.0.0.1, rdp_host)` mappings exist, elevating if needed.

### 3.3 Existing Profiles Synchronization

- Running `mfz hosts add` reads `mfz.ini` and syncs existing profiles (e.g. `stg` -> `stg-rdp`).

---

## 4. Error Handling & Edge Cases

- **UAC Cancellation**: If the user rejects the UAC prompt, the child process exits with an error or cancellation code; MFZ catches this and returns a clean error (`Administrator permission was denied.`).
- **Already Present**: If `rdp_host` is already present in the hosts file, no elevation prompt is triggered and no redundant writes occur.
- **Multiple Environments**: If `mfz hosts add` is invoked, it elevates once to write all missing hosts in a single batch.

---

## 5. Testing & Verification

1. **Unit / Integration Tests**:
   - Test `HostsManager` with mock hosts file in a temporary directory (`tempfile`) for add, duplicate check, and batch operations.
   - Verify `mfz hosts add` argument parsing with `--config`.
2. **End-to-End CLI Verification**:
   - Verify `mfz hosts add` reads environments from a custom config file and detects missing vs existing hosts.
   - Verify `mfz config add` updates both config and hosts file.
   - Verify current profiles (`stg` in `mfz.ini`) are added without hardcoding.
