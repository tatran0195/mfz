# MFZ — The Easiest Way to Connect to AWS Windows Servers 🚀

**MFZ** is a powerful yet simple tool designed for anyone who needs to access Windows servers in AWS. It handles all the complex "under-the-hood" stuff like security tokens (MFA), encrypted tunnels, and server discovery, so you can just focus on your work.

> 🛠️ **Developer?** Looking for technical details, schema, or build instructions? Check out the [Developer Guide](./README.dev.md).

---

> **Repository audit, 2026-10-15:** working remediation is documented in
> [issue inventory](./docs/audit/ISSUES.md), [verification](./docs/audit/VERIFICATION.md)
> and [coverage](./docs/audit/COVERAGE.md). Earlier source ZIPs contain a legacy
> reference config with two potentially genuine MFA seeds (validity unknown).
> Current legacy references were removed; if genuine, replace the corresponding
> devices. Removing source does not revoke seeds or sanitize old archives/history.
> See MFZ-A26; no seed values are reproduced here.


## 🌟 Why use MFZ?

If you've ever found connecting to an AWS Windows server frustrating because of MFA codes, command-line tunnels, or searching for Instance IDs—**MFZ was built for you.**

- 🎯 **No more searching**: Find your server by its name (Tags), not a random ID.
- ⌨️ **Less typing**: Interactive menus let you pick your environment with a single click.
- 🔒 **Credential storage**: Keep your AWS secret keys, MFA secrets, and passwords in the Windows Credential Manager store (protect plaintext files and explicit exports too).
- 🚀 **One-click launch**: One command opens your Remote Desktop client automatically.
- ⚡ **Zero external tools**: Pure-Rust native SSM WebSocket tunnel and direct STS auth—no AWS CLI or `session-manager-plugin` needed!

---

## ⚡ Quick Start (3 Steps)

### 1. Installation

If you have Rust installed, simply run:

```bash
cargo install --path .
```

Or place the compiled `mfz.exe` into any folder in your system `PATH`.

### 2. Add your first Environment

You can configure an environment by editing `mfz` (or `mfz.ini`) directly or using the CLI:

#### Option A: private configuration

Start with [`mfz.ini.example`](./mfz.ini.example). The following values are
**placeholders, not credentials to reuse**. Both ports and the descriptive host
alias are required. Either choose an explicit instance or a unique running-instance
tag; discovery rejects ambiguous matches.

```ini
[my-server]
profile = my-aws-profile
region = ap-northeast-1
instance_tag = Name=MyServerName
remote_port = 3389
local_port = 13389
rdp_host = my-server-rdp
rdp_user = Administrator
```

For direct IAM keys, replace `profile` with `aws_access_key_id` and store its secret
with `mfz config set-aws-secret my-server` (interactive prompt on Windows). For IAM
TOTP, configure `serial` and its private Base-32 seed together, then run
`mfz config secure my-server` to verify storage and remove plaintext. AWS MFA uses
SHA1/six digits/30 seconds; do not use published demonstration seeds. Protect the
file while editing it and never commit it.

#### Option B: CLI

```bash
# Named SDK profile; inherits that profile's region unless overridden:
mfz config add my-server --profile my-aws-profile \
  --instance-tag Name=MyServerName --local-port 13389 --rdp-host my-server-rdp

# Direct key ID without putting its secret into shell history:
mfz config add direct-server --aws-access-key-id YOUR_ACCESS_KEY_ID \
  --region ap-northeast-1 --instance i-0123456789abcdef0 \
  --local-port 23389 --rdp-host direct-rdp
mfz config set-aws-secret direct-server
```

Add refuses to overwrite an existing environment; use `mfz config edit` for
explicit changes. CLI secret arguments remain available, but may be visible in
process arguments/shell history; prefer the prompt or a private config followed
by verified secure migration.

### 3. Launch

Just type `mfz launch`, select your server from the list, and press **Enter**:

```bash
mfz launch
```

Or launch directly by name:

```bash
mfz launch my-server
```

---

## 🛠️ Essential Setup

Before you start, make sure your computer is ready:

1. **Windows Remote Desktop (`mstsc.exe`)**: required for desktop launch. Other platforms can use the native `--ssm-only` path when their credential source is available; Windows keyring operations are not simulated elsewhere.
2. **Zero External Dependencies (Standalone Mode)**: MFZ contains a native Rust SSM WebSocket tunnel and direct STS/EC2 client engine. **No AWS CLI (`aws.exe`) or Session Manager plugin (`session-manager-plugin`) is required!**
3. **Named AWS SDK profiles (optional)**: `profile = <name>` supports standard SDK credential providers, role/SSO caches and optional `credential_process`. The chosen profile supplies its region too, unless config/CLI overrides it. MFZ does not bootstrap a new interactive SSO login itself.

---

## 📖 Daily Usage Guide

### 🗂️ Managed Environments

Don't want to type names? Just run `mfz launch`. A beautiful interactive menu will appear where you can search for and select your server.

### 🛡️ Secure Your Passwords & Keys

By default, MFZ stores settings in `mfz.ini`. To move sensitive data (AWS secret keys, MFA secrets, or RDP passwords) to the secure Windows Credential Manager safe-box:

```bash
mfz config secure my-server
```

You can also set your AWS Secret Key directly into the Windows Credential Manager without writing it to disk:

```bash
mfz config set-aws-secret my-server
```

### 📝 Hostname Shortcuts

Native tunneled RDP always connects to the owned numeric `127.0.0.1:<port>`
endpoint; it does not trust DNS or a pre-existing hosts alias. `rdp_host` is kept
as a descriptive/optional manual alias for existing configurations. Config add
and launch no longer make implicit privileged hosts-file edits.

To **explicitly** manage aliases:

```bash
mfz hosts add --config mfz.ini
mfz hosts custom 127.0.0.1 my-server-rdp
```

Updates validate the complete batch and reject a hostname already mapped to a
different address. Windows elevation is awaited and its result verified, not
reported successful merely because UAC launched a child.

### 🚀 Fast File Transfer & Shared Folders

Clipboard and redirected-drive transfers share the same RDP-over-SSM transport. MFZ's native tunnel uses bounded fair queues, immediate batched writes, and adaptive SSM acknowledgement/retry handling to avoid unbounded backlogs and data loss under transfer load. Drive redirection is supported, but it is not a separate file-transfer channel.

If the **entire remote VM** becomes unresponsive, see the [native production-completion report](./docs/native-production-completion.md), [native transport performance report](./docs/native-transport-performance.md) and [Windows/EC2 investigation checklist](./docs/native-transfer-fix.md). The local transport fixes have automated regression coverage; the machine-wide symptom still needs live validation.

Sharing options:

- **Share a Specific Folder**: Want to share only a designated folder (e.g. `C:\Transfer` or `share_folder` in `mfz.ini`)?

  ```bash
  mfz launch my-server --share-folder C:\Transfer
  ```

  MFZ maps a canonical directory to an available virtual drive, redirects it into RDP and removes only its exact owned mapping on normal exit. `*` expands accessible local drives; abrupt process death is not guaranteed cleanup. `none` disables the explicit drive list, not a separately configured shared folder.
- **Control Shared Drives**: Specify exact drives to share, all drives (`*`), or disable sharing:

  ```bash
  mfz launch my-server --share-drives "C:,D:"
  mfz launch my-server --share-drives none
  ```

- **Fullscreen & Dynamic Window Resizing**: By default, MFZ launches in **fullscreen**. To launch in a floating window, pass `-w` or `--windowed`:

  ```bash
  mfz launch my-server --windowed
  ```

  In windowed mode, MFZ enables dynamic resolution so you can resize or maximize the window anytime, and Windows dynamically adjusts the remote screen resolution.
- **Connection Speed Optimization**: Standard Windows defaults are used by default. Choose an optimization profile to enhance speed:

  ```bash
  mfz launch my-server --optimize speed
  mfz launch my-server --optimize balanced
  ```

  Profiles (`speed`, `balanced`, `quality`, `none`) and granular overrides (`audio`, `wallpaper`, `font_smoothing`) can also be configured in `mfz.ini`.

### 🖱️ Double-Click Quick-Start (`.mfz` Files)

Tired of opening a command prompt every time? MFZ supports custom `.mfz` extension registration and desktop quick-launch files!

#### Multi-Profile Desktop Setup (e.g. STG and PRD)

1. **Register the `.mfz` file extension** in Windows Shell (instant, no admin/UAC required):

   ```bash
   mfz register
   ```

If Windows keeps another default app, choose MFZ through Open with/Settings;
   registration does not bypass protected Windows UserChoice defaults.

2. **Export shortcuts to your Desktop**:

   ```bash
   mfz export --all --desktop
   ```

3. **Double-click to connect**:
   - Review before allowing the file/referenced config to be read, then confirm
     the displayed target, credential use and shared resources before connecting.
   - Do not trust a `.mfz` extension or copied environment name as authorization.
     Unattended use requires explicit `--trust-profile` after independent review:

     ```bash
     mfz launch path/to/reviewed.mfz --trust-profile
     ```

#### Export Options

- `mfz export stg`: Creates `stg.mfz` in the current directory.
- `mfz export --all --desktop`: Creates `.mfz` quick-launch files for all configured profiles directly on your Desktop.
- `mfz export prd --standalone`: Exports portable **settings without secrets** (the default). The destination must supply credentials separately.
- `mfz export prd --pointer --config C:\Private\mfz.ini`: Pins a pointer to the selected config instead of relying on another working directory.
- `--force` explicitly replaces an existing export; otherwise output is no-clobber.
- `--include-secrets` explicitly includes plaintext AWS/MFA/RDP secrets and prints a warning; use a private file/directory and never commit/share it. Existing output DACLs are preserved, so protect those grants too. This cannot be combined with `--pointer`.
- Multiple exports require a directory, never one repeatedly overwritten filename.
- `mfz unregister`: Removes the Windows Shell file association cleanly.

---

## ❓ Frequently Asked Questions

**"Do I need to install AWS CLI or Session Manager Plugin?"**
No AWS CLI or Session Manager plugin is required by the runtime. MFZ uses the native AWS Rust SDK and Rust WebSocket/SSM/smux transport. An optional SDK profile may itself depend on an external credential process or previously authenticated SSO cache; that is provider setup, not an MFZ transport fallback.

**"I use SSO / AWS Identity Center. Does it work?"**
MFZ uses the selected SDK profile and its existing SSO cache/provider support. It does not implement initial browser/device-login bootstrap; authenticate the profile with your approved SSO tooling first. SSO must not be confused with the no-MFA direct/static-key mode.

**"Where are my settings stored?"**
Without `--config`, MFZ discovers configuration files using modern CLI priority:
1. Current working directory: `./mfz` *(modern clean format)*, `./mfz.ini` *(established default)*, or `./.mfz`
2. Application executable directory: `<exe_dir>/mfz`, `<exe_dir>/mfz.ini`
3. User & platform configuration directories: `$XDG_CONFIG_HOME/mfz/config`, `~/.config/mfz/config`, `~/.mfz/config`, `~/.mfz/mfz`, `~/.mfz/mfz.ini`, `%APPDATA%\mfz\config`, or `%APPDATA%\mfz\mfz.ini`.

An **explicit** `--config <path>` accepts extensionless paths (e.g. `--config mfz` or `--config prod` auto-detects `mfz.ini` or `prod.ini` if present) while strictly maintaining directory isolation without falling back to ambient user paths. A pointer's relative config path is relative to the pointer.

Only full-line `#`/`;` comments are comments inside configuration; those characters
inside passwords/drive lists are literal. Significant whitespace, leading quotes
or control characters use JSON-string escaping. Protect the file and its backups;
secure migration does not erase old exports, Git history or filesystem snapshots.

---

## Operational boundaries

- RDP keeps NLA and warns on unverifiable server identity; no silent certificate
  bypass or machine-wide UDP policy change is made. Performance hints in a `.rdp`
  file do **not** prove UDP is disabled. Validate your server identity/policy.
- Automatic RDP password use creates one exact-port, session-persistent credential
  and refuses to overwrite an existing entry. Cleanup checks observed ownership;
  it never deliberately deletes broad/unrelated user entries. Verify MSTSC's
  credential-target behavior on supported Windows versions before deployment.
- Secret-bearing temporary files are private at creation. Windows replacement
  preserves the destination DACL. If replacement/recovery or old-backup cleanup
  fails, inspect the reported paths; a recovery file may contain previous secrets.
  Unsupported ACL/filesystem behavior is an error, not an insecure fallback.
- `mfz qr generate` and `qr decode` intentionally display secrets. PNG writes are
  no-clobber unless `--force` is explicit. Decode input is bounded to 16 MiB,
  dimensions 4096×4096 and decoder allocation 64 MiB.
- `mfz qr register --profile admin --username alice --save existing-env --config mfz.ini`
  validates the existing save target before creating a device. Pending devices get
  bounded cleanup on handled failures; an acknowledged active device is never
  automatically deactivated because local saving failed. `--save` (alias
  `--save-env`) explicitly stores plaintext and warns; then run `config secure`.
- Loopback is not per-user authentication; trust the local machine and retain remote RDP authentication. Unsupported KMS handshake actions fail closed; no silent downgrade/fallback.
- Live AWS/IAM, Windows UAC/Credential Manager/MSTSC and the affected VM remain
  external acceptance gates. Local peers/benchmarks do not prove the whole-VM
  transfer symptom resolved in your deployment. See the verification ledger.

## Development verification

Use current stable Rust (audit checks used 1.99.0; no lower tested-MSRV claim).
The actual-module harness avoids linking the large EC2 SDK for portable regressions
without substituting production implementations:

```bash
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --manifest-path tools/transport-tests/Cargo.toml --locked --features session-sdk,enrollment-sdk
cargo build --locked --bin mfz
python tools/audit_cli_test.py --binary target/debug/mfz
```

The Windows workflow also runs all integration tests and an explicitly invoked,
random-identity real Credential Manager round trip. No live cloud tests are hidden
behind an ordinary unit-test pass.


## 👤 Author & Support

Created by **Waka** — [ta@e-technostar.com](mailto:ta@e-technostar.com)

## 📄 License

Licensed under the MIT License. Safe for personal and commercial use.
