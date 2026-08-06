//! What a fired hook tells Medulla, and where Medulla keeps it.
//!
//! The built-in hooks ([`super::builtin`]) exist to make a harness on a pty
//! legible. This is the other half of that: the shape one report takes on the
//! control socket, and the bounded log the TUI reads them back out of.
//!
//! # What travels
//!
//! A one-line [`HookReport::summary`], not the harness's payload. The payload
//! carries the operator's prompt text, tool inputs, and file contents, and none
//! of that needs to cross a socket for Medulla to know a turn ended. The shim
//! summarizes at the source (see the `medulla hook` command) and the raw payload
//! never leaves the harness's own process tree.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::types::HookEvent;

/// How many reports the log keeps.
///
/// A busy session fires a `PostToolUse` per tool call, so this is minutes of
/// history rather than hours — which is what the Hooks page shows. It is a
/// live-activity view, not an audit trail.
const CAPACITY: usize = 500;

/// One lifecycle event, as the harness reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookReport {
    /// The grant session the reporting harness was launched under — the same
    /// key [`LaunchSpec::mcp_grant_session`] carries, which is what ties a
    /// report back to the pane showing that harness.
    ///
    /// [`LaunchSpec::mcp_grant_session`]: crate::control_socket::grants::Grant
    pub session: String,
    /// The lifecycle moment that fired.
    pub event: HookEvent,
    /// When the server recorded it, in epoch milliseconds.
    pub at_ms: i64,
    /// One operator-facing line. Never the harness's payload — see the module
    /// docs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The directory the harness is working in, when it reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The harness's own session id, when it reported one. Observability only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
}

impl HookReport {
    /// A report of `event` from `session`, with everything optional unset.
    pub fn new(session: impl Into<String>, event: HookEvent) -> Self {
        HookReport {
            session: session.into(),
            event,
            at_ms: 0,
            summary: String::new(),
            cwd: None,
            harness_session_id: None,
        }
    }
}

/// A bounded, shared log of the reports this Medulla has received.
///
/// Cloning shares the log rather than copying it: the control socket's handler
/// and whatever renders the Hooks page hold the same one, and neither owns it.
#[derive(Clone, Default)]
pub struct HookEventLog {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    entries: VecDeque<HookReport>,
    /// Monotonic count of everything ever recorded, including what has since
    /// aged out. A reader that only wants "is there anything new" compares this
    /// rather than diffing the window.
    recorded: u64,
}

impl HookEventLog {
    /// An empty log.
    pub fn new() -> Self {
        HookEventLog::default()
    }

    /// Record `report`, dropping the oldest entry once the window is full.
    ///
    /// A poisoned lock is ignored rather than propagated: dropping a lifecycle
    /// report is a worse outcome for nobody, and panicking here would take down
    /// a control-socket connection serving an operator's live session.
    pub fn record(&self, report: HookReport) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.recorded += 1;
        if inner.entries.len() == CAPACITY {
            inner.entries.pop_front();
        }
        inner.entries.push_back(report);
    }

    /// The most recent reports, newest first, at most `limit` of them.
    pub fn recent(&self, limit: usize) -> Vec<HookReport> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        inner.entries.iter().rev().take(limit).cloned().collect()
    }

    /// The most recent reports from one session, newest first.
    pub fn recent_for(&self, session: &str, limit: usize) -> Vec<HookReport> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        inner
            .entries
            .iter()
            .rev()
            .filter(|entry| entry.session == session)
            .take(limit)
            .cloned()
            .collect()
    }

    /// How many reports have ever been recorded, including aged-out ones.
    pub fn recorded(&self) -> u64 {
        self.inner.lock().map(|inner| inner.recorded).unwrap_or(0)
    }

    /// The last event a session reported, if it has reported anything.
    ///
    /// This is the question the Agents rail asks: a session whose last report
    /// was `Stop` is idle, one that reported `Notification` is waiting on the
    /// operator, and one mid-turn reported a tool.
    pub fn last_event(&self, session: &str) -> Option<HookEvent> {
        let inner = self.inner.lock().ok()?;
        inner
            .entries
            .iter()
            .rev()
            .find(|entry| entry.session == session)
            .map(|entry| entry.event)
    }
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
