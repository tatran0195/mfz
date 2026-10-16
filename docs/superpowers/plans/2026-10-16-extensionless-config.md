# Extensionless MFZ Configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable MFZ to discover and read clean extensionless configuration files (`mfz`, `.mfz`, `config`) across working directories and standard user config paths, and auto-resolve `.ini` when passing `--config <name>` without an extension.

**Architecture:** Refactor `ConfigLoader::resolve_default_config_path` to search prioritized candidates (`./mfz`, `./mfz.ini`, `./.mfz`, exe directory, XDG/user config paths, APPDATA). Implement `ConfigLoader::resolve_cli_path` to preserve exact existing files while auto-detecting `<path>.ini` for extensionless explicit CLI paths without breaking directory isolation. Update CLI config display to reflect actual config filenames dynamically.

**Tech Stack:** Rust (2021 edition), `std::path::{Path, PathBuf}`, `configparser`, `cargo test`.

## Global Constraints

- Backwards compatibility: Existing `mfz.ini` configurations must continue to work without modification.
- Directory isolation: Explicit `--config <path>` must never fall back to ambient user/system directories.
- No directory collision: Only regular files (`is_file()`) must be treated as configuration candidates; folders named `mfz` must not be parsed as config files.
- Test coverage: All unit tests, audit regressions, and offline CLI checks must pass cleanly.

---

### Task 1: Implement Extensionless and User-Directory Discovery in `ConfigLoader::resolve_default_config_path`

**Files:**
- Modify: `src/config/loader.rs:80-103`
- Test: `tests/config.rs`

**Interfaces:**
- Produces: `pub fn resolve_default_config_path() -> PathBuf` with prioritized resolution:
  1. Local working directory (`./mfz`, `./mfz.ini`, `./.mfz`)
  2. Executable parent directory (`<exe_dir>/mfz`, `<exe_dir>/mfz.ini`)
  3. XDG config directory (`$XDG_CONFIG_HOME/mfz/config`, `$XDG_CONFIG_HOME/mfz/mfz.ini`)
  4. User home directories (`~/.config/mfz/config`, `~/.config/mfz/mfz.ini`, `~/.mfz/config`, `~/.mfz/mfz`, `~/.mfz/mfz.ini`)
  5. Windows APPDATA (`%APPDATA%/mfz/config`, `%APPDATA%/mfz/mfz.ini`)
  6. Fallback: `PathBuf::from("mfz.ini")`

- [ ] **Step 1: Write the failing unit test for extensionless default discovery**

Add to `tests/config.rs`:
```rust
#[test]
fn test_default_config_discovery_prefers_extensionless_mfz() {
    let directory = tempfile::tempdir().unwrap();
    let mfz_path = directory.path().join("mfz");
    let ini_path = directory.path().join("mfz.ini");

    std::fs::write(&ini_path, "[stg]\nprofile=legacy\nremote_port=3389\nlocal_port=13389\nrdp_host=legacy-host\n").unwrap();
    std::fs::write(&mfz_path, "[stg]\nprofile=modern\nremote_port=3389\nlocal_port=13389\nrdp_host=modern-host\n").unwrap();

    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(directory.path()).unwrap();

    let resolved = mfz::config::ConfigLoader::resolve_default_config_path();
    let loader = mfz::config::ConfigLoader::new(&resolved);
    let env = loader.load("stg").unwrap();

    std::env::set_current_dir(previous).unwrap();

    assert_eq!(resolved, std::path::PathBuf::from("mfz"));
    assert_eq!(env.profile.as_deref(), Some("modern"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config test_default_config_discovery_prefers_extensionless_mfz`  
Expected: FAIL (returns `"mfz.ini"` because `"mfz"` is not yet searched).

- [ ] **Step 3: Implement prioritized candidate discovery in `src/config/loader.rs`**

Replace `resolve_default_config_path` in `src/config/loader.rs`:
```rust
    pub fn resolve_default_config_path() -> PathBuf {
        // 1. Current working directory
        for name in ["mfz", "mfz.ini", ".mfz"] {
            let path = PathBuf::from(name);
            if path.is_file() {
                return path;
            }
        }

        // 2. Application executable directory (portable deployment)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                for name in ["mfz", "mfz.ini"] {
                    let candidate = parent.join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }

        // 3. User & platform config locations
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            for name in ["config", "mfz", "mfz.ini"] {
                let candidate = PathBuf::from(&xdg).join("mfz").join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }

        for home_var in ["USERPROFILE", "HOME"] {
            if let Some(base) = std::env::var_os(home_var) {
                let base_path = PathBuf::from(base);
                for name in ["config", "mfz", "mfz.ini"] {
                    let candidate = base_path.join(".config").join("mfz").join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
                for name in ["config", "mfz", "mfz.ini"] {
                    let candidate = base_path.join(".mfz").join(name);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }

        if let Some(appdata) = std::env::var_os("APPDATA") {
            let base_path = PathBuf::from(appdata);
            for name in ["config", "mfz", "mfz.ini"] {
                let candidate = base_path.join("mfz").join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }

        PathBuf::from("mfz.ini")
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test config test_default_config_discovery_prefers_extensionless_mfz`  
Expected: PASS

- [ ] **Step 5: Commit changes**

```bash
git add src/config/loader.rs tests/config.rs
git commit -m "feat: support extensionless and user-directory config discovery"
```

---

### Task 2: Implement Extension Auto-Detection for Explicit `--config` Paths

**Files:**
- Modify: `src/config/loader.rs:72-78`
- Test: `tests/config.rs`

**Interfaces:**
- Produces: `pub fn resolve_cli_path(input: impl AsRef<Path>) -> PathBuf`
  - If `input` exists as a file, returns `input.as_ref().to_path_buf()`.
  - If `input` has no extension and `<input>.ini` exists as a file, returns `<input>.ini`.
  - Otherwise returns `input.as_ref().to_path_buf()`.
- Updates: `ConfigLoader::from_cli(path: Option<&str>) -> Self` to use `resolve_cli_path`.

- [ ] **Step 1: Write failing unit tests for explicit `--config` path resolution**

Add to `tests/config.rs`:
```rust
#[test]
fn test_cli_path_resolution_auto_detects_ini_for_extensionless_argument() {
    let directory = tempfile::tempdir().unwrap();
    let ini_path = directory.path().join("production.ini");
    std::fs::write(&ini_path, "[prd]\nprofile=test\nremote_port=3389\nlocal_port=13389\nrdp_host=h\n").unwrap();

    let without_ext = directory.path().join("production");
    let resolved = mfz::config::ConfigLoader::resolve_cli_path(&without_ext);
    assert_eq!(resolved, ini_path);
}

#[test]
fn test_cli_path_resolution_preserves_exact_extensionless_file_when_present() {
    let directory = tempfile::tempdir().unwrap();
    let exact_path = directory.path().join("production");
    std::fs::write(&exact_path, "[prd]\nprofile=test\nremote_port=3389\nlocal_port=13389\nrdp_host=h\n").unwrap();

    let resolved = mfz::config::ConfigLoader::resolve_cli_path(&exact_path);
    assert_eq!(resolved, exact_path);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config test_cli_path_resolution`  
Expected: FAIL (method `resolve_cli_path` does not exist).

- [ ] **Step 3: Implement `resolve_cli_path` in `src/config/loader.rs`**

Add to `ConfigLoader` in `src/config/loader.rs`:
```rust
    pub fn resolve_cli_path(input: impl AsRef<Path>) -> PathBuf {
        let path = input.as_ref().to_path_buf();
        if path.is_file() {
            return path;
        }
        if path.extension().is_none() {
            let with_ini = path.with_extension("ini");
            if with_ini.is_file() {
                return with_ini;
            }
        }
        path
    }

    pub fn from_cli(path: Option<&str>) -> Self {
        path.map(Self::resolve_cli_path)
            .map(Self::new)
            .unwrap_or_else(|| Self {
                path: Self::resolve_default_config_path(),
            })
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test config test_cli_path_resolution`  
Expected: PASS

- [ ] **Step 5: Commit changes**

```bash
git add src/config/loader.rs tests/config.rs
git commit -m "feat: auto-detect .ini for explicit extensionless --config paths"
```

---

### Task 3: Display Dynamic Config File Name in CLI Output

**Files:**
- Modify: `src/commands/config.rs:38-60`

**Interfaces:**
- Updates: In `src/commands/config.rs`, replace hardcoded `"mfz.ini (Plaintext)"` labels with the actual filename of `loader.path()`:
  ```rust
  let config_file_label = loader
      .path()
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("Config file");
  ```

- [ ] **Step 1: Update `src/commands/config.rs` to show dynamic config filename**

In `src/commands/config.rs`:
```rust
                    let config_source_name = loader
                        .path()
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Config");

                    if env.auth_type == AuthType::Mfa {
                        let secret_loc = if env.secret.is_some() {
                            format!("{config_source_name} (Plaintext)").yellow()
                        } else {
                            match CredentialStore::get_totp_secret(&name) {
                                Ok(Some(_)) => "Windows Credential Manager".green(),
                                Ok(None) => "Missing".red(),
                                Err(error) => format!("Unavailable: {error}").yellow(),
                            }
                        };
                        println!("      {:<12} {}", "MFA Secret:".dimmed(), secret_loc);
                    }

                    let pass_loc = if env.rdp_password.is_some() {
                        format!("{config_source_name} (Plaintext)").yellow()
                    } else {
                        match CredentialStore::get_rdp_password(&name) {
                            Ok(Some(_)) => "Windows Credential Manager".green(),
                            Ok(None) => "Not configured".dimmed(),
                            Err(error) => format!("Unavailable: {error}").yellow(),
                        }
                    };
                    println!("      {:<12} {}", "RDP Pass:".dimmed(), pass_loc);
```

- [ ] **Step 2: Run all config tests to ensure no regressions**

Run: `cargo test --test config`  
Expected: PASS

- [ ] **Step 3: Commit changes**

```bash
git add src/commands/config.rs
git commit -m "style: dynamic config source labeling in config command output"
```

---

### Task 4: Add User-Directory Config Tests & Verify Full Regression Suite

**Files:**
- Modify: `tests/config.rs`
- Run: `tests/audit_regressions.rs`
- Run: `tools/audit_cli_test.py`

- [ ] **Step 1: Add user-directory fallback tests in `tests/config.rs`**

Add tests covering:
- Fallback to `USERPROFILE/.config/mfz/config`
- Fallback to `USERPROFILE/.mfz/config`
- Ignoring directories named `mfz` (ensuring `is_file()` prevents directory collisions)

```rust
#[test]
fn test_default_config_ignores_directory_named_mfz() {
    let directory = tempfile::tempdir().unwrap();
    let mfz_dir = directory.path().join("mfz");
    std::fs::create_dir(&mfz_dir).unwrap();
    let ini_path = directory.path().join("mfz.ini");
    std::fs::write(&ini_path, "[stg]\nprofile=test\nremote_port=3389\nlocal_port=13389\nrdp_host=h\n").unwrap();

    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(directory.path()).unwrap();

    let resolved = mfz::config::ConfigLoader::resolve_default_config_path();
    std::env::set_current_dir(previous).unwrap();

    assert_eq!(resolved, std::path::PathBuf::from("mfz.ini"));
}
```

- [ ] **Step 2: Run all unit and regression tests**

Run: `cargo test`  
Expected: All tests PASS.

- [ ] **Step 3: Build release binary and run audit CLI test harness**

Run:
```powershell
cargo build --locked --bin mfz
python tools/audit_cli_test.py --binary target/debug/mfz.exe
```
Expected:
`PASS: 16 actual-application offline subprocess checks; isolated files, no AWS/hosts/keyring mutations`

- [ ] **Step 4: Commit test updates**

```bash
git add tests/config.rs
git commit -m "test: add tests for user directory fallback and directory collision protection"
```

---

### Task 5: Documentation Updates

**Files:**
- Modify: `README.md`
- Modify: `README.dev.md`

- [ ] **Step 1: Update `README.md` and `README.dev.md`**

Update configuration sections to explain:
- Main configuration files can be named `mfz` *(clean extensionless format)* or `mfz.ini`.
- `--config` flag accepts extensionless names (e.g. `--config mfz` or `--config prod`) and automatically resolves `.ini` if needed.
- Standard search paths now include `$XDG_CONFIG_HOME/mfz/config`, `~/.config/mfz/config`, and `~/.mfz/config`.

- [ ] **Step 2: Commit documentation**

```bash
git add README.md README.dev.md
git commit -m "docs: document extensionless mfz configuration support and discovery hierarchy"
```
