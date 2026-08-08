//! The Sessions rail: one cursor over configured `Host → Group → Session`
//! sections.
//!
//! ```text
//! + New session             ← the one action, when this device hosts
//! ▸ this device             ← host row, only when a remote host exists
//!   ├ /workspace            ← optional path or harness group heading
//!   │ ├ t_41 · running      ← a session the orchestrator dispatched
//!   │ └ debug login         ← a session the operator started
//!   │   └ wf run · deploy   ← a workflow run that session started
//!   └ +3 more               ← the fold's paging control
//! ```
//!
//! Appearance preferences organize the sections by host, path, harness, or no
//! heading, and order their agents and sessions by creation time, recent
//! activity, or name.
//!
//! The rail lists **what is running**. It used to render the whole
//! `Host → Agent → Session` tree, with a row per declared agent, because
//! declaring one was a thing an operator did from here. It is not any more —
//! declarations are config-only — so an agent row was a level of nesting that
//! cost every session beneath it an indent and answered no question the operator
//! could act on.
//!
//! The agent tier is still *resolved*, just not drawn. [`place_agents`] still
//! puts the folded lanes onto the shared `Host → Agent` projection
//! ([`medulla::ui::hosts::host_rows`]) — the same call the Hosts tab renders, so
//! the two lenses cannot disagree — and that is what gives a session its lane
//! (hence its transcript) and its place in the order. Sessions are resolved
//! here: a dispatched one by the roster id the hub filed its task under, an
//! operator-started one by [`resolve::agent_for_session`]. A session in a
//! directory nothing declares is listed last rather than dropped; one that is
//! running, costing tokens and invisible is the failure the old
//! `── your harnesses ──` group existed to prevent.
//!
//! Row shapes live in [`types`]; the session → agent rule in [`resolve`]; this
//! module is the assembly.

use medulla::config::agent_declarations_for_host;
use medulla::runtime::AgentDeclaration;
use medulla::ui::hosts::{HostAgentRow, HostKind, HostRow};

use super::types::App;
use crate::ui::agents::{ordered_tasks, AgentLane, AgentRole};
use crate::worker::pty::SessionRow;

mod cleanup;
#[cfg(test)]
mod cleanup_tests;
mod cursor;
#[cfg(test)]
mod cursor_tests;
mod organize;
mod paging;
pub(in crate::ui::app) mod resolve;
// Kept apart from `tests` rather than nested inside it: the assembly rules and
// the served-dispatch merge are separate responsibilities, and one file for
// both had already grown past this repository's line ceiling.
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
pub(in crate::ui::app) mod tests;
mod types;

pub(in crate::ui::app) use cursor::rail_anchor;
#[cfg(test)]
pub(in crate::ui::app) use cursor::resolve_rail_cursor;
use types::{AgentGroup, HostGroup};
pub use types::{
    AgentRailRow, GroupRailRow, HostRailRow, RailAnchor, RailRow, SessionRailRow,
    WorkflowRunRailRow,
};

/// The label on the rail's "open a session" row.
///
/// The only action on the rail, so it sits at the top level and is capitalised
/// as one: it acts on the machine, not on a row beneath it.
pub(in crate::ui::app) const NEW_SESSION_LABEL: &str = "+ New session";

impl App {
    /// The agent declarations this machine's config records.
    ///
    /// Read live rather than cached: [`declare_agent`](medulla::config::declare_agent)
    /// writes the file and hands back the list, which is assigned straight into
    /// the loaded config, so the next frame's rail is the list as written.
    pub(in crate::ui::app) fn agent_declarations(&self) -> &[AgentDeclaration] {
        &self.loaded.config.fleet.agent_declarations
    }

    /// The host id this machine's agents are declared against.
    ///
    /// The running host's bus address is the authority — it is what the local
    /// roster stamps every entry with. Without a running host there is nothing
    /// local to place agents on, and the empty string matches the declarations
    /// that name no host.
    pub(in crate::ui::app) fn local_host_id(&self) -> String {
        self.host_obs
            .as_ref()
            .map(|host| host.address().to_string())
            .unwrap_or_default()
    }

    /// The declarations belonging to this machine, in declaration order.
    pub(in crate::ui::app) fn local_agent_declarations(&self) -> Vec<AgentDeclaration> {
        let host_id = self.local_host_id();
        agent_declarations_for_host(self.agent_declarations(), &host_id)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The rail's rows: the conversation, the create action, and the tree.
    ///
    /// Assembled in three passes so each one answers a single question. First the
    /// fold is split — the orchestrator and the function lanes keep their rows,
    /// the agent lanes become groups keyed by agent id. Then the shared
    /// `Host → Agent` projection places those groups, adding every agent that has
    /// no traffic and every host that holds one. Last the live PTY sessions are
    /// attached to whichever agent declares the directory they run in, and the
    /// whole thing is flattened under host rows — which appear only when there is
    /// more than one host to tell apart.
    pub(super) fn rail_rows(&self) -> Vec<RailRow> {
        let lanes = self.lanes();
        self.rail_rows_in(&lanes)
    }

    /// Assemble rail rows from one already-captured lane snapshot.
    ///
    /// Callers that also resolve a cursor anchor must use this with that same
    /// snapshot: lane indexes in fold rows are meaningful only to the lanes
    /// that produced them.
    pub(super) fn rail_rows_in(&self, lanes: &[AgentLane]) -> Vec<RailRow> {
        let folded = self.split_fold(lanes);
        let mut hosts = place_agents(&self.host_tree(), folded);
        let mut orphans = self.attach_sessions(&mut hosts);
        let appearance = &self.loaded.config.appearance;
        let sections = organize::organize(
            hosts,
            self.agent_declarations(),
            appearance.sidebar_grouping,
            appearance.sidebar_sort,
        );
        organize::sort_sessions(&mut orphans, appearance.sidebar_sort);
        self.flatten(sections, orphans, lanes)
    }

    /// Fold the lane rows into per-agent groups.
    ///
    /// Only agent-role lanes produce anything. The orchestrator's own lane, the
    /// `── functions ──` divider and the function lanes beneath it used to be
    /// carried through as rows of their own; the rail lists sessions now, and
    /// none of those is one.
    fn split_fold(&self, lanes: &[AgentLane]) -> Vec<AgentGroup> {
        lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.role == AgentRole::Agent)
            .map(|(lane_index, lane)| {
                let mut group = self.group_for_lane(lane, lane_index);
                group.sessions = ordered_tasks(&lane.tasks)
                    .into_iter()
                    .map(|task| SessionRailRow {
                        agent_id: Some(group.row.agent_id.clone()),
                        lane_index: Some(lane_index),
                        task: Some(task),
                        local: None,
                        last: false,
                    })
                    .collect();
                group.visible_tasks = self.revealed_subtasks(&lane.key).min(group.sessions.len());
                group.hidden = group.sessions.len().saturating_sub(group.visible_tasks);
                group.overflow =
                    group.hidden > 0 || group.visible_tasks > crate::ui::app::input::SUBTASK_PAGE;
                group
            })
            .collect()
    }

    /// The group an agent-role lane opens.
    ///
    /// The lane's `agent_id` is the roster id the hub filed its tasks under, so
    /// it is also the key the projection's agent is matched by — the two cannot
    /// drift, because the roster is a projection of the declarations. The host id
    /// here is only a hint for a lane the projection turns out not to know; a
    /// placed agent takes its host from the tree.
    fn group_for_lane(&self, lane: &AgentLane, lane_index: usize) -> AgentGroup {
        let agent_id = lane
            .agent_id
            .clone()
            .unwrap_or_else(|| lane.key.trim_start_matches("agent:").to_string());
        let host_id = lane
            .descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.host_id.clone())
            .unwrap_or_default();
        AgentGroup {
            row: AgentRailRow {
                agent_id,
                host_id,
                agent: None,
                lane_index: Some(lane_index),
            },
            sessions: Vec::new(),
            last_at: lane.last_at,
            lane_label: Some(lane.label.clone()),
            harness_label: lane.harness_label.clone(),
            visible_tasks: 0,
            hidden: 0,
            overflow: false,
        }
    }

    /// Attach the live local sessions to their agents, returning the unclaimed.
    ///
    /// An unclaimed session runs in a directory nothing declares. It is listed at
    /// the end rather than dropped — a session that is running, costing tokens
    /// and invisible is the failure the old `── your harnesses ──` group existed
    /// to prevent — and it is what the inline create-agent offer is for.
    fn attach_sessions(&self, hosts: &mut [HostGroup]) -> Vec<SessionRailRow> {
        let declarations = self.local_agent_declarations();
        let mut groups: Vec<&mut AgentGroup> = hosts
            .iter_mut()
            .flat_map(|host| host.agents.iter_mut())
            .collect();
        let mut orphans = Vec::new();
        for row in self.own_session_rows() {
            // A dispatch this device is serving reaches the rail from both
            // surfaces at once: `split_fold` folded the task from the event
            // stream, and `own_session_rows` lists the live pty so the runs it
            // started have a row to nest under. They are one harness, so the
            // task row takes the local session rather than a second row for the
            // same process appearing beside it — which is exactly the "carries
            // either (or … both)" case [`SessionRailRow`] documents.
            if let Some(existing) = self.task_row_serving(&mut groups, &row.id) {
                existing.local = Some(row);
                continue;
            }
            let agent_id = resolve::agent_for_session(&declarations, &row)
                .map(|declaration| declaration.agent_id.clone());
            let index = agent_id.as_ref().and_then(|agent_id| {
                groups
                    .iter()
                    .position(|group| group.row.agent_id.trim() == agent_id.trim())
            });
            let session = SessionRailRow {
                agent_id,
                lane_index: index.and_then(|index| groups[index].row.lane_index),
                task: None,
                local: Some(row),
                last: false,
            };
            match index {
                Some(index) => groups[index].sessions.push(session),
                None => orphans.push(session),
            }
        }
        orphans
    }

    /// The already-placed task row this device serves with `session_id`, if any.
    ///
    /// Matched through the daemon's running map rather than by name or
    /// directory: `session_for_task` is the same lookup the harness pane
    /// resolves a task's screen with, so the two surfaces cannot disagree about
    /// which pty is serving a dispatch. Rows that already carry a local session
    /// are skipped, so two live sessions never collapse onto one task.
    ///
    /// `None` once the task settles — the runtime drops the record then — which
    /// is the right answer: a retained session outlives its task row and needs a
    /// row of its own.
    fn task_row_serving<'a>(
        &self,
        groups: &'a mut [&mut AgentGroup],
        session_id: &str,
    ) -> Option<&'a mut SessionRailRow> {
        let local_sessions = self.local_sessions.as_ref()?;
        task_row_serving(groups, session_id, |task_id| {
            local_sessions.session_for_task(task_id)
        })
    }

    /// Flatten the tree into rows, wrapping sessions in host rows when needed.
    ///
    /// The agent groups survive the flattening without being rendered: they are
    /// what decides a session's order and its lane, and the sessions of one
    /// agent still come out contiguous. What they no longer get is a row.
    fn flatten(
        &self,
        sections: Vec<organize::Section>,
        orphans: Vec<SessionRailRow>,
        lanes: &[AgentLane],
    ) -> Vec<RailRow> {
        let mut rows: Vec<RailRow> = Vec::new();
        // A device that hosts nothing has nowhere to start a session, so the
        // action is absent there rather than present and refusing.
        if self.local_sessions.is_some() {
            rows.push(RailRow::NewSession);
        }
        // Progressive disclosure: one host is the common case, and a permanent
        // `mac-studio ▸` wrapper would add a level of nesting to the surface an
        // operator uses most.
        for mut section in sections.into_iter().filter(|section| {
            section
                .agents
                .iter()
                .any(|group| !group.sessions.is_empty() || group.overflow)
        }) {
            match section.header {
                organize::SectionHeader::Host(host) => rows.push(RailRow::Host(host)),
                organize::SectionHeader::Group(group) => rows.push(RailRow::Group(group)),
                organize::SectionHeader::None => {}
            }
            for group in &mut section.agents {
                paging::push_group(
                    &mut rows,
                    group,
                    &self.harness_runs,
                    lanes,
                    self.agent_anchor.as_ref(),
                );
            }
        }
        for mut session in orphans {
            session.last = true;
            let session = Box::new(session);
            let runs = run_rows_under(&session, &self.harness_runs);
            rows.push(RailRow::Session(session));
            rows.extend(runs);
        }
        rows
    }

    /// How many local harnesses are waiting on the operator right now.
    ///
    /// Counts every live session on this device, not only the rows on the rail:
    /// a session the orchestrator started and then got stuck on a permission
    /// prompt is exactly the case an operator needs told about, and it may have
    /// no row of its own.
    ///
    /// The attached session is excluded. Its prompt is on screen in front of
    /// the person the count is for, so counting it would ask them to go and
    /// look at what they are already looking at.
    pub(in crate::ui) fn sessions_waiting(&self) -> usize {
        let Some(harnesses) = self.local_sessions.as_ref() else {
            return 0;
        };
        Self::count_waiting(&harnesses.sessions.waiting_sessions(), &self.harness_focus)
    }

    /// The same count from an already-collected waiting set.
    ///
    /// The rail collects that set anyway to style its rows, so the header count
    /// is derived from it instead of taking the lock a second time — and, more
    /// to the point, the header and the rows beneath it are then answering from
    /// one snapshot rather than from two taken a few microseconds apart.
    pub(in crate::ui) fn count_waiting(
        waiting: &std::collections::HashSet<String>,
        focus: &crate::ui::harness_pane::HarnessFocus,
    ) -> usize {
        waiting
            .iter()
            .filter(|id| !focus.is_attached_to(id))
            .count()
    }

    /// The sessions on this device the operator can act on directly.
    ///
    /// A dispatched session reaches the rail through its task while that task is
    /// running, folded from the event stream, so listing it here as well would
    /// show one session twice. What is left is the operator's own — started by
    /// them, or taken from the orchestrator — plus the *retained* ones, which
    /// are dispatched sessions whose task has finished.
    ///
    /// Retained sessions have to be here, and the task row is not a substitute.
    /// A task row carries no local session (`local: None`), so the cursor on one
    /// resolves no pty: the pane cannot draw the live screen and there is
    /// nothing to attach the keyboard to. The task's own screen stops arriving
    /// at the same moment for the same reason — `session_for_task` resolves
    /// through the daemon's *running* map, and the admission guard drops that
    /// record when the task settles. So a finished task's harness is alive and
    /// reachable by nothing until it is listed here, which is the whole point of
    /// having kept it.
    ///
    /// Exited ones do **not** stay listed. A finished session is history, and a
    /// rail that keeps every one of them buries the sessions that are still
    /// doing something under the ones that are not. The two cases where reading
    /// a dead session still matters keep it — the operator is attached to it, or
    /// a run it started is still executing — and [`cleanup`] is where that rule
    /// and the forgetting it drives are written down.
    ///
    /// And a dispatched session that started a workflow run is listed too, task
    /// row or not. The task row carries `local: None`, so it has no grant to key
    /// runs by ([`run_rows_under`]) — which meant the runs an orchestrator's own
    /// harnesses start, the majority of them, were the ones the rail could not
    /// show. A run is minutes-to-hours of work in another process; leaving it
    /// invisible is the same failure retention exists to prevent. That does not
    /// double the row: [`attach_sessions`](Self::attach_sessions) merges such a
    /// session into the task row already standing for it, which is what gives
    /// that row the grant it was missing.
    pub(super) fn own_session_rows(&self) -> Vec<SessionRow> {
        let Some(harnesses) = self.local_sessions.as_ref() else {
            return Vec::new();
        };
        let mut rows: Vec<SessionRow> = harnesses
            .sessions
            .rows()
            .into_iter()
            .filter(|row| {
                row.origin.is_user()
                    || row.control == crate::worker::pty::SessionControl::User
                    || row.retained
                    // Only while a run is *still going*: the rows a settled one
                    // would contribute are dropped by `run_rows_under`, so a
                    // session held open by finished runs would be a row with
                    // nothing under it.
                    || row
                        .mcp_grant_session
                        .as_deref()
                        .is_some_and(|grant| self.harness_runs.any_active_for_session(grant))
            })
            .filter(|row| row.state.is_running() || self.keeps_finished_session(row))
            .collect();
        rows.sort_by_key(|row| row.started_at);
        rows
    }
}

/// Place the folded lanes onto the shared `Host → Agent` tree.
///
/// The tree decides what exists and in what order — it is the same projection
/// the Hosts tab renders, so the two lenses list the same agents under the same
/// hosts. A lane is matched onto its agent by id and contributes only what the
/// projection cannot know: the transcript behind the row, and the tasks folded
/// under it.
///
/// A lane the tree does not know keeps a row of its own. That is not a leftover
/// case: an agent the backend rosters is not necessarily one this hub declares
/// or advertises, and a rail that dropped it would hide work that is running.
fn place_agents(tree: &[HostRow], folded: Vec<AgentGroup>) -> Vec<HostGroup> {
    let mut folded: Vec<Option<AgentGroup>> = folded.into_iter().map(Some).collect();
    let mut hosts: Vec<HostGroup> = tree
        .iter()
        .map(|host| HostGroup {
            row: HostRailRow {
                host_id: host.id.clone(),
                label: host.label.clone(),
                local: host.kind == HostKind::Local,
            },
            agents: host
                .agents
                .iter()
                .map(|agent| placed_agent(agent, &host.id, &mut folded))
                .collect(),
        })
        .collect();
    for group in folded.into_iter().flatten() {
        match unplaced_host(&hosts, &group.row.host_id) {
            Some(index) => hosts[index].agents.push(group),
            None => hosts.push(HostGroup {
                row: HostRailRow {
                    host_id: group.row.host_id.clone(),
                    label: "unplaced".to_string(),
                    local: false,
                },
                agents: vec![group],
            }),
        }
    }
    hosts
}

/// One agent of the tree, with its folded lane taken if it has one.
fn placed_agent(
    agent: &HostAgentRow,
    host_id: &str,
    folded: &mut [Option<AgentGroup>],
) -> AgentGroup {
    let taken = folded
        .iter_mut()
        .find(|group| {
            group
                .as_ref()
                .is_some_and(|group| group.row.agent_id.trim() == agent.agent_id.trim())
        })
        .and_then(Option::take);
    let mut group = taken.unwrap_or_else(|| AgentGroup {
        row: AgentRailRow {
            agent_id: agent.agent_id.clone(),
            host_id: host_id.to_string(),
            agent: None,
            lane_index: None,
        },
        sessions: Vec::new(),
        last_at: 0,
        lane_label: None,
        harness_label: None,
        visible_tasks: 0,
        hidden: 0,
        overflow: false,
    });
    group.row.host_id = host_id.to_string();
    group.row.agent = Some(agent.clone());
    group
}

/// Where a lane the tree does not know is drawn: the host it names if that host
/// is on the tree, else the machine looking at it.
///
/// `None` only when there is no host at all to hang it from, which is a device
/// that hosts nothing and has declared nothing.
fn unplaced_host(hosts: &[HostGroup], host_id: &str) -> Option<usize> {
    let host_id = host_id.trim();
    hosts
        .iter()
        .position(|host| !host_id.is_empty() && host.row.host_id.trim() == host_id)
        .or_else(|| hosts.iter().position(|host| host.row.local))
}
/// The placed task row `session_id` is serving, given a task-to-session lookup.
///
/// Split from the [`App`] method so the row-picking rule is testable without a
/// daemon: `served` is the only thing the real call needs a running hub for.
fn task_row_serving<'a>(
    groups: &'a mut [&mut AgentGroup],
    session_id: &str,
    served: impl Fn(&str) -> Option<String>,
) -> Option<&'a mut SessionRailRow> {
    groups
        .iter_mut()
        .flat_map(|group| group.sessions.iter_mut())
        .find(|session| {
            session.local.is_none()
                && session
                    .task
                    .as_ref()
                    .is_some_and(|task| served(&task.task_id).as_deref() == Some(session_id))
        })
}

/// The workflow-run rows that belong under one session, oldest first.
///
/// Keyed by the grant session recorded on the PTY row at launch — the same key
/// the MCP subprocess reports under — so a session Medulla did not spawn, or
/// one whose harness was never granted the workflow tools, simply has none.
///
/// Only the runs still executing. A settled run is finished work, and the rail
/// is about work in flight: its record is written to the workflow store the
/// moment it settles, and the Workflows page lists that history properly —
/// with the run's steps, its inputs, and its error — where a rail row could
/// only ever repeat the word "failed" until the session went away.
fn run_rows_under(
    session: &SessionRailRow,
    runs: &medulla::control_socket::HarnessRunRegistry,
) -> Vec<RailRow> {
    let Some(local) = &session.local else {
        return Vec::new();
    };
    let Some(grant) = local.mcp_grant_session.as_deref() else {
        return Vec::new();
    };
    let reported: Vec<_> = runs
        .for_session(grant)
        .into_iter()
        .filter(|run| !run.status.is_terminal())
        .collect();
    let last_index = reported.len().saturating_sub(1);
    reported
        .into_iter()
        .enumerate()
        .map(|(index, run)| {
            RailRow::WorkflowRun(WorkflowRunRailRow {
                session_id: local.id.clone(),
                run,
                // The session's own last-leaf glyph is decided before its runs
                // exist, so the group's real last row is the last run under it.
                last: index == last_index && session.last,
            })
        })
        .collect()
}
