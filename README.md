# MFZ — AWS MFA + SSM Port Forwarding + RDP Session Manager

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**MFZ** (RDP Launcher) is a sleek Rust-powered CLI tool designed to eliminate the friction of connecting to Windows instances in AWS private subnets. It automates the entire chain: MFA authentication, SSM port forwarding, and launching your Remote Desktop client.

---

## ✨ Features

- 🔐 **Integrated MFA**: Seamlessly handles AWS MFA device authentication with in-memory credential caching.
- 🚀 **One-Command Launch**: Bridges the gap from `mfz launch` to a live RDP session in seconds.
- 🏗️ **SSM Tunneling**: Automatically establishes secure AWS Systems Manager (SSM) port-forwarding tunnels.
- 📱 **Built-in TOTP Utilities**:
  - **Registration**: Link new Virtual MFA devices to IAM users directly from the CLI.
  - **Generation**: Show scannable QR codes in your terminal (using Unicode half-blocks).
  - **Decoding**: Scan existing QR code images to extract TOTP secrets.
- 📝 **Hosts Management**: Quickly sync your local `hosts` file with aliases like `stg-rdp` or `prd-rdp`.
- 🎨 **Modern UX**: Professional terminal UI with spinners, progress feedback, and color-coded logging.

---

## 🛠️ Prerequisites

Before using MFZ, ensure you have the following installed:

1. **AWS CLI**: Configured with at least one named profile.
2. **AWS Session Manager Plugin**: Required for SSM port forwarding ([Install Guide](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html)).
3. **mstsc (Windows)**: The default Remote Desktop Connection client.

---

## 🚀 Installation

Install from source using Cargo:

```bash
# Clone the repository
git clone https://github.com/youruser/mfz.git
cd mfz

# Build and install
cargo install --path .
```

---

## ⚙️ Configuration

MFZ uses a simple INI-style configuration file (default: `mfz.ini`) to manage your target instances.

### Example `mfz.ini`

```ini
[stg]
profile     = my-aws-staging
serial      = arn:aws:iam::123456789012:mfa/waka-totp
secret      = JBSWY3DPEHPK3PXP
instance    = i-0123456789abcdef0
remote_port = 3389
local_port  = 13389
rdp_host    = stg-rdp

[prd]
profile     = my-aws-production
serial      = arn:aws:iam::123456789012:mfa/waka-totp
secret      = JBSWY3DPEHPK3PXP
instance    = i-0fedcba9876543210
remote_port = 3389
local_port  = 23389
rdp_host    = prd-rdp
```

| Key | Description |
| :--- | :--- |
| `profile` | The AWS CLI profile to use. |
| `serial` | Your virtual MFA device ARN. |
| `secret` | The Base-32 TOTP secret seed for MFA. |
| `instance` | The AWS Instance ID of the target server. |
| `remote_port` | The port on the instance (default RDP is 3389). |
| `local_port` | The local port to bind the tunnel to. |
| `rdp_host` | The hostname to use in the RDP client (use `mfz hosts add` to setup). |

---

## 📖 Usage Reference

### Launching a Session

Connect to an environment defined in your config:

```bash
mfz launch stg
mfz launch prd --fullscreen
mfz launch stg --ssm-only
```

### Managing Hosts

Map your local ports to friendly hostnames:

```bash
# Add default stg-rdp and prd-rdp entries to 127.0.0.1
mfz hosts add

# Add a custom entry
mfz hosts custom 127.0.0.1 my-custom-server
```

### MFA & QR Utilities

```bash
# Register a NEW MFA device in AWS IAM and generate a secret
mfz qr register --profile dev --username waka

# Build a TOTP QR for an existing secret and show it in terminal
mfz qr generate --account waka@example.com --secret JBSWY3DPEHPK3PXP --out qr.png

# Extract a secret from a QR code image
mfz qr decode path/to/qr.png
```

---

## 🤝 Contribution

Contributions are welcome! Please feel free to submit a Pull Request.

## 👤 Author

**Waka** — [ta@e-technostar.com](mailto:ta@e-technostar.com)

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
