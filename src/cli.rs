use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name        = "rdp-launcher",
    version,
    about       = "AWS MFA + SSM port forwarding + RDP session manager",
    long_about  = "\
RDP Launcher manages AWS MFA authentication, SSM port forwarding,
and optional RDP session launch.

Temporary MFA credentials are held in memory only — never written to disk.",
    after_help  = "\
EXAMPLES:
    rdp-launcher launch stg
    rdp-launcher launch prd --fullscreen
    rdp-launcher launch stg --ssm-only
    rdp-launcher list
    rdp-launcher hosts add
    rdp-launcher hosts custom 10.0.0.5 my-server
    rdp-launcher qr decode   path/to/qr.png
    rdp-launcher qr generate --account alice@example.com --issuer MyOrg
    rdp-launcher qr generate --secret JBSWY3DPEHPK3PXP --out totp.png
    rdp-launcher qr register --profile dev --username alice --account alice@example.com",
    propagate_version = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: TopCommand,
}

// ─── Top-level commands ───────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum TopCommand {
    /// Launch an RDP session (MFA → SSM → mstsc)
    Launch(LaunchArgs),

    /// List available environments from the config file
    List(ListArgs),

    /// Manage the system hosts file
    #[command(subcommand)]
    Hosts(HostsCommand),

    /// QR code & MFA device utilities
    #[command(subcommand)]
    Qr(QrCommand),
}

// ─── Launch ───────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct LaunchArgs {
    /// Environment name defined in environment.txt (e.g. stg, prd)
    pub environment: String,

    /// Open RDP in fullscreen mode
    #[arg(short, long)]
    pub fullscreen: bool,

    /// Start SSM tunnel only — skip launching mstsc
    #[arg(short, long = "ssm-only")]
    pub ssm_only: bool,

    /// Override the AWS region for this session
    #[arg(short, long)]
    pub region: Option<String>,

    /// Path to the environment config file
    #[arg(short, long, default_value = "environment.txt")]
    pub config: String,
}

// ─── List ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct ListArgs {
    #[arg(short, long, default_value = "environment.txt")]
    pub config: String,
}

// ─── Hosts ────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum HostsCommand {
    /// Add the default RDP entries (stg-rdp, prd-rdp → 127.0.0.1)
    Add,

    /// Add a single custom hostname entry
    Custom {
        /// IP address to map
        ip: String,
        /// Hostname alias
        hostname: String,
    },
}

// ─── QR ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum QrCommand {
    /// Decode a QR code image and extract the embedded OTP secret
    Decode {
        /// Path to the QR code image (PNG / JPG / BMP …)
        image: String,
    },

    /// Generate a TOTP QR code image (and display it inline in the terminal)
    Generate(GenerateArgs),

    /// Generate a TOTP secret, display QR inline, then register it with AWS IAM
    Register(RegisterArgs),
}

// ─── Generate args ────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct GenerateArgs {
    /// Account label shown in authenticator apps (e.g. alice@example.com)
    #[arg(short, long)]
    pub account: String,

    /// Issuer / organisation name shown in authenticator apps
    #[arg(short, long, default_value = "AWS")]
    pub issuer: String,

    /// Use this existing Base-32 secret instead of generating a new one
    #[arg(long)]
    pub secret: Option<String>,

    /// Save QR code PNG to this path (optional; always shown in terminal)
    #[arg(short, long)]
    pub out: Option<String>,
}

// ─── Register args ────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct RegisterArgs {
    /// AWS CLI profile to use for IAM calls
    #[arg(short, long, default_value = "default")]
    pub profile: String,

    /// IAM username to attach the virtual MFA device to
    #[arg(short, long)]
    pub username: String,

    /// Account label for the TOTP URI (defaults to --username)
    #[arg(short, long)]
    pub account: Option<String>,

    /// Issuer / organisation name
    #[arg(short, long, default_value = "AWS")]
    pub issuer: String,

    /// Save QR code PNG to this path (optional)
    #[arg(short, long)]
    pub out: Option<String>,

    /// AWS region override
    #[arg(short, long)]
    pub region: Option<String>,
}