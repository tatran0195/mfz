# RDP Shared Drive/Folder & Connection Speed Optimization Design

## Overview

MFZ provides automated AWS MFA authentication and SSM port-forwarding to Windows EC2 instances, launching Remote Desktop (`mstsc.exe`). However, previously MFZ launched `mstsc /v:{host}:{port}` directly without an `.rdp` profile. Consequently:

- Drive redirection was unconfigured / disabled, preventing fast and reliable file transfer between the local machine and the remote Windows desktop (forcing users into slow, memory-intensive clipboard copy-paste).
- Default RDP connection profiles were unoptimized for tunneled localhost connections, resulting in unnecessary UI latency and network bandwidth overhead.

This design introduces dynamic `.rdp` profile generation with drive/folder redirection and performance tuning, configurable per environment and overrideable via CLI flags.

---

## Architecture & Components

```
User CLI: mfz launch <env> [--share-drives] [--share-folder] [--no-optimize]
                  │
                  ▼
          Resolve Configuration (mfz.ini + CLI overrides)
                  │
                  ▼
          AWS MFA & SSM Port Forwarding Tunnel (127.0.0.1:local_port)
                  │
                  ▼
         Prepare Drive / Folder Redirection
         ├─ Specific folder? Mount local virtual drive (subst Z: "<folder>")
         └─ Build drive list (e.g., *, C:;D:, or Z:)
                  │
                  ▼
         Generate Optimized Temporary .rdp Profile
         ├─ Target host & port (rdp_host:local_port)
         ├─ drivestoredirect:s:<drives>
         ├─ Performance tuning (compression, bitmap caching, LAN profile)
         └─ Write to %TEMP%\mfz-<env>-<timestamp>.rdp
                  │
                  ▼
         Inject Credentials (cmdkey) & Launch mstsc <temp.rdp>
                  │
                  ▼
         Teardown & Cleanup (Drop / Exit)
         ├─ Delete %TEMP%\mfz-*.rdp
         ├─ Unmount virtual drive (subst Z: /d)
         ├─ Clean up cmdkey credentials
         └─ Terminate SSM tunnel process
```

---

## Configuration & CLI Schema

### 1. `mfz.ini` Schema Updates

Each environment section supports the following optional keys:
- `shared_drives`: (String, default: `*`) Drive letters to redirect into the remote desktop session.
  - `*`: Redirect all local drives.
  - `C:`, `C:;D:`: Redirect specific drives.
  - `none`: Disable drive redirection.
- `shared_folder`: (String, optional) Absolute path to a specific local directory (e.g. `C:\Users\Admin\Transfer`). When set, MFZ mounts this directory to an available virtual drive letter and shares it.
- `optimize_connection`: (Boolean, default: `true`) Whether to apply RDP compression, persistent bitmap caching, and bandwidth optimization flags.

### 2. CLI Updates

#### `mfz launch`

* `--share-drives <DRIVES>`: Override shared drives for this session (e.g. `*`, `C:,D:`, `none`).
- `--share-folder <PATH>`: Provide an ad-hoc local folder path to share for this session.
- `--no-optimize`: Disable connection performance tuning (defaults to standard Windows defaults).

#### `mfz config add` and `mfz config edit`

* `--shared-drives <DRIVES>`: Save `shared_drives` in `mfz.ini`.
- `--shared-folder <PATH>`: Save `shared_folder` in `mfz.ini`.
- `--optimize-connection <true|false>`: Set `optimize_connection` flag.

---

## Dynamic `.rdp` Profile Specification

The generated `.rdp` file contains standard Windows RDP directives:

```ini
full address:s:{rdp_host}:{local_port}
screen mode id:i:{2 if fullscreen else 1}
use multimon:i:0
smart sizing:i:1
drivestoredirect:s:{resolved_drives}
redirectclipboard:i:1
redirectprinters:i:0
redirectsmartcards:i:0
audiomode:i:2
compression:i:1
bitmapcachepersistenable:i:1
connection type:i:6
networkautodetect:i:0
bandwidthautodetect:i:1
disable wallpaper:i:1
disable full window drag:i:1
disable menu anims:i:1
disable themes:i:0
allow font smoothing:i:1
prompt for credentials:i:0
```

When `--no-optimize` is set or `optimize_connection: false`, the performance flags are omitted, preserving Windows RDP defaults.

---

## Folder Mapping via `subst`

When a `shared_folder` is configured:

1. MFZ checks existing occupied drive letters (A: through Z:).
2. Picks the first available drive letter in reverse order (starting from `Z:` down to `E:`).
3. Runs `subst <Drive>: "<Folder>"`.
4. Appends `<Drive>:` to the `.rdp` `drivestoredirect` list.
5. In remote Windows Explorer, the folder is accessible at `\\tsclient\<Drive>` and under "This PC".
6. On session exit or cancellation, MFZ executes `subst <Drive>: /d`.

---

## Teardown & Safety

Cleanup of all session artifacts is guaranteed:
- Virtual drive letter unmounted via `subst /d`.
- Temporary `.rdp` file deleted from `%TEMP%`.
- Cached credentials in `cmdkey` deleted.
- SSM port forwarding process killed.

---

## Testing & Verification

1. **Unit Tests**:
   - Serialization & deserialization of `shared_drives`, `shared_folder`, `optimize_connection` in `EnvironmentConfig`.
   - Generation of `.rdp` content string with various combinations of drive sharing and optimization flags.
   - Virtual drive letter allocator unit logic.
2. **Integration Tests**:
   - CLI argument parsing for `launch`, `config add`, and `config edit`.
   - Verification of `mfz.ini` updates.
3. **End-to-End Build**:
   - `cargo clippy`, `cargo fmt`, `cargo test`.
