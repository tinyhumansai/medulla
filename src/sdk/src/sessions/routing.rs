//! Session-class and transport routing: which lifetime a stimulus gets, and
//! whether a provider can serve it interactively at all.
//!
//! The two decisions are deliberately **orthogonal**.
//! [`route_session_class`] answers *how long does this session live* — a
//! property of the stimulus and the operator's policy. [`route_transport`]
//! answers *how is the child process driven* — a property of the provider's
//! capabilities. Conflating them is how you end up asking `codex` to hold a
//! conversation over a stdin channel it does not have.

use crate::tinyplace::HarnessProvider;

use super::types::{SessionClass, SessionPolicy};

/// What provoked a session. The routing input that is *not* operator policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stimulus {
    /// A `medulla-tinyplace/1` `task` frame — a discrete unit of delegated work.
    Task,
    /// A conversational plain-text DM from a peer.
    PlainText,
    /// The operator opened a session from the TUI.
    Operator,
}

/// How a provider's child process is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// One process per turn: spawn, feed the prompt as argv, read to EOF.
    ///
    /// Continuity, when the provider supports it, comes from resuming a captured
    /// session id rather than from keeping the process alive.
    OneShot,
    /// One long-lived process fed newline-delimited JSON turns over stdin.
    Interactive,
}

impl Transport {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::OneShot => "one-shot",
            Transport::Interactive => "interactive",
        }
    }
}

/// Whether a provider can be driven by a live stdin turn channel.
///
/// Only `claude` can: `claude -p --input-format stream-json` reads one JSON
/// user message per line and answers each with a terminating `result` frame.
///
/// `codex exec` reads stdin as *additional initial prompt* and blocks until EOF
/// — the opposite of a per-turn channel — so holding its stdin open wedges the
/// run. `opencode run` does the same. Both therefore get
/// [`Transport::OneShot`], and their continuity comes from session resume.
pub fn can_run_interactive(provider: HarnessProvider) -> bool {
    matches!(provider, HarnessProvider::Claude)
}

/// Whether a provider can resume a previously captured session id.
///
/// `claude --resume <id>` and `codex exec resume <id>` both can. `opencode` has
/// no resume flag, so an unbound `opencode` session runs each turn fresh —
/// which degrades continuity but never correctness.
pub fn can_resume(provider: HarnessProvider) -> bool {
    matches!(provider, HarnessProvider::Claude | HarnessProvider::Codex)
}

/// Resolve the lifetime class for one stimulus.
///
/// Precedence, highest first:
/// 1. A per-frame `requested` class — the sender knows what it wants.
/// 2. A non-`auto` operator `policy` pin.
/// 3. The stimulus itself: a task frame is discrete work and routes
///    [`SessionClass::Bounded`] (two tasks must never see each other's
///    context); a plain-text DM or an operator-opened session is a conversation
///    and routes [`SessionClass::Unbound`] (a peer talking to the agent expects
///    it to remember).
pub fn route_session_class(
    stimulus: Stimulus,
    requested: Option<SessionClass>,
    policy: SessionPolicy,
) -> SessionClass {
    if let Some(requested) = requested {
        return requested;
    }
    match policy {
        SessionPolicy::Bounded => return SessionClass::Bounded,
        SessionPolicy::Unbound => return SessionClass::Unbound,
        SessionPolicy::Auto => {}
    }
    match stimulus {
        Stimulus::Task => SessionClass::Bounded,
        Stimulus::PlainText | Stimulus::Operator => SessionClass::Unbound,
    }
}

/// Resolve how a session's child process is driven.
///
/// An [`SessionClass::Unbound`] session wants [`Transport::Interactive`] so the
/// conversation lives in one process; a [`SessionClass::Bounded`] one has
/// exactly one turn and gains nothing from a persistent process, so it always
/// runs one-shot.
///
/// A provider that cannot be driven interactively degrades to
/// [`Transport::OneShot`] rather than failing — an unbound `codex` session is
/// still a real session, it just rebuilds its context from `exec resume` each
/// turn.
pub fn route_transport(class: SessionClass, provider: HarnessProvider) -> Transport {
    match class {
        SessionClass::Bounded => Transport::OneShot,
        SessionClass::Unbound => {
            if can_run_interactive(provider) {
                Transport::Interactive
            } else {
                Transport::OneShot
            }
        }
    }
}

/// Whether an unbound session on `provider` can actually carry context across
/// turns, by either transport.
///
/// `false` means the operator is about to open a session that will answer every
/// turn with no memory of the last — worth surfacing in the UI rather than
/// silently degrading.
pub fn has_continuity(provider: HarnessProvider) -> bool {
    can_run_interactive(provider) || can_resume(provider)
}
