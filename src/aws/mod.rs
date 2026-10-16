//! AWS integration helpers, EC2 instance resolution by tag, and pagination.

pub mod ec2;
pub(crate) mod instance_selection;

pub use ec2::resolve_instance_by_tag;
