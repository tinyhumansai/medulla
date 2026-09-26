//! The Hosts surface: `Host → Agents`, the topology the advert is a projection
//! of (spec §2.4).
//!
//! The page used to render the worker roster flat and call each row a host. That
//! was true only while a machine advertised exactly one worker; now one machine
//! declares one entry per agent, so a flat roster is a list of *agents* with the
//! host level collapsed out of it. This module puts the level back: the hosts
//! this machine runs (always present, running or not), then every remote host
//! the roster reaches, each carrying the agents known to be on it.
//!
//! Two sources, deliberately not merged into one:
//!
//! - **declarations** (`[fleet].agentDeclarations`) are the truth for a local
//!   host — an agent exists because it is written down, not because something is
//!   running (spec §2.1);
//! - **the roster** is the truth for a remote host, because a remote host does
//!   not yet share its declared agent list over the link (plan §D1). Until it
//!   does, a remote host shows what the roster knows about it and says so,
//!   rather than pretending this machine declared anything over there.
//!
//! The rows are [`types`]; folding the two sources into the tree is
//! [`projection`].

mod projection;
mod types;

pub use projection::host_rows;
pub use types::{HostAgentRow, HostKind, HostRow};

#[cfg(test)]
mod tests;
