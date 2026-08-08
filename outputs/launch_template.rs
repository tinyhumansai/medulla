//! Session planning and spawn: find an idle session or launch a fresh harness.
//!
//! [`super::PtySessionExecutor::session_for`] applies the lifetime-class rules
//! that decide whether to reuse an idle session or start a new one, and
//! [`super::PtySessionExecutor::spawn_env`] builds the environment and
//! arguments a fresh harness spawns with.

use std::collections::HashMap;

use medulla::daemon::providers::RunTaskOptions;
use medulla::sessions::SessionClass;

use super::super::pty::{LaunchSpec, SessionControl, SessionOrigin};
use super::types::{OpenedSession, PtySessionExecutor, SessionPlan};

impl PtySessionExecutor {
LINE_MARKER_LAUNCH_BODY
}
