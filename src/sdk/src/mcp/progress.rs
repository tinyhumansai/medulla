//! Telling a client that a long tool call is still alive.
//!
//! MCP clients time a tool call out on *silence*, not on duration: Claude Code
//! aborts a call that has sent nothing for 1800 seconds. A workflow run that
//! sends nothing between its call and its return therefore has the whole run
//! measured against that ceiling, and a run longer than it is reported to the
//! caller as a failure while it carries on succeeding in the background.
//!
//! `notifications/progress` is the protocol's answer. A client that supplied a
//! `progressToken` in the request's `_meta` is asking to be kept informed, and
//! every notification carrying that token resets its idle timer. So a caller
//! that does choose to wait for a run gets a per-step heartbeat rather than
//! half an hour of nothing.
//!
//! Best effort throughout. A progress notification that could not be queued —
//! no token, a full channel, a client that went away — must never be allowed to
//! affect the run it is describing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

/// A live progress channel for one request.
///
/// Cloneable and `Send + Sync` because the thing reporting progress is a run
/// executing on other tasks, not the request handler itself.
#[derive(Clone)]
pub(crate) struct Progress {
    /// The token the client asked to be quoted back to it.
    token: Value,
    /// Where a notification goes to reach the client's stdout.
    outbound: tokio::sync::mpsc::Sender<Value>,
    /// How many notifications have been sent.
    ///
    /// The specification requires `progress` to increase with each
    /// notification for a given token. Counting sends satisfies that without
    /// pretending to know how much of the work is done — a workflow's step
    /// count is known, but a step's own duration is not.
    sent: Arc<AtomicU64>,
}

impl Progress {
    /// The progress channel this request asked for, if it asked for one.
    ///
    /// `params._meta.progressToken` is where MCP puts it. A request without one
    /// is a client that does not want notifications, and sending them anyway
    /// would be noise it has no correlation for.
    pub(crate) fn from_params(
        params: &Value,
        outbound: Option<&tokio::sync::mpsc::Sender<Value>>,
    ) -> Option<Self> {
        let token = params.get("_meta")?.get("progressToken")?.clone();
        if token.is_null() {
            return None;
        }
        Some(Self {
            token,
            outbound: outbound?.clone(),
            sent: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Report one step of progress, with a human-readable line for the client.
    ///
    /// `total` is passed through when the work has a known size — a workflow
    /// knows how many nodes it has — and omitted when it does not.
    ///
    /// Non-blocking and infallible by design: this is called from a synchronous
    /// event sink on the run's own task, so it can neither await nor propagate.
    /// A notification dropped because the client is not draining is a lost
    /// heartbeat, not a lost run.
    pub(crate) fn report(&self, message: impl Into<String>, total: Option<u64>) {
        let progress = self.sent.fetch_add(1, Ordering::Relaxed) + 1;
        let mut params = json!({
            "progressToken": self.token,
            "progress": progress,
            "message": message.into(),
        });
        if let Some(total) = total {
            // Never report a total smaller than the progress already sent: a
            // client rendering a bar would read that as going backwards.
            params["total"] = json!(total.max(progress));
        }
        let _ = self.outbound.try_send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": params,
        }));
    }
}
