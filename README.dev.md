# MFZ Developer Documentation 🛠️

This document contains technical details, architecture overviews, and advanced configuration for the **MFZ** tool. For a general overview and quick start, please see [README.md](./README.md).

---

## 🏗️ Technical Architecture

MFZ is built in Rust to provide a high-performance, safe, and portable binary. It orchestrates three primary AWS services:

1. **IAM/STS**: For session token acquisition and identity verification.
2. **SSM (Systems Manager)**: For creating secure WebSocket-based tunnels for port forwarding.
3. **EC2**: For dynamic instance discovery and state verification.

### Flow Diagram

```mermaid
graph TD
    A[CLI: launch name] --> B[Resolve Config]
    B --> C{Auth Type?}
    C -->|MFA| D[Generate TOTP + STS GetSessionToken]
    C -->|SSO| E[SDK SSO Credential Provider]
    D --> F[Resolved Credentials]
    E --> F
    F --> G{Instance Info?}
    G -->|ID| H[Start SSM Session]
    G -->|Tag| I[EC2: DescribeInstances by Tag]
    I --> H[Start SSM Session]
    H --> J[SSM Port-Forward Tunnel]
    J --> K[Launch mstsc.exe]
    K --> L[Inject Credentials via cmdkey]
```

---

## ⚙️ Advanced Configuration

### `mfz.ini` Full Schema

| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `aws_access_key_id` | String | None | AWS Access Key ID for 100% standalone mode (no `~/.aws/credentials` needed). |
| `aws_secret_access_key` | String | None | AWS Secret Access Key. *Recommendation: Store in Keyring via `mfz env secure` or `mfz env set-aws-secret`*. |
| `region` | String | `ap-northeast-1` | AWS Region for the environment (overridable via `--region`). |
| `profile` | String | None | Legacy fallback: AWS profile name in `~/.aws/credentials`. Required if `aws_access_key_id` omitted. |
| `auth_type` | Enum | Auto | `Mfa` or `Sso`. Inferred by presence of `serial`. |
| `serial` | String | None | IAM Virtual MFA ARN (e.g., `arn:aws:iam::...:mfa/user`). |
| `secret` | String | None | Base-32 shared secret. *Recommendation: Use `mfz env secure`*. |
| `instance` | String | None | Target AWS Instance ID. |
| `instance_tag` | String | None | Discovery query (e.g., `Name=Jump-Host`). |
| `remote_port` | u16 | 3389 | The port listening on the target instance. |
| `local_port` | u16 | 3389 | The local port to bind the SSM tunnel to. |
| `rdp_host` | String | (Required) | Friendly name for the RDP connection. |
| `rdp_user` | String | None | Auto-fill username for RDP. |
| `rdp_password` | String | None | Auto-fill password (stored in Windows Credential Manager if secured). |
| `shared_drives` / `share_drives` | String | None | Local drives to redirect (`*`, `C:;D:`, or `none`). |
| `shared_folder` / `share_folder` | String | None | Dedicated local directory path to map and redirect via virtual drive. |
| `fullscreen` | Boolean | `true` | Open RDP session in fullscreen mode. Set `false` for windowed mode. |
| `optimize` | String | `none` | Optimization profile: `speed`, `balanced`, `quality`, or `none`. |
| `audio` | Boolean | Profile default | Override audio redirection (`true` = play locally, `false` = mute). |
| `wallpaper` | Boolean | Profile default | Override desktop wallpaper display (`true` = show, `false` = disable). |
| `font_smoothing` | Boolean | `true` | Override clear font smoothing. |

---

## 🔒 Security Implementation

### Keyring Integration

MFZ uses the `keyring` crate with native Windows Credential Manager support (`windows-native`).

- **Namespace**: `mfz-cli`
- **Keys**: `totp-secret-<env>`, `rdp-password-<env>`, and `aws-secret-<env>`
- **Behavior**: Plain-text values in `mfz.ini` act as overrides. Running `mfz env secure <env>` migrates all plain-text secrets (AWS secret key, TOTP secret, RDP password) to the Windows Credential Manager and clears them from `mfz.ini`. You can also set the AWS secret key directly via `mfz env set-aws-secret <env>`.

### MFA Safety Margin

To prevent session expiration during login, MFZ implements a **Safety Margin** (default: 5s). If the current TOTP window is about to expire, the tool will pause and wait for the next window to ensure a fresh 30-second token is used for the AWS API call.

---

## 🖥️ Windows Shell Association (`.mfz`)

MFZ registers custom file associations in the Current User registry hive (`HKCU\Software\Classes`), requiring zero administrative rights or UAC prompts:

- `HKCU\Software\Classes\.mfz` -> `(Default) = "mfz.session"`
- `HKCU\Software\Classes\mfz.session` -> `(Default) = "MFZ Remote Desktop Profile"`
- `HKCU\Software\Classes\mfz.session\DefaultIcon` -> `(Default) = "<mfz.exe or icon.ico>"`
- `HKCU\Software\Classes\mfz.session\shell\open\command` -> `(Default) = "\"<mfz.exe>\" launch \"%1\""`

### File Resolution Strategy

When `mfz launch <input>` runs, it checks whether `<input>` is a file or environment name:
1. **Pointer File**: If the file defines `env = <name>` or `environment = <name>`, it loads section `[<name>]` from `mfz.ini`.
2. **Standalone Profile**: If the file defines configuration keys directly (`profile = ...`, `instance = ...`, `rdp_host = ...`), MFZ loads it as a standalone `EnvironmentConfig` without needing `mfz.ini`.
3. **File Stem Fallback**: If the file is 0-bytes or has no keys, MFZ extracts the file stem (e.g. `stg.mfz` -> `stg`) and loads `[stg]` from `mfz.ini`.
4. **Environment Name**: Standard lookup from `mfz.ini`.

---

## 🛠️ Development & Building

### Prerequisites

- Stable Rust (v1.70+)
- `cargo-edit` (optional, for dependency management)

### Build Commands

```bash
# Debug build
cargo build

# Production build (optimized with icon resources)
cargo build --release

# Run linting
cargo clippy

# Formatting
cargo +nightly fmt
```

### Windows Resources

The project uses `winres` to embed version metadata and the `icon.png` into the `mfz.exe` binary. Configuration is handled in `build.rs`.

---

## 👤 Author

**Waka** — [ta@e-technostar.com](mailto:ta@e-technostar.com)
