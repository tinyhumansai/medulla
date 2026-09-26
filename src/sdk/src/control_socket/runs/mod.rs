//! Workflow runs a granted harness started, reported back to the Medulla that
//! granted it.
//!
//! A harness reaching `workflow_run` over MCP executes the run *in the MCP
//! subprocess*: the engine, the embedded daemon, and every harness session it
//! dispatches live there, not in the Medulla the operator is looking at. So
//! from the operator's side the most consequential thing a session can do — set
//! a whole workflow going — was invisible until it finished and a run record
//! appeared on disk.
//!
//! This is the channel that closes that gap. The subprocess reports what it
//! started, how the run is progressing, and how it ended; the reports are keyed
//! by the *grant's session*, which is the same key the launcher recorded on the
//! PTY row ([`LaunchSpec::mcp_grant_session`](crate::mcp::attach)). That is what
//! lets the rail draw the run underneath the session that triggered it rather
//! than in a list of runs with no context.
//!
//! Everything here is bounded and best-effort. A report that cannot be
//! delivered never fails the run: the run is the work, and this is the view of
//! it.
//!
//! [`types`] holds the wire and data types; [`registry`] holds the table they
//! accumulate in, along with the retention and retirement rules that bound it.

mod registry;
mod types;

pub use registry::HarnessRunRegistry;
pub use types::{HarnessRun, HarnessRunFrame, HarnessRunStatus, RunReport};

// The retention bounds are the suite's to assert against; nothing in the
// library reads them outside the registry that enforces them.
#[cfg(test)]
pub(crate) use registry::{MAX_FRAMES_PER_RUN, MAX_RUNS_PER_SESSION};
