//! MFZ — AWS MFA + SSM port forwarding + RDP session manager
//!
//! A clean, modular Rust CLI architecture providing:
//! - `auth`: AWS MFA session negotiation, TOTP, IAM virtual device enrollment, Windows Keyring
//! - `aws`: EC2 instance resolution by tag, bounded pagination
//! - `cli`: Top-level command line argument parsing and routing
//! - `commands`: Command execution workflows and orchestrations
//! - `config`: Multi-environment INI profile management and atomic writes
//! - `error`: Application-wide typed errors and exit codes
//! - `qr`: QR code encoding, terminal visualization, image decoding, URI extraction
//! - `rdp`: Remote Desktop configuration generator and Windows connection launcher
//! - `sys`: Platform-specific integrations (hosts file, Windows file association, atomic storage DACL)
//! - `transport`: Streaming native WebSocket tunnel and SMUX v1 multiplexing protocol
//! - `ui`: User interface formatting, spinners, banners, and diagnostics

pub mod auth;
pub mod aws;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod rdp;
pub mod sys;
pub mod transport;
pub mod ui;
pub(crate) mod validation;

// ── Backward-compatible re-exports for existing modules and tests ──────────────
pub use auth::credentials;
pub use auth::enrollment;
pub use auth::mfa;
pub use auth::otp;
pub use auth::qr;
pub use sys::hosts;
pub use sys::shell;
pub use sys::storage;
#[cfg(windows)]
pub use sys::windows;
pub use transport::buffer as transport_buffer;
pub use transport::smux;
pub use transport::ssm;
pub use transport::ssm::flow as ssm_flow;
pub use transport::ssm::protocol as ssm_protocol;
pub use transport::ssm::tunnel as ssm_tunnel;
