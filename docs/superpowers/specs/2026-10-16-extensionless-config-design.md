# Extensionless Configuration Support Design

**Date**: 2026-10-16  
**Status**: Approved  
**Topic**: Extensionless MFZ Configuration Discovery & Resolution  

## 1. Objective

Modernize MFZ configuration loading to match professional cloud and developer CLI tools (e.g., `git`, `ssh`, `docker`, `aws`). MFZ will support clean extensionless configuration files (`mfz`, `.mfz`, `config`) without requiring legacy `.ini` file extensions, while seamlessly supporting `--config <name>` extension resolution and retaining complete backwards compatibility for existing `mfz.ini` configurations.

---

## 2. Architecture & Design

### 2.1 Default Configuration Discovery Order

When a command runs without an explicit `--config` flag, `ConfigLoader::resolve_default_config_path()` discovers configuration files using the following priority:

1. **Current Working Directory (`.`)**:
   - `./mfz` *(modern clean CLI standard)*
   - `./mfz.ini` *(legacy default for backwards compatibility)*
   - `./.mfz` *(clean project dotfile)*

2. **Application Executable Directory (Portable Distribution)**:
   - `<exe_dir>/mfz`
   - `<exe_dir>/mfz.ini`

3. **Standard User & Platform Configuration Directories**:
   - `$XDG_CONFIG_HOME/mfz/config` *(if `$XDG_CONFIG_HOME` is set)*
   - `~/.config/mfz/config` (or `%USERPROFILE%/.config/mfz/config` on Windows)
   - `~/.mfz/config`
   - `~/.mfz/mfz`
   - `~/.mfz/mfz.ini`
   - `%APPDATA%/mfz/config`
   - `%APPDATA%/mfz/mfz.ini`

4. **Default Creation Target**:
   - If no configuration file exists at any candidate location, the default path falls back to `mfz.ini` (or `mfz` if the directory explicitly contains `mfz`) in the current working directory to ensure predictable persistence behavior for initialization/add commands.

### 2.2 Explicit `--config <path>` Resolution

When the user specifies an explicit `--config <path>` flag on the command line:

1. **Exact Path Priority**:
   - If `<path>` exists as a file (e.g. `mfz`, `custom`, `mfz.ini`, `/etc/mfz`), it is used directly without modification.

2. **Extensionless Resolution**:
   - If `<path>` does **not** exist and has no file extension (e.g. `--config mfz` or `--config stg_conf`):
     - Check if `<path>.ini` exists.
     - If `<path>.ini` exists, resolve to `<path>.ini`.

3. **Missing File Fallback**:
   - If neither `<path>` nor `<path>.ini` exists:
     - Retain `<path>` directly. This ensures:
       - Saving a new file via `mfz config add ... --config custom` creates the exact filename requested.
       - Error messages display the exact path provided by the user.

4. **Directory Isolation Invariant**:
   - Explicit `--config` paths never perform ambient search across other directories (e.g. will not fallback to `~/.mfz`), preserving the security invariants verified by regression tests.

### 2.3 Distinction Between Config and Profile Files

- **Main Configuration (`mfz` / `mfz.ini`)**: Contains multi-environment declarations (`[stg]`, `[prd]`).
- **Session Profiles (`<name>.mfz`)**: Remains the dedicated extension for desktop launcher shortcuts, Windows Shell file associations (`mfz register`), custom icons, and portable session exports.

---

## 3. Implementation Plan

1. **Update `ConfigLoader` in `src/config/loader.rs`**:
   - Refactor `resolve_default_config_path()` to search the prioritized list of candidates.
   - Refactor `from_cli(path: Option<&str>)` to apply extension auto-detection when an explicit path lacks an extension and does not exist.
2. **Update Tests in `tests/config.rs`**:
   - Add test verifying `./mfz` extensionless discovery.
   - Add test verifying `./mfz.ini` fallback when `./mfz` is not present.
   - Add test verifying `--config <name>` auto-resolves `<name>.ini` if present.
   - Add test verifying `--config <name>` retains `<name>` if `<name>` exists.
   - Add test verifying user config paths (`~/.config/mfz/config`, `~/.mfz/config`).
3. **Run Regression Suites**:
   - Run `tests/audit_regressions.rs` and `tools/audit_cli_test.py`.
4. **Documentation**:
   - Update `README.md` and `README.dev.md` to document the new extensionless config convention.

---

## 4. Verification & Testing

- `cargo test --test config`
- `cargo test --test audit_regressions`
- `python tools/audit_cli_test.py --binary target/release/mfz.exe` (or debug binary)
