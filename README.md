# MFZ — The Easiest Way to Connect to AWS Windows Servers 🚀

**MFZ** is a powerful yet simple tool designed for anyone who needs to access Windows servers in AWS. It handles all the complex "under-the-hood" stuff like security tokens (MFA), encrypted tunnels, and server discovery, so you can just focus on your work.

> 🛠️ **Developer?** Looking for technical details, schema, or build instructions? Check out the [Developer Guide](./README.dev.md).

---

## 🌟 Why use MFZ?

If you've ever found connecting to an AWS Windows server frustrating because of MFA codes, command-line tunnels, or searching for Instance IDs—**MFZ was built for you.**

- 🎯 **No more searching**: Find your server by its name (Tags), not a random ID.
- ⌨️ **Less typing**: Interactive menus let you pick your environment with a single click.
- 🔒 **Super secure**: Keep your AWS secret keys, MFA secrets, and passwords in the Windows Credential Manager safe-box.
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

You can configure an environment by editing `mfz.ini` directly or using the CLI:

#### Option A: Direct in `mfz.ini` (Recommended — 100% Standalone)
Create or edit `mfz.ini` in the same directory as `mfz.exe`:

```ini
[my-server]
region = ap-northeast-1
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
serial = arn:aws:iam::123456789012:mfa/username
secret = JBSWY3DPEHPK3PXP
instance_tag = Name=MyServerName
rdp_host = my-server-rdp
rdp_user = Administrator
```

#### Option B: Via CLI Command
```bash
# Standalone direct credentials (no ~/.aws files needed):
mfz config add my-server \
  --aws-access-key-id AKIAIOSFODNN7EXAMPLE \
  --aws-secret-access-key wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY \
  --region ap-northeast-1 \
  --instance-tag Name=MyServerName

# Or using an existing ~/.aws/credentials profile:
mfz config add my-server --profile my-aws-profile --instance-tag Name=MyServerName
```

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

1. **Remote Desktop (`mstsc.exe`)**: Standard on all Windows PCs.
2. **Zero External Dependencies (Standalone Mode)**: MFZ contains a native Rust SSM WebSocket tunnel and direct STS/EC2 client engine. **No AWS CLI (`aws.exe`) or Session Manager plugin (`session-manager-plugin`) is required!**
3. **AWS Profile Fallback (Optional)**: If you already use `~/.aws/credentials` or AWS IAM Identity Center (SSO), MFZ supports `profile = <name>` as a fallback.

---

## 📖 Daily Usage Guide

### 🗂️ Managed Environments

Don't want to type names? Just run `mfz launch`. A beautiful interactive menu will appear where you can search for and select your server.

### 🛡️ Secure Your Passwords & Keys

By default, MFZ stores settings in `mfz.ini`. To move sensitive data (AWS secret keys, MFA secrets, or RDP passwords) to the secure Windows Credential Manager safe-box:

```bash
mfz env secure my-server
```

You can also set your AWS Secret Key directly into the Windows Credential Manager without writing it to disk:
```bash
mfz env set-aws-secret my-server
```

### 📝 Hostname Shortcuts

Tired of typing `127.0.0.1:13389`? MFZ uses friendly names like `stg-rdp` or `prd-rdp`:

- When you run `mfz config add`, MFZ **automatically adds** the hostname shortcut to your system hosts file (prompting for elevation via UAC if needed).
- To sync all hostname shortcuts for existing environments in your configuration:

```bash
mfz hosts add
```

### 🚀 Fast File Transfer & Shared Folders

Transferring files through RDP clipboard copy-paste can be slow and prone to freezing. MFZ natively shares your local drives and optimizes RDP performance for ultra-fast transfers:

- **Share a Specific Folder**: Want to share only a designated folder (e.g. `C:\Transfer` or `share_folder` in `mfz.ini`)?
  ```bash
  mfz launch my-server --share-folder C:\Transfer
  ```
  MFZ automatically maps this folder to a virtual drive letter (e.g. `Z:`) and shares it cleanly into `\\tsclient`, filtering out unmounted/empty optical drives, then unmounts it upon exit.
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
2. **Export shortcuts to your Desktop**:
   ```bash
   mfz export --all --desktop
   ```
3. **Double-click to connect**:
   - `stg.mfz` on your Desktop connects directly to STG!
   - `prd.mfz` on your Desktop connects directly to PRD!

#### Export Options
- `mfz export stg`: Creates `stg.mfz` in the current directory.
- `mfz export --all --desktop`: Creates `.mfz` quick-launch files for all configured profiles directly on your Desktop.
- `mfz export prd --standalone`: Exports a self-contained portable profile containing full connection details.
- `mfz unregister`: Removes the Windows Shell file association cleanly.

---

## ❓ Frequently Asked Questions

**"Do I need to install AWS CLI or Session Manager Plugin?"**
No! MFZ is completely self-contained. It uses pure Rust for STS token exchange and establishes native WebSocket port-forwarding tunnels directly to AWS Systems Manager.

**"I use SSO / AWS Identity Center. Does it work?"**
Yes! MFZ automatically detects SSO profiles. Just make sure you are logged in using `aws sso login` and MFZ will handle the rest.

**"Where are my settings stored?"**
Settings are kept in `mfz.ini` in the same folder as the program. You can easily share this file with teammates (just make sure to secure your secrets first!).

---

## 👤 Author & Support

Created by **Waka** — [ta@e-technostar.com](mailto:ta@e-technostar.com)

## 📄 License

Licensed under the MIT License. Safe for personal and commercial use.
