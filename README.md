# MFZ — The Easiest Way to Connect to AWS Windows Servers 🚀

**MFZ** is a powerful yet simple tool designed for anyone who needs to access Windows servers in AWS. It handles all the complex "under-the-hood" stuff like security tokens (MFA), encrypted tunnels, and server discovery, so you can just focus on your work.

> 🛠️ **Developer?** Looking for technical details, schema, or build instructions? Check out the [Developer Guide](./README.dev.md).

---

## 🌟 Why use MFZ?

If you've ever found connecting to an AWS Windows server frustrating because of MFA codes, command-line tunnels, or searching for Instance IDs—**MFZ was built for you.**

- 🎯 **No more searching**: Find your server by its name (Tags), not a random ID.
- ⌨️ **Less typing**: Interactive menus let you pick your environment with a single click.
- � **Super secure**: Keep your passwords in the Windows Credential Manager, not in text files.
- � **One-click launch**: One command opens your Remote Desktop client automatically.

---

## ⚡ Quick Start (3 Steps)

### 1. Installation

If you have Rust installed, simply run:

```bash
cargo install --path .
```

### 2. Add your first Environment

Tell MFZ about your server. Replace the values with your actual AWS details:

```bash
mfz config add my-server --profile my-aws-profile --instance-tag Name=MyServerName
```

### 3. Launch

Just type `mfz launch`, select your server from the list, and press **Enter**.

```bash
mfz launch
```

---

## 🛠️ Essential Setup

Before you start, make sure your computer is ready:

1. **AWS CLI**: You need the AWS Command Line Interface installed and configured.
2. **SSM Plugin**: This is what lets MFZ "tunnel" through the internet securely. [Download here](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html).
3. **Remote Desktop (mstsc)**: Standard on all Windows PCs.

---

## � Daily Usage Guide

### 🗂️ Managed Environments

Don't want to type names? Just run `mfz launch`. A beautiful interactive menu will appear where you can search for and select your server.

### 🛡️ Secure Your Passwords

By default, MFZ stores settings in a file named `mfz.ini`. To move sensitive data (like MFA secrets or RDP passwords) to the secure Windows safe-box:

```bash
mfz config secure my-server
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

- **Automatic Drive Sharing**: By default, your local drives are shared into the remote desktop. Inside the remote session, open **This PC** in File Explorer or navigate to `\\tsclient\` to drag and drop files directly.
- **Share a Specific Folder**: Want to share only a designated folder (e.g. `C:\Transfer`)?
  ```bash
  mfz launch my-server --share-folder C:\Transfer
  ```
  MFZ automatically maps this folder to a virtual drive letter and shares it, then unmounts it cleanly upon exit.
- **Control Shared Drives**: Specify exact drives to share or disable sharing:
  ```bash
  mfz launch my-server --share-drives "C:,D:"
  mfz launch my-server --share-drives none
  ```
- **Connection Speed Optimization**: Enabled by default (`compression:i:1`, `bitmapcachepersistenable:i:1`, LAN connection type `connection type:i:6`, and disabled audio/printer overhead). Use `--no-optimize` to revert to standard Windows RDP defaults.

---

## ❓ Frequently Asked Questions

**"I use SSO / AWS Identity Center. Does it work?"**
Yes! MFZ automatically detects SSO profiles. Just make sure you are logged in using `aws sso login` and MFZ will handle the rest.

**"Where are my settings stored?"**
Settings are kept in `mfz.ini` in the same folder as the program. You can easily share this file with teammates (just make sure to secure your secrets first!).

---

## 👤 Author & Support

Created by **Waka** — [ta@e-technostar.com](mailto:ta@e-technostar.com)

## 📄 License

Licensed under the MIT License. Safe for personal and commercial use.
