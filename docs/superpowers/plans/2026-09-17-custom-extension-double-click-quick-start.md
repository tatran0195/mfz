# Custom `.mfz` Extension and Double-Click Quick-Start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable users to double-click `.mfz` files (e.g. `stg.mfz`, `prd.mfz`) from Desktop or File Explorer to launch RDP sessions immediately, with Windows Registry association and export utilities.

**Architecture:**

1. Extend `src/config.rs` to parse `.mfz` files (pointer format, standalone portable format, and file stem fallback).
2. Add `Register`, `Unregister`, and `Export` commands to `src/cli.rs`.
3. Implement Windows Shell registry manipulation and file export generator in `src/shell.rs`.
4. Integrate `.mfz` file resolution into `cmd_launch` and wire up new commands in `src/main.rs`.

**Tech Stack:** Rust 2021, `clap`, `configparser`, Windows Registry (`reg.exe`), `mstsc`.

## Global Constraints

- Target Windows OS.
- File association must operate under `HKCU` (Current User) without requiring admin elevation.
- Pointer files reference local `mfz.ini` keeping secrets in Windows Credential Manager.
- Standalone files contain self-contained configurations for sharing with team members.

---

### Task 1: `.mfz` File Profile Loading & Resolution

**Files:**

- Modify: `src/config.rs`
- Test: `tests/config.rs`

**Interfaces:**

- `ConfigLoader::load_from_file_or_name(&self, input: &str) -> Result<(String, EnvironmentConfig)>`:
  - If `input` ends in `.mfz` or exists as a file:
    - Parse `.mfz` file:
      - If it has `env = <name>` or `environment = <name>`, load that section from `self.path` (`mfz.ini`).
      - If it has standard keys (`profile`, `remote_port`, etc.), parse directly as `EnvironmentConfig`.
      - If empty or single word, use the file stem (e.g. `stg.mfz` -> `stg`) and load from `self.path`.
  - Else load by environment name normally from `self.path`.

- [ ] **Step 1: Write failing tests in `tests/config.rs`**
Add tests for loading pointer `.mfz` files, standalone `.mfz` files, and filename stem fallback.

```rust
#[test]
fn test_config_load_pointer_mfz() {
    let ini_content = r#"
[stg]
profile = dev
instance = i-12345
rdp_host = stg-rdp
remote_port = 3389
local_port = 13389
"#;
    let ini_path = create_temp_config(ini_content);
    let loader = ConfigLoader::new(&ini_path);

    let mfz_file = std::env::temp_dir().join(format!("test_pointer_{}.mfz", std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()));
    fs::write(&mfz_file, "env = stg\n").unwrap();

    let (name, env) = loader.load_from_file_or_name(mfz_file.to_str().unwrap()).unwrap();
    assert_eq!(name, "stg");
    assert_eq!(env.profile, "dev");

    let _ = fs::remove_file(&ini_path);
    let _ = fs::remove_file(&mfz_file);
}

#[test]
fn test_config_load_standalone_mfz() {
    let mfz_content = r#"
profile = prd_profile
instance = i-99999
rdp_host = prd-rdp
remote_port = 3389
local_port = 13390
"#;
    let mfz_file = std::env::temp_dir().join(format!("test_standalone_{}.mfz", std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()));
    fs::write(&mfz_file, mfz_content).unwrap();

    let dummy_ini = std::env::temp_dir().join("non_existent_mfz.ini");
    let loader = ConfigLoader::new(&dummy_ini);

    let (name, env) = loader.load_from_file_or_name(mfz_file.to_str().unwrap()).unwrap();
    assert_eq!(env.profile, "prd_profile");
    assert_eq!(env.local_port, 13390);

    let _ = fs::remove_file(&mfz_file);
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test --test config test_config_load_pointer_mfz`
Expected: FAIL (method `load_from_file_or_name` does not exist).

- [ ] **Step 3: Implement `load_from_file_or_name` in `src/config.rs`**
Implement the file detection, parsing, pointer resolution, and fallback logic.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --test config`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs tests/config.rs
git commit -m "feat(config): add load_from_file_or_name for .mfz files"
```

---

### Task 2: CLI Commands for Extension Management and Export

**Files:**

- Modify: `src/cli.rs`

**Interfaces:**

- `TopCommand`:
  - `Register`: Register `.mfz` file extension
  - `Unregister`: Unregister `.mfz` file extension
  - `Export(ExportArgs)`: Export `.mfz` quick-launch files
- `ExportArgs`:
  - `pub environment: Option<String>`
  - `pub all: bool` (`#[arg(short, long)]`)
  - `pub desktop: bool` (`#[arg(short, long)]`)
  - `pub standalone: bool` (`#[arg(short, long)]`)
  - `pub out: Option<String>` (`#[arg(short, long)]`)
  - `pub config: String` (`#[arg(short, long, default_value = "mfz.ini")]`)

- [ ] **Step 1: Update `src/cli.rs` with `Register`, `Unregister`, and `Export` commands**
Add subcommands to `TopCommand` and define `ExportArgs`.

- [ ] **Step 2: Verify compilation with `cargo check --lib`**
Run: `cargo check --lib`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(cli): add register, unregister, and export subcommands"
```

---

### Task 3: Shell Registration & Export Module

**Files:**

- Create: `src/shell.rs`
- Modify: `src/lib.rs`
- Modify: `src/error.rs` (add `Shell` error variant if needed)

**Interfaces:**

- `register_file_association() -> Result<()>`:
  - Finds current executable path (`std::env::current_exe()`).
  - Finds `icon.ico` path if present next to binary or in current directory.
  - Executes `reg add` commands for `HKCU\Software\Classes\.mfz` and `HKCU\Software\Classes\mfz.session`.
- `unregister_file_association() -> Result<()>`:
  - Executes `reg delete` commands to clean up.
- `export_mfz_file(env_name: &str, config: Option<&EnvironmentConfig>, standalone: bool, dest: &Path) -> Result<PathBuf>`:
  - If `standalone` and `config` is `Some`: writes serialized key-values.
  - Else: writes `env = <env_name>\n`.

- [ ] **Step 1: Create `src/shell.rs`**
Implement the registry commands and file export helper.

- [ ] **Step 2: Export module in `src/lib.rs`**
Add `pub mod shell;` to `src/lib.rs`.

- [ ] **Step 3: Run `cargo check --lib`**
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/shell.rs src/lib.rs src/error.rs
git commit -m "feat(shell): implement windows registry file association and export"
```

---

### Task 4: Main Integration & Double-Click Launch Flow

**Files:**

- Modify: `src/main.rs`

**Interfaces:**

- `cmd_launch`:
  - Checks if `args.environment` is an `.mfz` file or pointer.
  - Loads environment using `loader.load_from_file_or_name`.
- Wire up `TopCommand::Register`, `TopCommand::Unregister`, and `TopCommand::Export`.

- [ ] **Step 1: Wire up launch and new commands in `src/main.rs`**
- In `cmd_launch`: use `loader.load_from_file_or_name`.
- Add handlers for `TopCommand::Register`, `TopCommand::Unregister`, and `TopCommand::Export`.

- [ ] **Step 2: Run complete test suite**
Run: `cargo test`
Expected: PASS.

- [ ] **Step 3: Test CLI manually**
Run: `cargo run -- export --help`
Run: `cargo run -- export stg`
Expected: Creates `stg.mfz`.
Run: `cargo run -- launch stg.mfz --ssm-only`
Expected: Successfully recognizes and launches `stg.mfz`!

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): integrate .mfz double-click launch, register, and export commands"
```

---

### Task 5: Documentation Updates & Final Verification

**Files:**

- Modify: `README.md`
- Modify: `README.dev.md`

- [ ] **Step 1: Update `README.md` and `README.dev.md`**
Document:
- `mfz register`: One-click setup for `.mfz` file association.
- `mfz export --all --desktop`: How to create `stg.mfz` and `prd.mfz` on Desktop.
- Double-clicking `.mfz` files to connect.

- [ ] **Step 2: Run full test suite & clean up any test `.mfz` files**
Run: `cargo test`

- [ ] **Step 3: Commit documentation**

```bash
git add README.md README.dev.md
git commit -m "docs: document .mfz double-click quick-start and export commands"
```
