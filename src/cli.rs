use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name        = "mfz",
    version,
    about       = "AWS MFA + SSM port forwarding + RDP session manager",
    long_about  = "\
RDP Launcher manages AWS MFA authentication, SSM port forwarding,
and optional RDP session launch.

Temporary MFA credentials are held in memory only — never written to disk.",
    after_help  = "\
EXAMPLES:
    mfz launch stg
    mfz launch prd --fullscreen
    mfz launch stg --ssm-only
    mfz list
    mfz config list
    mfz config add my-env --profile dev --instance i-0123... --local-port 13389 --rdp-host my-host
    mfz hosts add
    mfz qr decode   path/to/qr.png
    mfz qr register --profile dev --username alice",
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

    /// Manage environment configurations in mfz.ini
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// List all environments and their details
    List {
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
    /// Add a new environment
    Add {
        name: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        serial: String,
        #[arg(long)]
        secret: String,
        #[arg(long)]
        instance: String,
        #[arg(long, default_value = "3389")]
        remote_port: u16,
        #[arg(long)]
        local_port: u16,
        #[arg(long)]
        rdp_host: String,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
    /// Edit an existing environment (only specified fields will be updated)
    Edit {
        name: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        serial: Option<String>,
        #[arg(long)]
        secret: Option<String>,
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        remote_port: Option<u16>,
        #[arg(long)]
        local_port: Option<u16>,
        #[arg(long)]
        rdp_host: Option<String>,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
    /// Remove an environment
    Remove {
        name: String,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
}

// ─── Launch ───────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct LaunchArgs {
    /// Environment name defined in mfz.ini (e.g. stg, prd)
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
    #[arg(short, long, default_value = "mfz.ini")]
    pub config: String,
}

// ─── List ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct ListArgs {
    #[arg(short, long, default_value = "mfz.ini")]
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

    /// Automatically save the new serial and secret to this environment in mfz.ini
    #[arg(long)]
    pub save: Option<String>,
}