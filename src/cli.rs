use clap::{Parser, Subcommand};

#[derive(Parser)]
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

#[derive(Subcommand)]
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
    Config(Box<ConfigCommand>),

    /// Register the .mfz file association in Windows Shell (double-click to connect)
    Register,

    /// Unregister the .mfz file association from Windows Shell
    Unregister,

    /// Export quick-launch .mfz files for environments
    Export(ExportArgs),
}

// ─── Export args ─────────────────────────────────────────────────────────────

#[derive(Parser)]
pub struct ExportArgs {
    /// Environment name to export (e.g. stg, prd)
    #[arg(conflicts_with = "all")]
    pub environment: Option<String>,

    /// Export all configured environments
    #[arg(short, long)]
    pub all: bool,

    /// Save directly to the Windows Desktop
    #[arg(short, long, conflicts_with = "out")]
    pub desktop: bool,

    /// Export as standalone portable profile (default: true)
    #[arg(short, long, conflicts_with = "pointer")]
    pub standalone: bool,

    /// Export as lightweight pointer file referencing local mfz.ini instead
    #[arg(short, long)]
    pub pointer: bool,

    /// Explicitly include plaintext AWS/TOTP/RDP secrets (use a private destination)
    #[arg(long, conflicts_with = "pointer")]
    pub include_secrets: bool,

    /// Explicitly replace existing exported profile files
    #[arg(long)]
    pub force: bool,

    /// Custom output directory or file path
    #[arg(short, long)]
    pub out: Option<String>,

    /// Path to config file
    #[arg(short, long)]
    pub config: Option<String>,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// List all environments and their details
    List {
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Add a new environment
    Add {
        /// Environment identifier (e.g. stg, prd)
        name: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        aws_access_key_id: Option<String>,
        #[arg(long)]
        aws_secret_access_key: Option<String>,
        #[arg(long)]
        region: Option<String>,
        #[arg(long)]
        serial: Option<String>,
        #[arg(long)]
        secret: Option<String>,
        #[arg(long, conflicts_with = "instance_tag")]
        instance: Option<String>,
        #[arg(long)]
        instance_tag: Option<String>,
        #[arg(long, default_value = "3389", value_parser = clap::value_parser!(u16).range(1..))]
        remote_port: u16,
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
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
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Edit an existing environment (only specified fields will be updated)
    Edit {
        /// Environment identifier (e.g. stg, prd)
        name: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        aws_access_key_id: Option<String>,
        #[arg(long)]
        aws_secret_access_key: Option<String>,
        #[arg(long)]
        region: Option<String>,
        #[arg(long)]
        serial: Option<String>,
        #[arg(long)]
        secret: Option<String>,
        #[arg(long, conflicts_with = "instance_tag")]
        instance: Option<String>,
        #[arg(long)]
        instance_tag: Option<String>,
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
        remote_port: Option<u16>,
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
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
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Remove an environment
    Remove {
        name: String,
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Move plain-text secrets and passwords from mfz.ini to the secure system keyring
    Secure {
        name: String,
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Store an AWS Secret Access Key in the secure system keyring for an environment
    SetAwsSecret {
        /// Environment identifier (e.g. stg, prd)
        name: String,
        /// AWS Secret Access Key (prompted interactively if omitted)
        #[arg(long)]
        secret: Option<String>,
    },
}

// ─── Launch ───────────────────────────────────────────────────────────────────

#[derive(Parser)]
pub struct LaunchArgs {
    /// Environment name defined in mfz.ini (e.g. stg, prd)
    pub environment: Option<String>,

    /// Trust an externally supplied standalone/config-pointer profile after review
    #[arg(long)]
    pub trust_profile: bool,

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
    #[arg(short, long)]
    pub config: Option<String>,

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

#[derive(Parser)]
pub struct ListArgs {
    #[arg(short, long)]
    pub config: Option<String>,
}

// ─── Hosts ────────────────────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum HostsCommand {
    /// Add RDP host entries for environments defined in config (defaults to mfz.ini)
    Add {
        /// Path to config file
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Internal validated snapshot for one explicit elevated hosts update
    #[command(hide = true)]
    Apply {
        #[arg(long = "entry", required = true)]
        entries: Vec<String>,
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

#[derive(Subcommand)]
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

#[derive(Parser)]
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

    /// Explicitly replace an existing secret-bearing PNG
    #[arg(long)]
    pub force: bool,
}

// ─── Register args ────────────────────────────────────────────────────────────

#[derive(Parser)]
pub struct RegisterArgs {
    /// Named AWS SDK profile to use for IAM calls
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

    /// Explicitly replace an existing secret-bearing PNG
    #[arg(long)]
    pub force: bool,

    /// Configuration file for --save (the target environment must already exist)
    #[arg(short, long)]
    pub config: Option<String>,

    /// Save serial + plaintext secret to an existing environment after activation
    #[arg(long, alias = "save-env")]
    pub save: Option<String>,
}

#[cfg(test)]
mod config_origin_tests {
    use super::*;
    #[test]
    fn omitted_and_explicit_default_config_paths_keep_distinct_origins() {
        let omitted = Cli::try_parse_from(["mfz", "list"]).unwrap();
        let TopCommand::List(omitted) = omitted.command else {
            panic!("wrong command");
        };
        assert!(omitted.config.is_none());
        let explicit = Cli::try_parse_from(["mfz", "list", "--config", "mfz.ini"]).unwrap();
        let TopCommand::List(explicit) = explicit.command else {
            panic!("wrong command");
        };
        assert_eq!(explicit.config.as_deref(), Some("mfz.ini"));
    }
}
