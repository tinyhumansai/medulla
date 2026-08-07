//! Harness lifecycle classification for Agents-rail lane styling.
//!
//! The event fold exposes task activity, attention, terminal results, and an
//! open-vocabulary peer-session state. This module reduces those inputs to the
//! five states the rail promises, independently of how a row is laid out.

use ratatui::style::Color;

use crate::ui::agents::{AgentLane, TaskStatus};
use crate::ui::util::SPINNER;
use crate::worker::pty::{AttentionKind, HarnessAttention, PtyState, SessionRow, ATTENTION_GLYPH};

/// The five lifecycle states shown by harness-backed rows in the Agents rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HarnessVisualState {
    /// The harness is actively processing work.
    Working,
    /// The harness is blocked on operator input.
    NeedsInput,
    /// The most recent settled task or session failed.
    Errored,
    /// The most recent settled task or session completed successfully.
    Completed,
    /// The harness is new, idle, cancelled, or otherwise inactive.
    Inactive,
}

impl HarnessVisualState {
    /// Stable sidebar wording for this state.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsInput => "needs input",
            Self::Errored => "errored",
            Self::Completed => "completed",
            Self::Inactive => "inactive",
        }
    }

    /// The foreground colour promised for this lifecycle state.
    pub(super) fn color(self) -> Color {
        match self {
            Self::Working | Self::Completed => Color::Green,
            Self::NeedsInput => Color::Yellow,
            Self::Errored => Color::Red,
            Self::Inactive => Color::DarkGray,
        }
    }

    /// Whether the row should blink.
    ///
    /// Active work does, because a lane that is moving reads differently from
    /// one that has stopped. A lane waiting on the operator does too, and for a
    /// stronger reason: it is the only state on this rail that will not resolve
    /// itself, so it is the only one worth pulling an eye off another pane for.
    /// The two are told apart by colour — green is moving, yellow wants you.
    ///
    /// Terminal states stay solid: nothing about them is waiting.
    pub(super) fn flashes(self) -> bool {
        matches!(self, Self::Working | Self::NeedsInput)
    }

    /// Whether this state should *pulse* — brighten and dim on the frame clock.
    ///
    /// Narrower than [`flashes`](Self::flashes), and the difference is the point.
    /// Working now has an animation of its own — a spinner, which says "moving"
    /// far better than a colour change can — so the pulse is spent only on the
    /// states that will not resolve themselves. Errored joins needs-input there:
    /// a harness that fell over is exactly as stuck, and used to sit on the rail
    /// as still as a healthy one.
    pub(super) fn pulses(self) -> bool {
        matches!(self, Self::NeedsInput | Self::Errored)
    }

    /// The glyph for this state on animation frame `frame`.
    ///
    /// Working spins. That is the whole reason this takes a frame: a live
    /// harness and an idle one both drew `●`, so the rail could say a session was
    /// *alive* but never that it was *doing something* — which is the thing an
    /// operator actually watches a rail for.
    pub(super) fn glyph(self, frame: usize) -> String {
        match self {
            Self::Working => SPINNER[frame % SPINNER.len()].to_string(),
            Self::NeedsInput => ATTENTION_GLYPH.to_string(),
            Self::Errored => "✕".to_string(),
            Self::Completed => "✓".to_string(),
            Self::Inactive => "○".to_string(),
        }
    }
}

/// Classify one operator-started local harness from its row and resolved cue.
///
/// The rail's five states, sourced for a session this device runs itself. The
/// row carries facts no peer session has — a live screen classification, an exit
/// code, a recorded write error — and until now used almost none of them: every
/// running session drew the same dot whatever it was doing, and a failed one
/// drew a cross nobody was told to look at.
///
/// `cue` is passed in already resolved rather than read off the row, because the
/// caller suppresses it for the pane the operator is currently attached to —
/// a harness cannot be waiting on you while you are sitting in front of it.
pub(super) fn classify_local(row: &SessionRow, cue: Option<&HarnessAttention>) -> HarnessVisualState {
    // A failure cue outranks the exit status it was derived from *and* anything
    // still on screen: it is the reason the session stopped mattering.
    if let Some(cue) = cue {
        return if cue.kind.is_failure() {
            HarnessVisualState::Errored
        } else if cue.kind == AttentionKind::Completed {
            HarnessVisualState::Completed
        } else {
            HarnessVisualState::NeedsInput
        };
    }
    match row.state {
        PtyState::Failed => HarnessVisualState::Errored,
        PtyState::Exited { code: Some(code) } if code != 0 => HarnessVisualState::Errored,
        PtyState::Exited { .. } => HarnessVisualState::Completed,
        // Live, and its own screen says a turn is in flight. Everything else
        // running is a composer waiting for someone to type in it, which is
        // presence rather than activity.
        PtyState::Running if row.working => HarnessVisualState::Working,
        PtyState::Running => HarnessVisualState::Inactive,
    }
}

/// Resolve current activity before considering an older terminal task.
///
/// Pending input wins over active work, then the most recent terminal task
/// supplies completion or error. A taskless peer lane falls back to its
/// reported session state.
pub(super) fn classify(lane: &AgentLane, session_state: Option<&str>) -> HarnessVisualState {
    if lane.tasks.iter().any(|task| task.attention.is_some()) {
        return HarnessVisualState::NeedsInput;
    }
    if lane.active_tasks > 0
        || lane
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::Running)
    {
        return HarnessVisualState::Working;
    }
    if let Some(task) = lane.tasks.iter().max_by_key(|task| task.last_at) {
        return match task.status {
            TaskStatus::Running => HarnessVisualState::Working,
            TaskStatus::Done => HarnessVisualState::Completed,
            TaskStatus::Failed => HarnessVisualState::Errored,
            TaskStatus::Cancelled => HarnessVisualState::Inactive,
        };
    }
    session_state
        .map(classify_session)
        .unwrap_or(HarnessVisualState::Inactive)
}

/// Normalize an open-vocabulary peer-session state into the sidebar contract.
/// Unknown states fail closed to inactive rather than claiming work.
fn classify_session(state: &str) -> HarnessVisualState {
    match state.trim().to_ascii_lowercase().as_str() {
        "running" | "working" | "busy" | "active" => HarnessVisualState::Working,
        "waiting" | "needs_input" | "needs-input" | "awaiting_input" | "awaiting-input" => {
            HarnessVisualState::NeedsInput
        }
        "error" | "errored" | "failed" => HarnessVisualState::Errored,
        "complete" | "completed" | "done" | "ended" | "success" | "succeeded" => {
            HarnessVisualState::Completed
        }
        _ => HarnessVisualState::Inactive,
    }
}
