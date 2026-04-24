use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("MFA authentication error: {0}")]
    Mfa(String),

    #[error("SSM session error: {0}")]
    Ssm(String),

    #[error("QR code error: {0}")]
    QrCode(String),

    #[error("Hosts file error: {0}")]
    Hosts(String),

    #[error("RDP error: {0}")]
    Rdp(String),

    #[error("IAM error: {0}")]
    Iam(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 1,
            Self::Mfa(_) => 2,
            Self::Ssm(_) => 3,
            Self::QrCode(_) => 4,
            Self::Hosts(_) => 5,
            Self::Rdp(_) => 6,
            Self::Iam(_) => 7,
            Self::Io(_) => 8,
        }
    }
}
