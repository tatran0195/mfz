use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "mfz",
    version,
    about = "AWS MFA + SSM port forwarding + RDP session manager",
    long_about = "\
RDP Launcher manages AWS MFA authentication, SSM port forwarding,
and optional RDP session launch.

Temporary MFA credentials are held in memory only — never written to disk.",
    after_help = "\
EXAMPLES:
    mfz launch stg
    mfz launch prd --windowed
    mfz launch stg --ssm-only
    mfz list
    mfz config list
    mfz config add my-env --profile dev --instance i-0123... --local-port 13389 --rdp-host my-host
    mfz hosts add
    mfz qr decode   path/to/qr.png
    mfz qr register --profile dev --username alice",
    propagate_version = true
)]
pub struct Cli {
    /// Enable file logging (mfz.log)
    #[arg(short, long)]
    pub log: bool,

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

    /// Register the .mfz file association in Windows Shell (double-click to connect)
    Register,

    /// Unregister the .mfz file association from Windows Shell
    Unregister,

    /// Export quick-launch .mfz files for environments
    Export(ExportArgs),
}

// ─── Export args ─────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct ExportArgs {
    /// Environment name to export (e.g. stg, prd)
    pub environment: Option<String>,

    /// Export all configured environments
    #[arg(short, long)]
    pub all: bool,

    /// Save directly to the Windows Desktop
    #[arg(short, long)]
    pub desktop: bool,

    /// Export as standalone portable profile (default: true)
    #[arg(short, long, default_value_t = true)]
    pub standalone: bool,

    /// Export as lightweight pointer file referencing local mfz.ini instead
    #[arg(short, long)]
    pub pointer: bool,

    /// Custom output directory or file path
    #[arg(short, long)]
    pub out: Option<String>,

    /// Path to config file
    #[arg(short, long, default_value = "mfz.ini")]
    pub config: String,
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
        /// Environment identifier (e.g. stg, prd)
        name: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        serial: Option<String>,
        #[arg(long)]
        secret: Option<String>,
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        instance_tag: Option<String>,
        #[arg(long, default_value = "3389")]
        remote_port: u16,
        #[arg(long)]
        local_port: u16,
        #[arg(long)]
        rdp_host: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        pass: Option<String>,
        #[arg(long)]
        shared_drives: Option<String>,
        #[arg(long)]
        shared_folder: Option<String>,
        #[arg(long)]
        fullscreen: Option<bool>,
        #[arg(long)]
        optimize: Option<String>,
        #[arg(long, hide = true)]
        optimize_connection: Option<bool>,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
    /// Edit an existing environment (only specified fields will be updated)
    Edit {
        /// Environment identifier (e.g. stg, prd)
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
        instance_tag: Option<String>,
        #[arg(long)]
        remote_port: Option<u16>,
        #[arg(long)]
        local_port: Option<u16>,
        #[arg(long)]
        rdp_host: Option<String>,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        pass: Option<String>,
        #[arg(long)]
        shared_drives: Option<String>,
        #[arg(long)]
        shared_folder: Option<String>,
        #[arg(long)]
        fullscreen: Option<bool>,
        #[arg(long)]
        optimize: Option<String>,
        #[arg(long, hide = true)]
        optimize_connection: Option<bool>,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
    /// Remove an environment
    Remove {
        name: String,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
    /// Move plain-text secrets and passwords from mfz.ini to the secure system keyring
    Secure {
        name: String,
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },
}

// ─── Launch ───────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
pub struct LaunchArgs {
    /// Environment name defined in mfz.ini (e.g. stg, prd)
    pub environment: Option<String>,

    /// Open RDP in windowed mode (overrides fullscreen default)
    #[arg(short, long)]
    pub windowed: bool,

    /// Start SSM tunnel only — skip launching mstsc
    #[arg(short, long = "ssm-only")]
    pub ssm_only: bool,

    /// Override the AWS region for this session
    #[arg(short, long)]
    pub region: Option<String>,

    /// Path to the environment config file
    #[arg(short, long, default_value = "mfz.ini")]
    pub config: String,

    /// Override shared drives for this session (e.g. *, C:, or "none")
    #[arg(long = "share-drives")]
    pub share_drives: Option<String>,

    /// Share a specific local folder into the session (mapped to a virtual drive)
    #[arg(long = "share-folder")]
    pub share_folder: Option<String>,

    /// Connection optimization profile (speed, balanced, quality, none)
    #[arg(short = 'O', long)]
    pub optimize: Option<String>,
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
    /// Add RDP host entries for environments defined in config (defaults to mfz.ini)
    Add {
        /// Path to config file
        #[arg(short, long, default_value = "mfz.ini")]
        config: String,
    },

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
