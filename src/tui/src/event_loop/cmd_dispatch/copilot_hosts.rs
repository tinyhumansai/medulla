//! The loopback host each copilot thread keeps between turns.
//!
//! A turn used to start an embedded daemon and drop it with the reply. That was
//! right while a turn was self-contained, and wrong the moment the pane became a
//! conversation: the harness session a turn opens is remembered *by the daemon*,
//! so a daemon that dies with the turn takes the conversation with it. The
//! second instruction would start a third session that had never seen the first
//! two, and "now do the same to the other node" could not work.
//!
//! So the host outlives the turn and is keyed by thread — one conversation per
//! pane, which is what an operator editing two workflows side by side means by
//! having two panes. Dropping the process spawn and the loopback bind per turn
//! is a real saving, but it is a side effect rather than the reason.
//!
//! The cache is capped. Each entry is a live daemon holding a harness process
//! group, so an operator walking a large catalogue must not accumulate one per
//! workflow they glanced at. Evicting the least recently used ends that thread's
//! conversation and nothing else: the next instruction on it starts a fresh
//! session, which is exactly the behaviour before any of this.

use std::sync::{Arc, Mutex, OnceLock};

use medulla::daemon::embedded::EmbeddedDaemonOptions;
use medulla::workflows::LocalWorkflowHost;

/// How many copilot conversations stay live at once.
///
/// Small on purpose. Each is a daemon and the harness processes it spawns, and
/// an operator is realistically editing one workflow and referring back to
/// another — not holding ten conversations.
const MAX_LIVE_HOSTS: usize = 3;

/// `thread -> host`, least-recently-used first.
///
/// A `Vec` rather than a map because the cap is three: recency ordering is the
/// operation that matters and a linear scan of three entries is free.
type Hosts = Vec<(String, Arc<LocalWorkflowHost>)>;

fn hosts() -> &'static Mutex<Hosts> {
    static HOSTS: OnceLock<Mutex<Hosts>> = OnceLock::new();
    HOSTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// The host serving `thread`, starting one if this is its first turn.
///
/// `options` is only called when a host actually has to be started, so a
/// continuing conversation costs a lookup rather than rebuilding the daemon
/// configuration it is not going to use.
///
/// # Errors
///
/// Fails when no coding-agent CLI is installed or the loopback endpoints cannot
/// be bound — both are situations the operator has to see rather than a pane
/// that accepts instructions and answers none of them.
pub(super) fn host_for(
    thread: &str,
    options: impl FnOnce() -> EmbeddedDaemonOptions,
) -> Result<Arc<LocalWorkflowHost>, String> {
    let mut hosts = hosts().lock().expect("copilot host cache lock");

    if let Some(host) = touch(&mut hosts, thread) {
        return Ok(host);
    }

    let host = Arc::new(LocalWorkflowHost::start(options())?);
    insert(&mut hosts, thread, host.clone());
    Ok(host)
}

/// Move a thread's host onto a new key.
///
/// A create turn runs on a sentinel thread because the workflow it is building
/// has no id yet. When one appears, the transcript moves onto it — and the
/// conversation has to move with it, or the operator's very next instruction
/// ("now add a Slack step") would be the first turn of a session that had never
/// heard of the workflow it just built.
pub(super) fn rename(from: &str, to: &str) {
    rename_in(
        &mut hosts().lock().expect("copilot host cache lock"),
        from,
        to,
    );
}

/// End a thread's conversation, stopping its daemon.
///
/// Called when a workflow stops existing. Left uncalled, the entry would sit
/// there until the cap pushed it out — harmless, but it would keep a harness
/// process group alive for a workflow the operator deleted.
pub(super) fn forget(thread: &str) {
    forget_in(
        &mut hosts().lock().expect("copilot host cache lock"),
        thread,
    );
}

// The bookkeeping below is generic over the stored value because none of it
// depends on what a host *is* — and a test that had to start a real daemon to
// check a rename would need a coding CLI on `PATH`, which this suite does not
// get to assume.

/// The entry for `thread`, moved to the most-recent end.
fn touch<T: Clone>(entries: &mut Vec<(String, T)>, thread: &str) -> Option<T> {
    let index = entries.iter().position(|(key, _)| key == thread)?;
    let entry = entries.remove(index);
    let value = entry.1.clone();
    entries.push(entry);
    Some(value)
}

/// Add an entry as the most recent, evicting the least recent past the cap.
fn insert<T>(entries: &mut Vec<(String, T)>, thread: &str, value: T) {
    entries.push((thread.to_string(), value));
    while entries.len() > MAX_LIVE_HOSTS {
        entries.remove(0);
    }
}

/// Move `from`'s entry onto the key `to`.
fn rename_in<T>(entries: &mut Vec<(String, T)>, from: &str, to: &str) {
    // Any entry already on the destination key is replaced: the workflow is
    // new, so a conversation filed under its id is a stale one from a deleted
    // workflow that happened to share it.
    entries.retain(|(key, _)| key != to);
    if let Some(entry) = entries.iter_mut().find(|(key, _)| key == from) {
        entry.0 = to.to_string();
    }
}

/// Drop `thread`'s entry, if it has one.
fn forget_in<T>(entries: &mut Vec<(String, T)>, thread: &str) {
    entries.retain(|(key, _)| key != thread);
}

#[cfg(test)]
#[path = "copilot_hosts_tests.rs"]
mod tests;
