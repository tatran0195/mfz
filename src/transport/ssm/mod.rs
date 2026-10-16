//! Native AWS Systems Manager (SSM) WebSocket port-forwarding and protocol stack.

pub mod client;
pub mod flow;
pub(crate) mod lifecycle;
pub mod protocol;
pub mod tunnel;
pub(crate) mod writer;

#[cfg(test)]
pub(crate) mod tests;

pub use client::{start_with_retries, SsmSession};
pub use flow::TunnelOptions;
pub use tunnel::{
    start_port_forward_tunnel, start_port_forward_tunnel_with_options, SessionCredentials,
    SsmTunnelHandle,
};
