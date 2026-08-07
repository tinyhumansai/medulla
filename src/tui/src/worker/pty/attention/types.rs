//! Data types for the `attention` module: what a harness is waiting for, and
//! how loudly the rail should say so.

#[allow(unused_imports)]
use super::*;

/// Why a harness wants the operator.
///
/// Ordered by how certain the signal is, most certain first — [`Ord`] is derived
/// from that order and [`HarnessAttention::supersedes`] relies on it, so a bell
/// never displaces a permission prompt we can actually name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionKind {
    /// The harness process died, or a write to it failed. Nothing will happen
    /// in this session again until the operator restarts it, which is what makes
    /// it the most certain cue there is.
    Failed,
    /// A startup dialog recognised by [`super::super::dialog`] — trust, the
    /// bypass disclaimer, codex's update prompt. The harness will not take work
    /// until it is answered.
    Dialog,
    /// A permission or approval prompt: the harness wants to run a command,
    /// apply a patch, or leave plan mode, and is holding until told.
    Approval,
    /// The harness printed a blocking error and stopped — a usage limit, an
    /// expired credential, an API failure it will not retry past.
    ///
    /// Distinct from [`Failed`](Self::Failed): the process is alive and the
    /// screen is readable, but the turn is over and it did not do the work. The
    /// operator has to intervene, and nothing on the row would otherwise say so.
    Error,
    /// A numbered choice is on screen with the cursor resting on an option. What
    /// it is asking is not recognised, but that it is asking is unambiguous.
    Choice,
    /// A turn the orchestrator dispatched has settled and the session is being
    /// held open for the operator to read.
    ///
    /// The weakest *actionable* cue, and deliberately not raised for an ordinary
    /// idle composer: a harness sitting at a prompt nobody asked anything of is
    /// the normal resting state of half the rail, and flagging it would make the
    /// flag mean nothing. A retained session is different — something finished,
    /// and it is standing there because a person is expected to look.
    Completed,
    /// The harness rang the terminal bell. Every harness does this when a turn
    /// ends or a prompt opens, so it is the one cue that needs no vocabulary —
    /// and the vaguest, so it loses to anything else.
    Bell,
}

impl AttentionKind {
    /// The display string, used in tests and in the pane title.
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionKind::Failed => "failed",
            AttentionKind::Dialog => "dialog",
            AttentionKind::Approval => "approval",
            AttentionKind::Error => "error",
            AttentionKind::Choice => "choice",
            AttentionKind::Completed => "completed",
            AttentionKind::Bell => "bell",
        }
    }

    /// Whether this cue reports something that went *wrong*.
    ///
    /// Drives colour rather than precedence: a failure and a permission prompt
    /// both want the operator, and only one of them is bad news. The rail draws
    /// the first in red and the second in the configured attention colour, so
    /// the two are told apart before either is read.
    pub fn is_failure(self) -> bool {
        matches!(self, AttentionKind::Failed | AttentionKind::Error)
    }
}

/// A harness waiting on the operator, as the rail shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessAttention {
    /// Why it is waiting.
    pub kind: AttentionKind,
    /// What it is waiting for, in the operator's terms — short enough to sit at
    /// the end of a rail row.
    pub what: String,
    /// Epoch ms when this cue was *first* seen.
    ///
    /// Preserved across refreshes for as long as the cue holds, so the rail can
    /// say how long a harness has been stuck rather than resetting to zero on
    /// every repaint of the same dialog.
    pub since: i64,
}

impl HarnessAttention {
    /// Build a cue first seen at `since`.
    pub fn new(kind: AttentionKind, what: impl Into<String>, since: i64) -> Self {
        Self {
            kind,
            what: what.into(),
            since,
        }
    }

    /// Whether `self` should replace `other` on the row.
    ///
    /// A more certain kind always wins; within one kind the incumbent stays, so
    /// its [`since`](Self::since) survives a repaint that says the same thing.
    pub fn supersedes(&self, other: &HarnessAttention) -> bool {
        self.kind < other.kind
    }

    /// How long the harness has been waiting, in milliseconds.
    pub fn waiting_ms(&self, now: i64) -> i64 {
        now.saturating_sub(self.since).max(0)
    }

    /// The rail suffix: what it wants, and how long it has wanted it.
    ///
    /// The elapsed time is the half an operator acts on — a prompt that has been
    /// up for four minutes is a stalled agent, and the same prompt two seconds
    /// old is just a harness doing its job.
    pub fn label(&self, now: i64) -> String {
        let waited = self.waiting_ms(now) / 1_000;
        if waited >= 1 {
            format!("{} · {waited}s", self.what)
        } else {
            self.what.clone()
        }
    }
}
