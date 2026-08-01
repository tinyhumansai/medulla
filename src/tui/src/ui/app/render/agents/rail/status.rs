//! Harness lifecycle classification for Agents-rail lane styling.
//!
//! The event fold exposes task activity, attention, terminal results, and an
//! open-vocabulary peer-session state. This module reduces those inputs to the
//! five states the rail promises, independently of how a row is laid out.

use ratatui::style::Color;

use crate::ui::agents::{AgentLane, TaskStatus};

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

    /// Only active work flashes; every terminal or waiting state stays solid.
    pub(super) fn flashes(self) -> bool {
        self == Self::Working
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
