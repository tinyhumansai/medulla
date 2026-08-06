//! The Agents rail: one cursor over the whole `Host → Agent → Session` tree.
//!
//! The rail used to concatenate two lists — the lanes the event fold produced,
//! and the harnesses the operator had started under a `── your harnesses ──`
//! divider — and that split is exactly what the agent/session redefinition
//! removes. A task *is* an agent session; a harness is not an entity at all, only
//! the type an agent runs. So the rail now renders one tree:
//!
//! ```text
//! ◆ orchestrator            ← the conversation (not an agent)
//! + New agent               ← declares one on this machine
//! ▸ this device             ← host row, only when a remote host exists
//!   ● medulla-claude        ← DECLARED agent: present with zero sessions
//!     ├ t_41 · running      ← a session the orchestrator dispatched
//!     └ debug login         ← a session the operator started
//! ```
//!
//! **Agents come from the tree, not from traffic**: a lane is folded from task
//! events, so an agent nothing had been dispatched to produced no row at all —
//! which made the rail a list of what happened rather than of what exists.
//!
//! The host and agent levels are the shared `Host → Agent` projection
//! ([`medulla::ui::hosts::host_rows`]) — literally the same call the Hosts tab
//! renders, so the two lenses cannot disagree about what exists. Lanes attach to
//! the agents it produces; a lane for an agent the projection does not know (a
//! backend-side roster agent, a peer session) still gets a row of its own, so
//! nothing that used to be visible disappears. **Sessions** are the rail's own
//! level and are resolved here: a dispatched one by the roster id the hub filed
//! its task under, an operator-started one by [`resolve::agent_for_session`].
//!
//! Row shapes live in [`types`]; the session → agent rule in [`resolve`]; this
//! module is the assembly.

use medulla::config::agent_declarations_for_host;
use medulla::runtime::AgentDeclaration;
use medulla::ui::hosts::{HostAgentRow, HostKind, HostRow};

use super::types::App;
use crate::ui::agents::{AgentLane, AgentRole, AgentRow};
use crate::worker::pty::SessionRow;

pub(in crate::ui::app) mod resolve;
#[cfg(test)]
pub(in crate::ui::app) mod tests;
mod types;

pub use types::{AgentRailRow, HostRailRow, RailRow, SessionRailRow, WorkflowRunRailRow};

/// The label on the rail's "declare an agent" row.
///
/// It says *agent* rather than *harness* because that is what it produces: a
/// declared `harness × workspace` identity that outlives the session it starts.
pub(in crate::ui::app) const NEW_AGENT_LABEL: &str = "+ New agent";

/// The label on the action row that opens a session under an agent.
///
/// Indented and lower-cased beside [`NEW_AGENT_LABEL`] because it is a leaf of
/// one agent's group rather than an action on the machine.
pub(in crate::ui::app) const NEW_SESSION_LABEL: &str = "+ new session";

/// One agent and the sessions hanging off it, before the tree is flattened.
struct AgentGroup {
    /// The agent row itself.
    row: AgentRailRow,
    /// Its sessions, dispatched and operator-started alike.
    sessions: Vec<SessionRailRow>,
    /// Sessions the fold's own page already hid, carried so the counts add up.
    hidden: usize,
    /// Whether the fold drew an overflow row under this agent's lane.
    ///
    /// The rail does **not** re-cap what the fold already paged (#171): the fold
    /// reveals `SUBTASK_PAGE` sessions per page and decides when the `+N more`
    /// row exists, including the fully-revealed case where it is instead the
    /// `show less` control and `hidden` is zero. A second cap here would clip
    /// below the page the operator just asked for, so this only records that the
    /// row is owed.
    overflow: bool,
}

/// One host and the agents placed on it, before the tree is flattened.
struct HostGroup {
    /// The host row, drawn only once there is more than one of them.
    row: HostRailRow,
    /// Its agents, in the order the shared projection lists them.
    agents: Vec<AgentGroup>,
}

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
        let (lane_rows, folded) = self.split_fold(&lanes);
        let mut hosts = place_agents(&self.host_tree(), folded);
        let orphans = self.attach_sessions(&mut hosts);
        self.flatten(lane_rows, hosts, orphans)
    }

    /// Split the folded rows into the non-agent ones and the per-agent groups.
    fn split_fold(&self, lanes: &[AgentLane]) -> (Vec<AgentRow>, Vec<AgentGroup>) {
        let mut lane_rows: Vec<AgentRow> = Vec::new();
        let mut groups: Vec<AgentGroup> = Vec::new();
        for row in self.agent_rows() {
            match row {
                AgentRow::Lane { lane_index } => {
                    let Some(lane) = lanes.get(lane_index) else {
                        continue;
                    };
                    if lane.role != AgentRole::Agent {
                        lane_rows.push(row);
                        continue;
                    }
                    groups.push(self.group_for_lane(lane, lane_index));
                }
                AgentRow::Sub {
                    lane_index, task, ..
                } => {
                    let Some(group) = groups.last_mut() else {
                        continue;
                    };
                    group.sessions.push(SessionRailRow {
                        agent_id: Some(group.row.agent_id.clone()),
                        lane_index: Some(lane_index),
                        task: Some(task),
                        local: None,
                        last: false,
                    });
                }
                AgentRow::More { hidden, .. } => {
                    if let Some(group) = groups.last_mut() {
                        group.hidden += hidden;
                        group.overflow = true;
                    }
                }
                AgentRow::Separator => lane_rows.push(row),
            }
        }
        (lane_rows, groups)
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

    /// Flatten the tree into rows, wrapping agents in host rows when needed.
    fn flatten(
        &self,
        lane_rows: Vec<AgentRow>,
        hosts: Vec<HostGroup>,
        orphans: Vec<SessionRailRow>,
    ) -> Vec<RailRow> {
        let mut rows: Vec<RailRow> = lane_rows.into_iter().map(RailRow::Lane).collect();
        // A device that hosts nothing cannot declare an agent on itself, so the
        // action is absent there rather than present and refusing.
        let hosting = self.local_sessions.is_some();
        if hosting {
            rows.push(RailRow::NewAgent);
        }
        // Only over a tree that exists — see [`RailRow::AgentsHeader`]. Counted
        // from the groups rather than from `rows`, because the host rows that
        // wrap them have not been pushed yet.
        if hosts.iter().any(|host| !host.agents.is_empty()) {
            rows.push(RailRow::AgentsHeader);
        }
        // Which agents this machine may open a session under: a session is
        // started by the host that owns the agent, so only the agents declared
        // here get the action. Collected once rather than re-scanned per group.
        let declared: Vec<String> = if hosting {
            self.local_agent_declarations()
                .into_iter()
                .map(|declaration| declaration.agent_id)
                .collect()
        } else {
            Vec::new()
        };
        // Progressive disclosure: one host is the common case, and a permanent
        // `mac-studio ▸` wrapper would add a level of nesting to the surface an
        // operator uses most.
        let show_hosts = hosts.len() > 1;
        for mut host in hosts {
            if show_hosts {
                rows.push(RailRow::Host(host.row));
            }
            for group in &mut host.agents {
                let offers_session = declared
                    .iter()
                    .any(|agent_id| agent_id.trim() == group.row.agent_id.trim());
                push_group(&mut rows, group, offers_session, &self.harness_runs);
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
    /// Exited ones stay listed: the last screen is often the reason it exited,
    /// and a row that vanishes on failure is a row that hides the failure. They
    /// leave when the operator forgets them.
    ///
    /// And a dispatched session that started a workflow run is listed too, task
    /// row or not. The task row carries `local: None`, so it has no grant to key
    /// runs by ([`run_rows_under`]) — which meant the runs an orchestrator's own
    /// harnesses start, the majority of them, were the ones the rail could not
    /// show. A run is minutes-to-hours of work in another process; leaving it
    /// invisible is the same failure retention exists to prevent.
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
                    || row
                        .mcp_grant_session
                        .as_deref()
                        .is_some_and(|grant| self.harness_runs.any_for_session(grant))
            })
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
/// The workflow-run rows that belong under one session, oldest first.
///
/// Keyed by the grant session recorded on the PTY row at launch — the same key
/// the MCP subprocess reports under — so a session Medulla did not spawn, or
/// one whose harness was never granted the workflow tools, simply has none.
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
    let reported = runs.for_session(grant);
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

/// Push one agent row and the sessions under it, tree-marked.
///
/// `offers_session` closes the group with the `+ New session` action. It is off
/// for an agent this machine does not declare — a remote host's agent, or a lane
/// the fold produced for an agent declared somewhere else — because the flow it
/// opens reads the declaration for the harness and the directory to start in.
///
/// Paging is the fold's, not the rail's (#171): `agent_rows` reveals a page of
/// task sublanes at a time and marks the rest with an overflow row, so a second
/// cap here would clip the page the operator just asked to see. The overflow row
/// is re-emitted under the group and stays selectable, which is what makes
/// `Enter` on it page the lane open — and, once the lane is fully revealed, fold
/// it back.
fn push_group(
    rows: &mut Vec<RailRow>,
    group: &mut AgentGroup,
    offers_session: bool,
    runs: &medulla::control_socket::HarnessRunRegistry,
) {
    rows.push(RailRow::Agent(group.row.clone()));
    let shown = group.sessions.len();
    for (index, session) in group.sessions.iter_mut().enumerate() {
        // The action row below closes the group when it is offered, so the last
        // session is only the tree's last leaf when neither it nor the overflow
        // row follows.
        session.last = !offers_session && !group.overflow && index + 1 == shown;
        let session = Box::new(session.clone());
        let run_rows = run_rows_under(&session, runs);
        rows.push(RailRow::Session(session));
        rows.extend(run_rows);
    }
    if group.overflow {
        rows.push(RailRow::Lane(AgentRow::More {
            lane_index: group.row.lane_index.unwrap_or(0),
            hidden: group.hidden,
        }));
    }
    // Last, under the sessions it adds to: the group reads as a list of what
    // this agent is running, and the action that starts one more belongs at the
    // end of that list rather than above it.
    if offers_session {
        rows.push(RailRow::NewSession {
            agent_id: group.row.agent_id.clone(),
        });
    }
}
