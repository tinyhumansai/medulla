//! The `hook.report` op: a launched harness telling Medulla what just happened
//! to it.
//!
//! The one op on this socket that carries no authority at all. It cannot start,
//! stop, or read anything — it appends a line to a bounded log — so the checks
//! here are about *attribution* rather than permission: a report is filed under
//! the grant's own session, never under a session the caller names, so one
//! harness cannot write history for another.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::harness_hooks::{HookEvent, HookReport};

use super::super::super::grants::Grant;
use super::super::super::types::{ControlFailure, ErrorKind, FleetOps};

use super::params::{optional_str, required_str};

/// The longest summary a report may carry.
///
/// A hook payload can contain a whole file, and the shim already summarizes —
/// this is the backstop for one that does not, so a runaway line cannot fill
/// the log window with a single entry.
const MAX_SUMMARY: usize = 400;

/// The longest `cwd` or `harnessSessionId` a report may carry.
///
/// Both are free-form strings from the reporting client, with no backstop of
/// their own the way `summary` has. [`HookEventLog`](crate::harness_hooks::HookEventLog)
/// retains hundreds of reports, so an unbounded field here is an easy way to
/// pin an unbounded amount of client-supplied memory.
const MAX_FIELD: usize = 256;

/// Record one lifecycle report, attributed to the caller's own session.
pub(super) fn hook_report(
    ops: &Arc<dyn FleetOps>,
    grant: &Grant,
    params: &Value,
) -> Result<Value, ControlFailure> {
    let event = required_str(params, "event")?;
    let event = HookEvent::from_wire(&event).ok_or_else(|| {
        ControlFailure::new(
            ErrorKind::BadRequest,
            format!("`{event}` is not a lifecycle event Medulla knows"),
        )
    })?;

    let mut report = HookReport::new(grant.session.clone(), event);
    report.at_ms = crate::clock::now_millis();
    report.summary = optional_str(params, "summary")
        .map(|summary| truncate(&summary))
        .unwrap_or_default();
    report.cwd = optional_str(params, "cwd");
    report.harness_session_id = optional_str(params, "harnessSessionId");

    ops.record_hook_event(report);
    Ok(json!({ "recorded": true }))
}

/// Clip `summary` to [`MAX_SUMMARY`] characters, on a character boundary.
fn truncate(summary: &str) -> String {
    if summary.chars().count() <= MAX_SUMMARY {
        return summary.to_string();
    }
    let clipped: String = summary.chars().take(MAX_SUMMARY - 1).collect();
    format!("{clipped}…")
}
