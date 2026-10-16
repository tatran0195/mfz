//! Streaming network transport, SMUX multiplexing, and AWS SSM data channels.

pub mod buffer;
pub mod smux;
pub mod ssm;

pub use buffer::retain_bytes;
pub use smux::{SmuxOutbound, SmuxSession, SmuxStream};
pub use ssm::{
    start_port_forward_tunnel, start_port_forward_tunnel_with_options, SessionCredentials,
    SsmSession, SsmTunnelHandle, TunnelOptions,
};
