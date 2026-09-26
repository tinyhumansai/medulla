//! Resolving a session to the agent that owns it.
//!
//! The rail's own level, and the only one it resolves for itself: hosts and
//! agents come from the shared projection (see [`super`]). Kept apart from the
//! assembly so the rule can be tested without building an
//! [`App`](super::super::types::App).
//!
//! A **dispatched** session already knows its agent: the hub files a task under
//! the roster id the dispatch named ([`lane_id`]), and that id is the lane's
//! `agent_id`. Nothing is re-derived here — re-deriving it by workspace would
//! disagree with the hub on exactly the case the hub was fixed for (a machine
//! advertising several agents at one address).
//!
//! An **operator-started** session knows only where it is running, so it is
//! matched back to the declaration whose `harness × workspace` it is a session
//! of. No match means the directory is undeclared, which is a real state the
//! rail shows rather than hides — and the prompt for inline agent creation.
//!
//! [`lane_id`]: https://docs.rs/medulla

use medulla::runtime::AgentDeclaration;

use crate::worker::pty::SessionRow;

/// The declaration a local PTY session belongs to, if one claims it.
///
/// Matched on `harness × workspace`, which is what an agent *is*: the CLI that
/// runs the work and the directory it runs in. Paths are compared with trailing
/// separators trimmed, because a declaration typed by hand and a cwd resolved by
/// the spawner disagree about the trailing slash far more often than they
/// disagree about the directory.
///
/// The harness compared is the session's *id*
/// ([`harness_id`](SessionRow::harness_id)) — a custom preset's own id, else the
/// CLI's wire name — because that is the vocabulary a declaration is written in.
/// A preset is a different agent running the same CLI, so comparing the CLI
/// underneath it matched a `deepseek` declaration against `claude` and left
/// every preset-backed session in the orphan list.
///
/// The first match wins. Two declarations of the same harness in the same
/// directory are the same agent declared twice, so which one claims the session
/// changes nothing an operator can see.
pub fn agent_for_session<'a>(
    declarations: &'a [AgentDeclaration],
    row: &SessionRow,
) -> Option<&'a AgentDeclaration> {
    let cwd = normalize_path(&row.cwd);
    let harness = row.harness_id();
    declarations.iter().find(|declaration| {
        declaration.harness.trim().eq_ignore_ascii_case(harness)
            && normalize_path(&declaration.workspace.path) == cwd
    })
}

/// A path with its trailing separators removed, for comparison only.
///
/// Never used to *build* a path: an empty result means "the root or nothing",
/// and both compare equal to each other, which is the honest answer for a blank
/// declaration.
fn normalize_path(path: &str) -> &str {
    let trimmed = path.trim();
    let stripped = trimmed.trim_end_matches('/');
    if stripped.is_empty() {
        trimmed
    } else {
        stripped
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
