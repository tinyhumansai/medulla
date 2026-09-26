//! What the pane shows for a rail row that has no transcript of its own.
//!
//! A host header and the action row both name something real, and neither is a
//! conversation. The pane used to fall back to
//! [`lane_lines`](crate::ui::agents::lane_lines) for them — with a lane index
//! that had itself fallen back to **0**, the orchestrator's — so selecting an
//! action row showed the orchestrator thinking, attributed to a row that had not
//! thought anything.
//!
//! So each row describes itself instead: the host says where it is and how many
//! sessions it holds, and the action says what it will do. Nothing here is
//! lane-shaped, because none of these rows has a lane.

use crate::ui::agents::Line as StyledLine;

use super::super::super::rail::{RailRow, NEW_SESSION_LABEL};
use super::super::super::types::App;
use super::types::Selection;

/// A rendered description of a laneless row: what to title the pane, and what to
/// put in it.
pub(super) struct RowPanel {
    /// The pane title — what the row is, not "Transcript".
    pub(super) title: String,
    /// The body, already wrapped to the pane width.
    pub(super) lines: Vec<StyledLine>,
}

/// Describe whatever laneless row the cursor is on.
///
/// Total by construction: an unknown or missing row degrades to an empty panel
/// rather than borrowing another row's content, which is the failure this module
/// exists to end.
pub(super) fn row_panel(app: &App, selection: &Selection, width: usize) -> RowPanel {
    let _ = (app, width);
    match selection.row() {
        Some(RailRow::Host(host)) => host_panel(selection, host),
        Some(RailRow::NewSession) => new_session_panel(),
        _ => RowPanel {
            title: "Transcript".into(),
            lines: Vec::new(),
        },
    }
}

/// The host: what it is called, whether it can be acted on, what it holds.
fn host_panel(selection: &Selection, host: &super::super::super::rail::HostRailRow) -> RowPanel {
    let lines = vec![
        field("host", &host.label),
        field("id", &host.host_id),
        field(
            "reach",
            if host.local {
                "this device — its agents are declared here"
            } else {
                "remote — its agents are declared on that machine"
            },
        ),
        field("sessions", &sessions_under(selection).to_string()),
    ];
    RowPanel {
        title: format!("host · {}", host.label),
        lines,
    }
}

/// The `+ New session` action, in one breath: what it asks and what it starts.
fn new_session_panel() -> RowPanel {
    RowPanel {
        title: NEW_SESSION_LABEL.to_string(),
        lines: vec![
            note("Start a session on this device: pick a harness type, then the"),
            note("directory it works in. Nothing else is asked — the session names"),
            note("itself from your first prompt."),
            StyledLine::default(),
            note("It is yours at birth: the orchestrator will not dispatch into it"),
            note("until you hand it over with ^G."),
            StyledLine::default(),
            note("⏎ or ^T opens the picker."),
        ],
    }
}

/// One `label   value` row, label dimmed so the values line up as the content.
fn field(label: &str, value: &str) -> StyledLine {
    StyledLine {
        text: format!("{label:<10} {value}"),
        color: None,
        dim: false,
    }
}

/// One dimmed prose line.
fn note(text: &str) -> StyledLine {
    StyledLine {
        text: text.to_string(),
        color: None,
        dim: true,
    }
}

/// How many session rows hang off the host under the cursor.
///
/// Counted off the rail rather than off the roster so the number and the rows
/// beneath it cannot disagree: the walk stops at the next host row, which is
/// exactly where this host's group ends.
fn sessions_under(selection: &Selection) -> usize {
    selection
        .rows
        .iter()
        .skip(selection.active + 1)
        .take_while(|row| !matches!(row, RailRow::Host(_)))
        .filter(|row| matches!(row, RailRow::Session(_)))
        .count()
}
