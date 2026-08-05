//! What the pane shows for a rail row that has no transcript of its own.
//!
//! A host header, an agent nothing has been dispatched to, and the two action
//! rows all name something real, and none of them is a conversation. The pane
//! used to fall back to [`lane_lines`](crate::ui::agents::lane_lines) for them —
//! with a lane index that had itself fallen back to **0**, the orchestrator's —
//! so selecting `+ New agent` showed the orchestrator thinking, attributed to a
//! row that had not thought anything.
//!
//! So each of those rows describes itself instead: the agent says what it is and
//! how to start a session on it, the host says where it is and how many agents
//! it holds, and the action rows say what they will do. Nothing here is
//! lane-shaped, because none of these rows has a lane.

use crate::ui::agents::Line as StyledLine;

use super::super::super::rail::{RailRow, NEW_AGENT_LABEL, NEW_SESSION_LABEL};
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
    match selection.row() {
        Some(RailRow::Agent(agent)) => agent_panel(app, selection, agent, width),
        Some(RailRow::Host(host)) => host_panel(selection, host),
        Some(RailRow::NewAgent) => new_agent_panel(),
        Some(RailRow::NewSession { agent_id }) => new_session_panel(app, agent_id),
        _ => RowPanel {
            title: "Transcript".into(),
            lines: Vec::new(),
        },
    }
}

/// The agent itself: its identity, where it runs, and what it is running.
///
/// Reached only for an agent with no lane — one the fold has produced no traffic
/// for. An agent *with* a lane keeps its own transcript, which is the thing an
/// operator opened it for.
fn agent_panel(
    app: &App,
    selection: &Selection,
    agent: &super::super::super::rail::AgentRailRow,
    width: usize,
) -> RowPanel {
    let sessions = sessions_under(selection);
    let mut lines = vec![
        field("agent", &agent.label()),
        field("id", &agent.agent_id),
        field("harness", agent.harness().unwrap_or("not declared")),
        field("workspace", agent.workspace().unwrap_or("not declared")),
        field("host", &host_label(app, selection, &agent.host_id)),
    ];
    let roles = agent
        .agent
        .as_ref()
        .map(|row| row.roles.join(", "))
        .filter(|roles| !roles.trim().is_empty())
        // An agent with no roles is offered for every template, which is the
        // useful thing to say — "none" would read as "excluded from all".
        .unwrap_or_else(|| "any".to_string());
    lines.push(field("roles", &roles));
    lines.push(field("sessions", &sessions.to_string()));
    lines.push(StyledLine::default());
    lines.extend(wrap_note(
        if sessions == 0 {
            format!(
                "No sessions yet. {NEW_SESSION_LABEL} under this agent — or ^T — starts one in {}.",
                agent.workspace().unwrap_or("its workspace")
            )
        } else {
            format!(
                "Its sessions are listed under it on the rail; {NEW_SESSION_LABEL} — or ^T — starts another."
            )
        },
        width,
    ));
    RowPanel {
        title: format!("agent · {}", agent.label()),
        lines,
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
        field("agents", &agents_under(selection).to_string()),
    ];
    RowPanel {
        title: format!("host · {}", host.label),
        lines,
    }
}

/// The `+ New agent` action, in one breath: what it writes and what it does not.
fn new_agent_panel() -> RowPanel {
    RowPanel {
        title: NEW_AGENT_LABEL.to_string(),
        lines: vec![
            note("Declare an agent on this device: a harness type × a workspace"),
            note("directory, with a name you choose."),
            StyledLine::default(),
            note("Declaring starts nothing. The agent gets a row from that moment"),
            note("whether or not anything ever runs in it, and the orchestrator"),
            note("can dispatch to it by name."),
            StyledLine::default(),
            note("⏎ or click opens the picker."),
        ],
    }
}

/// The `+ new session` action, named for the agent it will start one on.
fn new_session_panel(app: &App, agent_id: &str) -> RowPanel {
    let declaration = medulla::config::agent_declaration(app.agent_declarations(), agent_id);
    let mut lines = vec![
        note("Start a session on this agent — its declared harness, in its"),
        note("declared directory. You are asked for a name first."),
        StyledLine::default(),
    ];
    match declaration {
        Some(declaration) => {
            lines.push(field("agent", agent_id));
            lines.push(field("harness", &declaration.harness));
            lines.push(field(
                "workspace",
                declaration.workspace.path().unwrap_or("not declared"),
            ));
        }
        // The rail only offers the action for a declared agent, so this is the
        // window between a declaration being removed and the next frame.
        None => lines.push(field("agent", agent_id)),
    }
    lines.push(StyledLine::default());
    lines.push(note(
        "It is yours at birth: the orchestrator will not dispatch into it",
    ));
    lines.push(note("until you hand it over with ^G."));
    lines.push(StyledLine::default());
    lines.push(note("⏎ or click starts it."));
    RowPanel {
        title: format!("{NEW_SESSION_LABEL} · {agent_id}"),
        lines,
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

/// Wrap a sentence into dimmed lines at the pane width.
fn wrap_note(text: String, width: usize) -> Vec<StyledLine> {
    crate::ui::util::wrap(&text, width.max(20))
        .into_iter()
        .map(|row| note(&row))
        .collect()
}

/// How many session rows hang off the agent under the cursor.
///
/// Counted off the rail rather than off the roster so the number and the rows
/// beneath it cannot disagree: the walk stops at the next agent, host, or the
/// machine-level action, which is exactly where that agent's group ends.
fn sessions_under(selection: &Selection) -> usize {
    selection
        .rows
        .iter()
        .skip(selection.active + 1)
        .take_while(|row| {
            !matches!(
                row,
                RailRow::Agent(_) | RailRow::Host(_) | RailRow::NewAgent
            )
        })
        .filter(|row| matches!(row, RailRow::Session(_)))
        .count()
}

/// How many agent rows hang off the host under the cursor.
fn agents_under(selection: &Selection) -> usize {
    selection
        .rows
        .iter()
        .skip(selection.active + 1)
        .take_while(|row| !matches!(row, RailRow::Host(_)))
        .filter(|row| matches!(row, RailRow::Agent(_)))
        .count()
}

/// What to call the host an agent is placed on.
///
/// The rail's own host row when there is one, else this machine when the id is
/// the local host's, else the bare id — which is all a lane-only agent carries.
fn host_label(app: &App, selection: &Selection, host_id: &str) -> String {
    let host_id = host_id.trim();
    if let Some(host) = selection.rows.iter().find_map(|row| match row {
        RailRow::Host(host) if host.host_id.trim() == host_id && !host_id.is_empty() => Some(host),
        _ => None,
    }) {
        return host.label.clone();
    }
    if host_id.is_empty() || host_id == app.local_host_id().trim() {
        return "this device".to_string();
    }
    host_id.to_string()
}
