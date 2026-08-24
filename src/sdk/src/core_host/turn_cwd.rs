//! The directory the *current* embedded OpenHuman turn is working in.
//!
//! # Why this module exists
//!
//! Medulla's lifecycle hooks are registered **once, process-globally**, at boot
//! ([`super::hooks::register_lifecycle_hooks`]). The working directory is the
//! opposite: it belongs to a single run, and two workflow nodes driving the same
//! process can legitimately be working in two different checkouts. So the hook
//! callback has no per-turn state of its own to read, and the value it needs is
//! only known by the caller that started the turn.
//!
//! Before this module the payload said `std::env::current_dir()`, which is one
//! long-lived value for the whole process — the directory Medulla was started
//! from. A `PostToolUse` auto-commit hook resolves its target repository from
//! that `cwd` (OpenHuman's tool arguments carry neither a `file_path` nor a
//! `cd`, so it is the only signal), so a workflow whose `workspace` was a
//! feature worktree had its checkpoint committed into whatever repository the
//! Medulla process happened to be launched in, on whatever branch that was.
//! Committing the wrong repository is worse than not committing at all.
//!
//! # Mechanism
//!
//! A [`tokio::task_local!`], scoped by the run around the core call it makes.
//! Chosen over the alternatives because:
//!
//! * **A process-global cell would race.** Concurrent turns overwrite each
//!   other's directory, and the hook would read whichever run wrote last — the
//!   same class of bug, only intermittent.
//! * **Widening OpenHuman's `ToolHookContext` is not ours to do.** The type is
//!   vendored upstream, and the directory is a Medulla-side fact about the run,
//!   not something the core knows.
//! * **The scope genuinely reaches the callback.** `Core::invoke` already runs
//!   the whole dispatch inside `CoreContext::scope`, the agent tool loop awaits
//!   its middleware directly, and the middleware awaits the embedder tool hook
//!   directly — no `tokio::spawn` in between. OpenHuman itself carries per-turn
//!   facts to tool execution this way (its sandbox mode, approval gate, and turn
//!   origin are all task-locals).
//!
//! # Limits worth knowing
//!
//! Standard task-local semantics: the scope does not follow a detached task. In
//! particular OpenHuman fires **post-turn** hooks through `tokio::spawn`, so a
//! `Stop` hook cannot read this value — which is why the stop payload still
//! carries no `cwd`. A turn started outside a scope (the TUI's own chat, an MCP
//! call) reads [`current_turn_cwd`] as `None` and the payload falls back to the
//! process directory, which for those callers is the right answer.

use std::future::Future;
use std::path::{Path, PathBuf};

tokio::task_local! {
    /// Directory the in-flight embedded turn is working in, when a caller
    /// declared one. Read by the hook adapter while building a payload.
    static TURN_CWD: PathBuf;
}

/// Run `future` with `cwd` installed as the current turn's working directory.
///
/// A `None`, empty, or blank `cwd` installs nothing and simply awaits `future`,
/// so a caller with no directory to declare does not have to branch: the hook
/// payload then falls back to the process directory exactly as before.
pub async fn with_turn_cwd<F>(cwd: Option<&Path>, future: F) -> F::Output
where
    F: Future,
{
    match cwd.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) => TURN_CWD.scope(path.to_path_buf(), future).await,
        None => future.await,
    }
}

/// The current turn's working directory, or `None` outside a scope.
pub fn current_turn_cwd() -> Option<PathBuf> {
    TURN_CWD.try_with(|path| path.clone()).ok()
}
