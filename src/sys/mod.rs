//! Operating system integrations, filesystem storage, hosts file, and shell associations.

pub mod hosts;
pub mod shell;
pub mod storage;

#[cfg(windows)]
pub mod windows;
