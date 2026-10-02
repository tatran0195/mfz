# RDP Shared Folder Fix, Customizable Connection Optimization, and Fullscreen Sizing Design Spec

## Overview

This specification addresses three related improvements to MFZ's Remote Desktop management:

1. **Shared Folder Redirection Fix**: Resolve errors where redirected drives fail with `\\tsclient\<drive> is not accessible ... Attempt to access invalid address` by supporting both `share_folder` and `shared_folder` config aliases, redirecting only the mounted virtual drive when sharing folders, and filtering out unready drives (like empty optical/CD-ROM drives).
2. **Customizable Optimization Profiles**: Switch to unoptimized Windows MSTSC defaults by default, eliminating `--no-optimize`. Introduce configurable profiles (`speed`, `balanced`, `quality`, `none`) with granular overrides (audio, wallpaper, font smoothing) in `mfz.ini` and `--optimize <profile>` on the CLI.
3. **Default Fullscreen & Dynamic Window Resizing**: Make fullscreen the default launch mode. Replace `--fullscreen` with `--windowed` (`-w`), support `fullscreen = false` in `mfz.ini`, and inject `dynamic resolution:i:1` so windowed mode smoothly resizes and maximizes without fixed-resolution distortion.

---

## 1. Drive & Shared Folder Redirection

### 1.1 Root Cause Analysis

- In `mfz.ini`, the section specified `share_folder=...`. However, `src/config.rs` only queried `shared_folder`. Consequently, `env.shared_folder` evaluated to `None`.
- `shared_drives` defaulted to `*`. In Windows RDP, `*` redirects all drives enumerated on the host. On the host, drive `E:` is an empty optical/CD-ROM drive (`DriveType: 5`). When accessed via `\\tsclient\E` in the remote session, Windows threw `Attempt to access invalid address`.

### 1.2 Specifications

- **Key Aliasing**: In `src/config.rs`, support both `share_folder` and `shared_folder`, as well as `share_drives` and `shared_drives`.
- **Targeted Redirection**:
  - When `shared_folder` (or `--share-folder`) is set without an explicit `shared_drives` argument, `resolve_drives_to_redirect` will redirect **only** the mounted virtual drive letter (e.g. `Z:`).
  - When explicit drives are set (e.g., `C:,D:`), append the mapped virtual drive letter.
  - When `*` is explicitly requested, filter the enumerated drive letters against Windows volume readiness or `GetDriveTypeW` to exclude drives with no media (DriveType 5 / CD-ROM without media, or unready volumes).
- **Virtual Drive Mount**:
  - Continue using `FolderDriveGuard` with `subst <letter>: <path>` and reverse drive allocation (`Z` down to `E`).
  - Automatically unmount `subst <letter>: /d` upon RDP process completion or termination.

---

## 2. Customizable Connection Optimization

### 2.1 Behavior & Defaults

- **Default**: Unoptimized (Windows MSTSC default settings). No performance flags are injected unless explicitly requested.
- **Removed**: `--no-optimize` CLI flag.
- **Added**: `--optimize <PROFILE>` (or `-O <PROFILE>`) in `mfz launch` and `mfz config add / edit`.

### 2.2 Optimization Profiles

| Profile | Description | RDP Directives |
| :--- | :--- | :--- |
| `none` (default) | Standard Windows defaults | None |
| `speed` | Maximum responsiveness & throughput | `compression:i:1`<br>`bitmapcachepersistenable:i:1`<br>`connection type:i:6`<br>`networkautodetect:i:0`<br>`bandwidthautodetect:i:1`<br>`audiomode:i:2` (mute)<br>`redirectprinters:i:0`<br>`redirectsmartcards:i:0`<br>`disable wallpaper:i:1`<br>`disable full window drag:i:1`<br>`disable menu anims:i:1`<br>`disable themes:i:0`<br>`allow font smoothing:i:1` |
| `balanced` | Smooth performance with audio & wallpaper | `compression:i:1`<br>`bitmapcachepersistenable:i:1`<br>`connection type:i:6`<br>`networkautodetect:i:1`<br>`bandwidthautodetect:i:1`<br>`audiomode:i:0` (play locally)<br>`redirectprinters:i:0`<br>`redirectsmartcards:i:0`<br>`disable wallpaper:i:0`<br>`disable full window drag:i:0`<br>`disable menu anims:i:1`<br>`disable themes:i:0`<br>`allow font smoothing:i:1` |
| `quality` | Full desktop fidelity with caching | `compression:i:1`<br>`bitmapcachepersistenable:i:1`<br>`connection type:i:6`<br>`audiomode:i:0`<br>`disable wallpaper:i:0`<br>`disable full window drag:i:0`<br>`disable menu anims:i:0`<br>`disable themes:i:0`<br>`allow font smoothing:i:1` |

### 2.3 Granular Configuration in `mfz.ini`

Users can declare a profile and selectively override attributes:

```ini
[stg]
optimize = speed
audio = true           # Overrides audiomode:i:0
wallpaper = true       # Overrides disable wallpaper:i:0
font_smoothing = true  # Overrides allow font smoothing:i:1
```

---

## 3. Display, Fullscreen & Dynamic Window Resizing

### 3.1 Behavior & Defaults

- **Default Launch Mode**: Fullscreen (`fullscreen = true` by default).
- **Removed**: `--fullscreen` CLI flag.
- **Added**: `--windowed` (`-w`) CLI flag to launch in a floating window.

### 3.2 Configuration in `mfz.ini`

- `fullscreen = true | false` (defaults to `true` if omitted).
- `mfz config add / edit` supports `--fullscreen <bool>` (or `--windowed`).

### 3.3 Dynamic Window Resizing

In `src/rdp.rs`, the generated RDP profile directives will be:

```ini
screen mode id:i:<2 if fullscreen else 1>
use multimon:i:0
smart sizing:i:1
dynamic resolution:i:1
```

When `opts.fullscreen` is `false`, MSTSC starts windowed. Thanks to `dynamic resolution:i:1`, users can freely resize or maximize the window, and Windows will dynamically adapt the remote resolution without fixed-dimension letterboxing or stretching.

---

## 4. Components & Interfaces

### 4.1 `src/cli.rs`

- In `LaunchArgs`:
  - Remove `pub fullscreen: bool`
  - Remove `pub no_optimize: bool`
  - Add `#[arg(short, long)] pub windowed: bool`
  - Add `#[arg(short = 'O', long)] pub optimize: Option<String>`
- In `ConfigCommand::Add` & `ConfigCommand::Edit`:
  - Remove `optimize_connection: Option<bool>`
  - Add `optimize: Option<String>`
  - Add `fullscreen: Option<bool>`

### 4.2 `src/config.rs`

- In `EnvironmentConfig`:
  - `pub fullscreen: Option<bool>`
  - `pub optimize: Option<String>`
  - `pub opt_audio: Option<bool>`
  - `pub opt_wallpaper: Option<bool>`
  - `pub opt_font_smoothing: Option<bool>`
- In `from_section`:
  - Read `share_folder` or `shared_folder`
  - Read `share_drives` or `shared_drives`
  - Read `fullscreen`
  - Read `optimize`
  - Read granular overrides (`audio`, `wallpaper`, `font_smoothing`)

### 4.3 `src/rdp.rs`

- `OptimizationProfile` enum (`None`, `Speed`, `Balanced`, `Quality`).
- `OptimizationSettings` struct with granular options.
- `RdpProfileOptions`:
  - `fullscreen: bool` (default true, overridden if `--windowed`)
  - `optimization: OptimizationSettings`
  - Always output `dynamic resolution:i:1` and `smart sizing:i:1`.
- `resolve_drives_to_redirect`:
  - If `shared_folder` provided and `shared_drives` is `None`: return `Some(folder_drive)`.
  - Filter ready drives if `*` is requested.

---

## 5. Testing & Verification Plan

### 5.1 Unit Tests

- `tests/config.rs`:
  - Test parsing `share_folder` and `shared_folder`.
  - Test parsing `fullscreen = true/false`.
  - Test parsing `optimize = speed/balanced/quality/none` and overrides.
- `tests/rdp.rs`:
  - Test `resolve_drives_to_redirect` when folder is specified without drives -> returns only folder drive.
  - Test `build_rdp_file_content` with default settings (fullscreen = true, screen mode 2, optimize = none, dynamic resolution 1).
  - Test `build_rdp_file_content` in windowed mode (screen mode 1, dynamic resolution 1).
  - Test `build_rdp_file_content` with `speed`, `balanced`, and granular overrides.

### 5.2 Manual Verification

- Verify `mfz launch stg --windowed` launches MSTSC in windowed mode with dynamic resizing.
- Verify `mfz launch stg` mounts `share_folder` from `mfz.ini` as a virtual drive letter and redirects it cleanly without empty optical drives.
