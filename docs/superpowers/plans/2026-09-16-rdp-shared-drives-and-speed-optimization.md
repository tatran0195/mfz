# RDP Shared Local Drive/Folder & Connection Speed Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable configurable local drive/folder sharing into remote Windows RDP sessions and optimize RDP connection responsiveness and transfer throughput over SSM tunnels via dynamically generated `.rdp` profiles.

**Architecture:** Extend `EnvironmentConfig` and CLI definitions with `shared_drives`, `shared_folder`, and `optimize_connection`. In `src/rdp.rs`, build an `.rdp` profile generator that includes `drivestoredirect` and performance tuning directives (bulk compression, persistent bitmap cache, LAN network mode, disabling audio/printer bloat), map custom folders via Windows `subst` virtual drives with guaranteed RAII unmounting, and launch `mstsc` with the generated `.rdp` file.

**Tech Stack:** Rust (1.70+), `mstsc.exe`, Windows Credential Manager (`cmdkey`), Windows `subst.exe`, `tempfile`/std::env::temp_dir.

## Global Constraints

- Must maintain full backward compatibility with existing `mfz.ini` files.
- Default `shared_drives` is `*` (share all drives) unless set to `none` or specific drive letters.
- Default `optimize_connection` is `true`.
- Any temporary `.rdp` files and virtual `subst` drives must be cleaned up reliably upon session termination.
- All existing tests in `tests/` must continue to pass.

---

### Task 1: Extend Configuration Schema & Serialization

**Files:**

- Modify: `src/config.rs`
- Test: `tests/config.rs`

**Interfaces:**

- Consumes: `Ini` parser from `configparser`
- Produces:

  ```rust
  pub struct EnvironmentConfig {
      // existing fields...
      pub shared_drives: Option<String>,
      pub shared_folder: Option<String>,
      pub optimize_connection: Option<bool>,
  }
  ```

- [ ] **Step 1: Write failing tests for new config fields**
In `tests/config.rs`, add tests for parsing and saving `shared_drives`, `shared_folder`, and `optimize_connection`:

```rust
#[test]
fn test_config_shared_drives_and_optimization() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("mfz.ini");

    let content = r#"
[test_env]
profile = dev
instance = i-12345
rdp_host = test-host
remote_port = 3389
local_port = 13389
shared_drives = C:,D:
shared_folder = C:\Shared
optimize_connection = true
"#;
    std::fs::write(&config_path, content).unwrap();

    let loader = ConfigLoader::new(&config_path);
    let env = loader.load("test_env").unwrap();

    assert_eq!(env.shared_drives.as_deref(), Some("C:,D:"));
    assert_eq!(env.shared_folder.as_deref(), Some(r"C:\Shared"));
    assert_eq!(env.optimize_connection, Some(true));
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test --test config`
Expected: FAIL (fields `shared_drives`, `shared_folder`, `optimize_connection` do not exist on `EnvironmentConfig`).

- [ ] **Step 3: Implement new fields and parsing in `src/config.rs`**
Update `EnvironmentConfig` struct, `from_section`, and `to_map`:

```rust
pub struct EnvironmentConfig {
    pub profile: String,
    pub auth_type: AuthType,
    pub serial: Option<String>,
    pub secret: Option<String>,
    pub instance: Option<String>,
    pub instance_tag: Option<String>,
    pub remote_port: u16,
    pub local_port: u16,
    pub rdp_host: String,
    pub rdp_user: Option<String>,
    pub rdp_password: Option<String>,
    pub shared_drives: Option<String>,
    pub shared_folder: Option<String>,
    pub optimize_connection: Option<bool>,
}
```

In `from_section`:

```rust
let parse_bool = |key: &str| -> Option<bool> {
    ini.get(name, key).and_then(|v| match v.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    })
};

let shared_drives = get_opt("shared_drives");
let shared_folder = get_opt("shared_folder");
let optimize_connection = parse_bool("optimize_connection");
```

And in `to_map`:

```rust
if let Some(drives) = &self.shared_drives {
    m.insert("shared_drives".into(), drives.clone());
}
if let Some(folder) = &self.shared_folder {
    m.insert("shared_folder".into(), folder.clone());
}
if let Some(opt) = self.optimize_connection {
    m.insert("optimize_connection".into(), opt.to_string());
}
```

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --test config`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config.rs
git commit -m "feat(config): add shared_drives, shared_folder, and optimize_connection"
```

---

### Task 2: Dynamic `.rdp` Content Generation & Performance Directives

**Files:**

- Modify: `src/rdp.rs`
- Test: `tests/rdp.rs`

**Interfaces:**

- Consumes: Configuration values (host, port, fullscreen, username, shared_drives, optimize_connection)
- Produces:

  ```rust
  pub struct RdpProfileOptions<'a> {
      pub host: &'a str,
      pub port: u16,
      pub fullscreen: bool,
      pub username: Option<&'a str>,
      pub drives_to_redirect: Option<&'a str>,
      pub optimize_connection: bool,
  }

  pub fn build_rdp_file_content(opts: &RdpProfileOptions) -> String;
  ```

- [ ] **Step 1: Write unit tests for `.rdp` profile builder**
Create `tests/rdp.rs`:

```rust
use mfz::rdp::{build_rdp_file_content, RdpProfileOptions};

#[test]
fn test_build_rdp_content_with_optimization_and_drives() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: true,
        username: Some("Administrator"),
        drives_to_redirect: Some("*"),
        optimize_connection: true,
    };

    let content = build_rdp_file_content(&opts);

    assert!(content.contains("full address:s:stg-rdp:13389"));
    assert!(content.contains("screen mode id:i:2"));
    assert!(content.contains("username:s:Administrator"));
    assert!(content.contains("drivestoredirect:s:*"));
    assert!(content.contains("compression:i:1"));
    assert!(content.contains("bitmapcachepersistenable:i:1"));
    assert!(content.contains("connection type:i:6"));
    assert!(content.contains("audiomode:i:2"));
}

#[test]
fn test_build_rdp_content_no_optimization_and_specific_drives() {
    let opts = RdpProfileOptions {
        host: "prd-rdp",
        port: 3389,
        fullscreen: false,
        username: None,
        drives_to_redirect: Some("C:;D:"),
        optimize_connection: false,
    };

    let content = build_rdp_file_content(&opts);

    assert!(content.contains("full address:s:prd-rdp:3389"));
    assert!(content.contains("screen mode id:i:1"));
    assert!(content.contains("drivestoredirect:s:C:;D:"));
    assert!(!content.contains("connection type:i:6"));
    assert!(!content.contains("compression:i:1"));
}

#[test]
fn test_build_rdp_content_disabled_drives() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: false,
        username: None,
        drives_to_redirect: None,
        optimize_connection: true,
    };

    let content = build_rdp_file_content(&opts);
    assert!(content.contains("drivestoredirect:s:"));
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test --test rdp`
Expected: FAIL (`build_rdp_file_content` and `RdpProfileOptions` not found).

- [ ] **Step 3: Implement `build_rdp_file_content` in `src/rdp.rs`**
In `src/rdp.rs`:

```rust
#[derive(Debug, Clone)]
pub struct RdpProfileOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub fullscreen: bool,
    pub username: Option<&'a str>,
    pub drives_to_redirect: Option<&'a str>,
    pub optimize_connection: bool,
}

pub fn build_rdp_file_content(opts: &RdpProfileOptions) -> String {
    let mut lines = Vec::new();

    lines.push(format!("full address:s:{}:{}", opts.host, opts.port));
    lines.push(format!("screen mode id:i:{}", if opts.fullscreen { 2 } else { 1 }));
    lines.push("use multimon:i:0".into());
    lines.push("smart sizing:i:1".into());

    let drives = opts.drives_to_redirect.unwrap_or("");
    lines.push(format!("drivestoredirect:s:{drives}"));
    lines.push("redirectclipboard:i:1".into());

    if let Some(user) = opts.username {
        lines.push(format!("username:s:{user}"));
    }
    lines.push("prompt for credentials:i:0".into());

    if opts.optimize_connection {
        lines.push("compression:i:1".into());
        lines.push("bitmapcachepersistenable:i:1".into());
        lines.push("connection type:i:6".into());
        lines.push("networkautodetect:i:0".into());
        lines.push("bandwidthautodetect:i:1".into());
        lines.push("audiomode:i:2".into());
        lines.push("redirectprinters:i:0".into());
        lines.push("redirectsmartcards:i:0".into());
        lines.push("disable wallpaper:i:1".into());
        lines.push("disable full window drag:i:1".into());
        lines.push("disable menu anims:i:1".into());
        lines.push("disable themes:i:0".into());
        lines.push("allow font smoothing:i:1".into());
    }

    lines.join("\r\n") + "\r\n"
}
```

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --test rdp`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/rdp.rs tests/rdp.rs
git commit -m "feat(rdp): add build_rdp_file_content with performance tuning directives"
```

---

### Task 3: Virtual Drive Mapping Guard (`subst`) & Full RDP Launch Lifecycle

**Files:**

- Modify: `src/rdp.rs`
- Test: `tests/rdp.rs`

**Interfaces:**

- Consumes: Folder path, `subst.exe`
- Produces:

  ```rust
  pub struct FolderDriveGuard {
      drive_letter: char,
  }
  impl FolderDriveGuard {
      pub fn mount(folder: &Path) -> Result<Self>;
      pub fn drive_letter(&self) -> char;
      pub fn unmount(self);
  }

  pub struct LaunchOptions<'a> {
      pub host: &'a str,
      pub port: u16,
      pub fullscreen: bool,
      pub username: Option<&'a str>,
      pub password: Option<&'a str>,
      pub shared_drives: Option<&'a str>,
      pub shared_folder: Option<&'a Path>,
      pub optimize_connection: bool,
  }

  pub fn launch(opts: LaunchOptions) -> Result<()>;
  ```

- [ ] **Step 1: Write tests for drive letter allocation & resolver logic**
In `tests/rdp.rs`:

```rust
use mfz::rdp::resolve_drives_to_redirect;

#[test]
fn test_resolve_drives_to_redirect() {
    assert_eq!(resolve_drives_to_redirect(Some("*"), None), Some("*".to_string()));
    assert_eq!(resolve_drives_to_redirect(Some("none"), None), None);
    assert_eq!(resolve_drives_to_redirect(Some("C:"), Some('Z')), Some("C:;Z:".to_string()));
    assert_eq!(resolve_drives_to_redirect(None, Some('Z')), Some("Z:".to_string()));
    assert_eq!(resolve_drives_to_redirect(Some("none"), Some('Z')), Some("Z:".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test --test rdp`
Expected: FAIL (`resolve_drives_to_redirect` not found).

- [ ] **Step 3: Implement `FolderDriveGuard`, `resolve_drives_to_redirect`, and `rdp::launch`**
In `src/rdp.rs`:
- Implement `find_available_drive_letter() -> Result<char>`:
  Checks `Z` down to `E` using `std::path::Path::new(&format!("{}:\\", letter)).exists()`.
- Implement `FolderDriveGuard`:
  Mounts via `Command::new("subst").args([&format!("{letter}:"), folder.to_str().unwrap()])`.
  Implements `Drop` to run `subst {letter}: /d`.
- Implement `resolve_drives_to_redirect(shared_drives: Option<&str>, folder_letter: Option<char>) -> Option<String>`.
- In `rdp::launch`:
  - Mount folder if provided.
  - Resolve drives list.
  - Build `.rdp` content string.
  - Write to temp file `std::env::temp_dir().join(format!("mfz-{}.rdp", uuid_or_timestamp))`.
  - Cache credentials with `cmdkey`.
  - Execute `mstsc.exe` passing the temp `.rdp` file.
  - Ensure temp `.rdp` file is removed, credentials deleted, and folder unmounted in `finally` / drop manner.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --test rdp`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/rdp.rs tests/rdp.rs
git commit -m "feat(rdp): add folder subst guard, drive resolution, and temp rdp launch"
```

---

### Task 4: CLI Flags & Command Dispatch Integration

**Files:**

- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Test: `tests/config.rs`

**Interfaces:**

- Consumes: CLI args from `clap`
- Produces: Updated CLI args for `LaunchArgs`, `ConfigCommand::Add`, `ConfigCommand::Edit`, and terminal display in `cmd_config`.

- [ ] **Step 1: Add new flags in `src/cli.rs`**
In `LaunchArgs`:

```rust
/// Override shared drives for this session (e.g. *, C:, or "none")
#[arg(long = "share-drives")]
pub share_drives: Option<String>,

/// Share a specific local folder into the session (mapped to a virtual drive)
#[arg(long = "share-folder")]
pub share_folder: Option<String>,

/// Disable RDP performance optimizations
#[arg(long = "no-optimize")]
pub no_optimize: bool,
```

In `ConfigCommand::Add` & `ConfigCommand::Edit`:

```rust
#[arg(long = "shared-drives")]
pub shared_drives: Option<String>,

#[arg(long = "shared-folder")]
pub shared_folder: Option<String>,

#[arg(long = "optimize-connection")]
pub optimize_connection: Option<bool>,
```

- [ ] **Step 2: Update `cmd_config` and `cmd_launch` in `src/main.rs`**
In `cmd_config`:
- `list`: Print `Shared Drives`, `Shared Folder`, and `Optimization` for each env.
- `add`: Store `shared_drives`, `shared_folder`, and `optimize_connection`.
- `edit`: Update `shared_drives`, `shared_folder`, and `optimize_connection` if provided.

In `cmd_launch`:

- Resolve `shared_drives`: CLI `--share-drives` overrides env config (default: `"*"`. If `"none"` or empty, disabled).
- Resolve `shared_folder`: CLI `--share-folder` overrides env config.
- Resolve `optimize_connection`: `!args.no_optimize && env.optimize_connection.unwrap_or(true)`.
- Pass to `rdp::launch`.

- [ ] **Step 3: Run all unit and integration tests**
Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat(cli): integrate drive sharing and optimization flags into launch and config"
```

---

### Task 5: End-to-End Verification & Documentation

**Files:**

- Modify: `README.md`
- Modify: `README.dev.md`

- [ ] **Step 1: Update documentation**
Update `README.md` and `README.dev.md` with:
- Explanations of `shared_drives`, `shared_folder`, and `optimize_connection`.
- Examples of launching with `--share-folder` and `--share-drives`.
- Performance characteristics of RDP over SSM.

- [ ] **Step 2: Run formatting and lints**
Run: `cargo fmt --check` and `cargo clippy -- -D warnings`
Expected: Clean with 0 warnings.

- [ ] **Step 3: Commit**

```bash
git add README.md README.dev.md
git commit -m "docs: document RDP shared drives and connection optimization options"
```

---

## Plan Review Checklist

- [x] Spec coverage verified against `2026-09-16-rdp-shared-drive-and-speed-optimization-design.md`
- [x] No placeholders or undefined types
- [x] Step-by-step TDD with explicit tests and commands
