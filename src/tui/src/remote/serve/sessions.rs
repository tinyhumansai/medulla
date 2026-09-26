//! Opening, listing and closing sessions on behalf of a connected client.
//!
//! The translation between the wire's [`RemoteSessionRow`] and the manager's own
//! `SessionRow`, plus the one rule that matters for opening: a client may only
//! start something this host actually offered.

use medulla::protocol::{
    RemoteAttention, RemoteHarnessChoice, RemoteSessionRow, RemoteSessionState,
};

use crate::ui::harness_pane::{HarnessChoice, LocalSessions};
use crate::worker::pty::{PtyState, SessionRow};

/// Describe one of this host's launchable harnesses for the client's picker.
pub fn wire_choice(choice: &HarnessChoice) -> RemoteHarnessChoice {
    RemoteHarnessChoice {
        id: choice.id().to_string(),
        provider: choice.provider.as_wire().to_string(),
        preset: choice.preset.as_ref().map(|preset| preset.id.clone()),
        display_name: choice.display_name().to_string(),
    }
}

/// Find the local choice a client's request names.
///
/// Resolved **by id against what this host offers**, never reconstructed from
/// the fields the client sent. That is the whole authorization story for opening
/// a session: the daemon can only ever start something it already advertised, so
/// a client cannot name an arbitrary binary, an unlisted preset, or a provider
/// this machine deliberately does not serve. A request for something not on the
/// list is simply not found.
pub fn resolve_choice(
    sessions: &LocalSessions,
    wanted: &RemoteHarnessChoice,
) -> Option<HarnessChoice> {
    sessions
        .choices()
        .into_iter()
        .find(|choice| choice.id() == wanted.id)
}

/// Convert a manager row into the row the client's rail draws.
pub fn wire_row(row: &SessionRow, bracketed_paste: bool) -> RemoteSessionRow {
    RemoteSessionRow {
        id: row.id.clone(),
        label: row.label.clone(),
        provider: row.provider.as_wire().to_string(),
        preset: row.preset.clone(),
        state: match row.state {
            PtyState::Running => RemoteSessionState::Running,
            PtyState::Exited { code } => RemoteSessionState::Exited { code },
            PtyState::Failed => RemoteSessionState::Failed,
        },
        cwd: row.cwd.clone(),
        name: row.name.clone(),
        thread_name: row.thread_name.clone(),
        started_at: row.started_at,
        last_output_at: row.last_output_at,
        busy: row.busy,
        working: row.working,
        attention: row.attention.as_ref().map(|attention| RemoteAttention {
            summary: attention.what.clone(),
            kind: attention.kind.as_str().to_string(),
            // `since` deliberately does not cross: it is a reading from *this*
            // machine's clock, and sending it would invite the client to
            // subtract it from its own. The client stamps its own.
        }),
        bracketed_paste,
        last_error: row.last_error.clone(),
    }
}
