# RDP Shared Folder Fix, Customizable Optimization, and Fullscreen Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix shared folder mapping errors (`\\tsclient\E`), introduce configurable optimization profiles (`speed`, `balanced`, `quality`, `none`) with standard unoptimized default, and establish fullscreen as default launch mode with `--windowed` override and dynamic resolution.

**Architecture:**

1. Refactor configuration loading in `src/config.rs` to support `share_folder`/`shared_folder` aliases, `fullscreen` setting, `optimize` profiles, and granular overrides.
2. Update CLI commands in `src/cli.rs` to replace `--fullscreen` with `--windowed`, remove `--no-optimize`, and add `--optimize <profile>`.
3. Enhance RDP profile generation in `src/rdp.rs` with `OptimizationSettings`, `dynamic resolution:i:1`, and isolated virtual drive redirection that avoids empty CD-ROM/optical drives.
4. Integrate the updated configuration and launch flow in `src/main.rs`.

**Tech Stack:** Rust 2021, `clap`, `configparser`, `windows-sys` / Windows Win32 APIs, `mstsc`.

## Global Constraints

- Target Windows OS using powershell.
- Backward compatibility: parse both `share_folder` and `shared_folder`, `share_drives` and `shared_drives`.
- Default optimization is `none` (MSTSC defaults).
- Default launch display mode is `fullscreen` (overridden by `--windowed` or `fullscreen = false` in `mfz.ini`).

---

### Task 1: Configuration Schema & Aliases

**Files:**

- Modify: `src/config.rs`
- Test: `tests/config.rs`

**Interfaces:**

- `EnvironmentConfig`:
  - `pub fullscreen: Option<bool>`
  - `pub optimize: Option<String>`
  - `pub opt_audio: Option<bool>`
  - `pub opt_wallpaper: Option<bool>`
  - `pub opt_font_smoothing: Option<bool>`
- `from_section`: reads `share_folder` or `shared_folder`, `share_drives` or `shared_drives`, `fullscreen`, `optimize`, and granular overrides.

- [ ] **Step 1: Write the failing tests in `tests/config.rs`**
Add tests verifying `share_folder` alias, `fullscreen`, `optimize` profiles, and granular overrides.

```rust
#[test]
fn test_config_share_folder_alias_and_fullscreen() {
    let content = r#"
[stg]
profile = dev
instance = i-12345
rdp_host = stg-rdp
remote_port = 3389
local_port = 13389
share_folder = D:\SharedData
fullscreen = false
optimize = speed
audio = true
wallpaper = false
font_smoothing = true
"#;
    let path = create_temp_config(content);
    let loader = ConfigLoader::new(&path);
    let env = loader.load("stg").unwrap();

    assert_eq!(env.shared_folder.as_deref(), Some(r"D:\SharedData"));
    assert_eq!(env.fullscreen, Some(false));
    assert_eq!(env.optimize.as_deref(), Some("speed"));
    assert_eq!(env.opt_audio, Some(true));
    assert_eq!(env.opt_wallpaper, Some(false));
    assert_eq!(env.opt_font_smoothing, Some(true));

    fs::remove_file(&path).unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test --test config test_config_share_folder_alias_and_fullscreen`
Expected: FAIL (field `fullscreen`, `optimize`, etc. do not exist on `EnvironmentConfig`).

- [ ] **Step 3: Update `src/config.rs`**
Update `EnvironmentConfig` struct, `from_section`, and `to_map` to handle:
- `shared_folder`: check `get_opt("shared_folder").or_else(|| get_opt("share_folder"))`
- `shared_drives`: check `get_opt("shared_drives").or_else(|| get_opt("share_drives"))`
- `fullscreen`: check `parse_bool("fullscreen")`
- `optimize`: check `get_opt("optimize").or_else(|| get_opt("optimize_profile"))`
- `opt_audio`: `parse_bool("audio").or_else(|| parse_bool("opt_audio"))`
- `opt_wallpaper`: `parse_bool("wallpaper").or_else(|| parse_bool("opt_wallpaper"))`
- `opt_font_smoothing`: `parse_bool("font_smoothing").or_else(|| parse_bool("opt_font_smoothing"))`

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --test config`
Expected: PASS (all config tests pass).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config.rs
git commit -m "feat(config): add share_folder alias, fullscreen, and optimization settings"
```

---

### Task 2: CLI Interface Updates

**Files:**

- Modify: `src/cli.rs`

**Interfaces:**

- `LaunchArgs`:
  - `pub windowed: bool` (`#[arg(short, long)]`)
  - `pub optimize: Option<String>` (`#[arg(short = 'O', long)]`)
- `ConfigCommand::Add` and `ConfigCommand::Edit`:
  - `pub optimize: Option<String>`
  - `pub fullscreen: Option<bool>`

- [ ] **Step 1: Update `src/cli.rs`**
Update `LaunchArgs`, `ConfigCommand::Add`, and `ConfigCommand::Edit` in `src/cli.rs`:
- Remove `pub fullscreen: bool` from `LaunchArgs` and add `#[arg(short, long)] pub windowed: bool`
- Remove `pub no_optimize: bool` from `LaunchArgs` and add `#[arg(short = 'O', long)] pub optimize: Option<String>`
- In `ConfigCommand::Add` and `Edit`, replace `optimize_connection: Option<bool>` with `optimize: Option<String>`, and add `#[arg(long)] fullscreen: Option<bool>`.

- [ ] **Step 2: Verify `cli.rs` syntax by checking compiler output**
Run: `cargo check --lib`
Expected: PASS or clean library check (ignoring callers in `main.rs` that will be updated in Task 4).

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(cli): replace --fullscreen with --windowed, add --optimize and remove --no-optimize"
```

---

### Task 3: RDP Profile Generation & Drive Filtering

**Files:**

- Modify: `src/rdp.rs`
- Test: `tests/rdp.rs`

**Interfaces:**

- `OptimizationProfile`: `None`, `Speed`, `Balanced`, `Quality`
- `OptimizationSettings`: struct containing profile and granular overrides
- `RdpProfileOptions`:
  - `fullscreen: bool`
  - `optimization: OptimizationSettings`
  - `dynamic resolution:i:1` and `smart sizing:i:1` always output
- `resolve_drives_to_redirect(shared_drives: Option<&str>, folder_letter: Option<char>) -> Option<String>`:
  - If `folder_letter` is `Some(f)` and `shared_drives` is `None`: returns `Some(f)` (redirects ONLY the virtual drive).
  - If `shared_drives` is `Some("*")`, queries ready Windows drive letters and skips optical/empty drives (DriveType 5 without media).

- [ ] **Step 1: Write failing tests in `tests/rdp.rs`**

```rust
#[test]
fn test_resolve_drives_folder_only() {
    assert_eq!(
        resolve_drives_to_redirect(None, Some('Z')),
        Some("Z:".to_string())
    );
}

#[test]
fn test_build_rdp_content_default_fullscreen_no_optimization() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: true,
        username: Some("Administrator"),
        drives_to_redirect: Some("Z:"),
        optimization: OptimizationSettings::default(),
    };

    let content = build_rdp_file_content(&opts);
    assert!(content.contains("screen mode id:i:2"));
    assert!(content.contains("dynamic resolution:i:1"));
    assert!(content.contains("smart sizing:i:1"));
    assert!(content.contains("drivestoredirect:s:Z:"));
    assert!(!content.contains("connection type:i:6"));
}

#[test]
fn test_build_rdp_content_windowed_speed_optimization() {
    let opts = RdpProfileOptions {
        host: "stg-rdp",
        port: 13389,
        fullscreen: false,
        username: None,
        drives_to_redirect: None,
        optimization: OptimizationSettings::from_profile_name("speed"),
    };

    let content = build_rdp_file_content(&opts);
    assert!(content.contains("screen mode id:i:1"));
    assert!(content.contains("dynamic resolution:i:1"));
    assert!(content.contains("connection type:i:6"));
    assert!(content.contains("compression:i:1"));
    assert!(content.contains("bitmapcachepersistenable:i:1"));
    assert!(content.contains("audiomode:i:2"));
}
```

- [ ] **Step 2: Run tests to verify failure**
Run: `cargo test --test rdp`
Expected: FAIL.

- [ ] **Step 3: Implement `OptimizationProfile`, `OptimizationSettings`, drive resolution & filtering in `src/rdp.rs`**
- Implement `OptimizationProfile` (`None`, `Speed`, `Balanced`, `Quality`).
- Implement `OptimizationSettings` with parsing from profile string and applying granular overrides.
- In `build_rdp_file_content`:
  - Always inject `dynamic resolution:i:1` and `smart sizing:i:1`.
  - Set `screen mode id:i:2` if `opts.fullscreen` is true, else `1`.
  - Inject profile directives according to `OptimizationSettings`.
- In `resolve_drives_to_redirect`:
  - Handle `(None, Some(f)) => Some(f)`.
  - Handle `*` by scanning drives and filtering out unready / DriveType 5 optical drives without media.

- [ ] **Step 4: Run tests to verify they pass**
Run: `cargo test --test rdp`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/rdp.rs tests/rdp.rs
git commit -m "feat(rdp): add dynamic resolution, optimization profiles, and folder-only drive redirection"
```

---

### Task 4: Main Launch & Config Integration

**Files:**

- Modify: `src/main.rs`

**Interfaces:**

- `cmd_launch`:
  - Resolves `fullscreen`: `if args.windowed { false } else { env.fullscreen.unwrap_or(true) }`.
  - Resolves `optimize`: CLI `args.optimize.as_deref().or(env.optimize.as_deref())`.
  - Resolves `shared_drives`: `args.share_drives.as_deref().or(env.shared_drives.as_deref())` (NO forced fallback to `*` if `share_folder` is present).
  - Displays updated information in terminal UI: Fullscreen/Windowed mode, Optimization profile, and Shared folder/drives.
- `ConfigCommand::Add` and `ConfigCommand::Edit`:
  - Accept `optimize` and `fullscreen` options and persist to `EnvironmentConfig`.

- [ ] **Step 1: Update `cmd_launch` and config handling in `src/main.rs`**
Replace `--fullscreen` references with `args.windowed`. Update optimization options handling. Update `print_kv` output to show active display mode and optimization profile.

- [ ] **Step 2: Run full test suite**
Run: `cargo test`
Expected: PASS (all unit and integration tests succeed).

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): integrate default fullscreen, windowed flag, and optimization profiles"
```

---

### Task 5: Documentation & Verification

**Files:**

- Modify: `README.md`
- Modify: `README.dev.md`

- [ ] **Step 1: Update `README.md` and `README.dev.md`**
- Update CLI usage examples (replace `--fullscreen` with `--windowed`, document default fullscreen).
- Document `--optimize <profile>` (`speed`, `balanced`, `quality`, `none`) and granular overrides.
- Document shared folder redirection without empty CD-ROM clutter.

- [ ] **Step 2: Run manual CLI verification commands**
Run: `cargo run -- launch --help`
Run: `cargo run -- config list`
Verify `--windowed` is present, `--no-optimize` is removed, and `--optimize` is documented.

- [ ] **Step 3: Commit documentation updates**

```bash
git add README.md README.dev.md
git commit -m "docs: document default fullscreen, --windowed, and optimization profiles"
```
