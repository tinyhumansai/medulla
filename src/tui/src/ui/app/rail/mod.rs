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
//! **Agents come from declarations**, not from traffic: a lane is folded from
//! task events, so an agent nothing had been dispatched to produced no row at
//! all — which made the rail a list of what happened rather than of what exists.
//! Declarations are the source; lanes attach to them where they exist, and a lane
//! nothing declares still gets a row of its own so nothing that used to be
//! visible disappears.
//!
//! Row shapes live in [`types`]; the two resolution rules (session → agent,
//! agent → host) in [`resolve`]; this module is the assembly.

use std::collections::HashSet;

use medulla::config::agent_declarations_for_host;
use medulla::runtime::AgentDeclaration;

use super::types::App;
use crate::ui::agents::{AgentLane, AgentRole, AgentRow};
use crate::worker::pty::SessionRow;

pub(in crate::ui::app) mod resolve;
#[cfg(test)]
pub(in crate::ui::app) mod tests;
mod types;

pub use types::{AgentRailRow, HostRailRow, RailRow, SessionRailRow};

/// The label on the rail's "declare an agent" row.
///
/// It says *agent* rather than *harness* because that is what it produces: a
/// declared `harness × workspace` identity that outlives the session it starts.
pub(in crate::ui::app) const NEW_AGENT_LABEL: &str = "+ New agent";

/// The most sessions listed under one agent before the rest are counted.
const MAX_SESSIONS_PER_AGENT: usize = 8;

/// One agent and the sessions hanging off it, before the tree is flattened.
struct AgentGroup {
    /// The agent row itself.
    row: AgentRailRow,
    /// Its sessions, dispatched and operator-started alike.
    sessions: Vec<SessionRailRow>,
    /// Sessions the fold's own cap already hid, carried so the counts add up.
    hidden: usize,
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
    /// the agent lanes become groups keyed by agent id. Then the declarations
    /// this machine holds are folded in, adding every agent with no traffic. Last
    /// the live PTY sessions are attached to whichever agent declares the
    /// directory they run in, and the whole thing is flattened under host rows —
    /// which appear only when there is more than one machine to tell apart.
    pub(super) fn rail_rows(&self) -> Vec<RailRow> {
        let lanes = self.lanes();
        let (lane_rows, mut groups) = self.split_fold(&lanes);
        self.add_declared_agents(&mut groups);
        let orphans = self.attach_sessions(&mut groups);
        self.flatten(lane_rows, groups, orphans)
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
    /// it is also the key a declaration is looked up by — the two cannot drift,
    /// because the roster is a projection of the declarations.
    fn group_for_lane(&self, lane: &AgentLane, lane_index: usize) -> AgentGroup {
        let agent_id = lane
            .agent_id
            .clone()
            .unwrap_or_else(|| lane.key.trim_start_matches("agent:").to_string());
        let declaration =
            medulla::config::agent_declaration(self.agent_declarations(), &agent_id).cloned();
        let host_id = declaration
            .as_ref()
            .map(|declaration| declaration.host_id.clone())
            .or_else(|| {
                lane.descriptor
                    .as_ref()
                    .and_then(|descriptor| descriptor.host_id.clone())
            })
            .unwrap_or_default();
        AgentGroup {
            row: AgentRailRow {
                agent_id,
                host_id,
                declaration,
                lane_index: Some(lane_index),
            },
            sessions: Vec::new(),
            hidden: 0,
        }
    }

    /// Add a row for every declared agent that has no lane.
    ///
    /// This is the whole point of sourcing the rail from declarations: an agent
    /// you declared and have not dispatched to is a real, targetable thing, and a
    /// rail that only lists traffic cannot show it.
    ///
    /// Every declaration, not only this machine's: the tab *is* the topology, so
    /// an agent declared against another host belongs on it — under that host's
    /// row — even before the link handshake starts exchanging rosters. Creating
    /// one is still local-only, which the create flow enforces rather than this.
    fn add_declared_agents(&self, groups: &mut Vec<AgentGroup>) {
        let seen: HashSet<String> = groups
            .iter()
            .map(|group| group.row.agent_id.trim().to_string())
            .collect();
        for declaration in self.agent_declarations().iter().cloned() {
            if seen.contains(declaration.agent_id.trim()) {
                continue;
            }
            groups.push(AgentGroup {
                row: AgentRailRow {
                    agent_id: declaration.agent_id.clone(),
                    host_id: declaration.host_id.clone(),
                    declaration: Some(declaration),
                    lane_index: None,
                },
                sessions: Vec::new(),
                hidden: 0,
            });
        }
    }

    /// Attach the live local sessions to their agents, returning the unclaimed.
    ///
    /// An unclaimed session runs in a directory nothing declares. It is listed at
    /// the end rather than dropped — a harness that is running, costing tokens
    /// and invisible is the failure the old `── your harnesses ──` group existed
    /// to prevent — and it is what the inline create-agent offer is for.
    fn attach_sessions(&self, groups: &mut [AgentGroup]) -> Vec<SessionRailRow> {
        let declarations = self.local_agent_declarations();
        let mut orphans = Vec::new();
        for row in self.own_session_rows() {
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
        mut groups: Vec<AgentGroup>,
        orphans: Vec<SessionRailRow>,
    ) -> Vec<RailRow> {
        let mut rows: Vec<RailRow> = lane_rows.into_iter().map(RailRow::Lane).collect();
        // A device that hosts nothing cannot declare an agent on itself, so the
        // action is absent there rather than present and refusing.
        if self.harnesses.is_some() {
            rows.push(RailRow::NewAgent);
        }
        let local = self.local_host_id();
        let local = local.trim();
        let show_hosts =
            resolve::has_remote_host(groups.iter().map(|group| group.row.host_id.as_str()), local);
        for host_id in host_order(&groups, local) {
            if show_hosts {
                let is_local = host_id == local;
                rows.push(RailRow::Host(HostRailRow {
                    label: resolve::host_label(&host_id, is_local),
                    host_id: host_id.clone(),
                    local: is_local,
                }));
            }
            for group in groups
                .iter_mut()
                .filter(|group| placed_on(&group.row.host_id, local) == host_id)
            {
                push_group(&mut rows, group);
            }
        }
        rows.extend(orphans.into_iter().map(|mut session| {
            session.last = true;
            RailRow::Session(Box::new(session))
        }));
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
    pub(in crate::ui) fn harnesses_waiting(&self) -> usize {
        let Some(harnesses) = self.harnesses.as_ref() else {
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

    /// The sessions on this device that no dispatched task already describes.
    ///
    /// A dispatched session reaches the rail through its task, folded from the
    /// event stream; listing it here as well would show one session twice. What
    /// is left is the operator's own — started by them, or taken from the
    /// orchestrator — which nothing else on this device can see.
    ///
    /// Exited ones stay listed: the last screen is often the reason it exited,
    /// and a row that vanishes on failure is a row that hides the failure. They
    /// leave when the operator forgets them.
    pub(super) fn own_session_rows(&self) -> Vec<SessionRow> {
        let Some(harnesses) = self.harnesses.as_ref() else {
            return Vec::new();
        };
        let mut rows: Vec<SessionRow> = harnesses
            .sessions
            .rows()
            .into_iter()
            .filter(|row| {
                row.origin.is_user() || row.control == crate::worker::pty::HarnessControl::User
            })
            .collect();
        rows.sort_by_key(|row| row.started_at);
        rows
    }
}

/// The host an agent is drawn under: its own, or the local one when it names
/// none. An agent nothing places belongs to the machine looking at it.
fn placed_on(host_id: &str, local: &str) -> String {
    let host_id = host_id.trim();
    if host_id.is_empty() {
        local.to_string()
    } else {
        host_id.to_string()
    }
}

/// The hosts to draw, local first and the rest in the order their agents appear.
///
/// Sorting by host id would reorder the rail whenever a peer's key happened to
/// sort differently from the last machine that connected.
fn host_order(groups: &[AgentGroup], local: &str) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for group in groups {
        let host_id = placed_on(&group.row.host_id, local);
        if !order.contains(&host_id) {
            order.push(host_id);
        }
    }
    order.sort_by_key(|host_id| host_id.as_str() != local);
    order
}

/// Push one agent row and the sessions under it, capped and tree-marked.
fn push_group(rows: &mut Vec<RailRow>, group: &mut AgentGroup) {
    rows.push(RailRow::Agent(group.row.clone()));
    let shown = group.sessions.len().min(MAX_SESSIONS_PER_AGENT);
    let hidden = group.hidden + (group.sessions.len() - shown);
    for (index, session) in group.sessions.iter_mut().take(shown).enumerate() {
        session.last = hidden == 0 && index + 1 == shown;
        rows.push(RailRow::Session(Box::new(session.clone())));
    }
    if hidden > 0 {
        rows.push(RailRow::Lane(AgentRow::More {
            lane_index: group.row.lane_index.unwrap_or(0),
            hidden,
        }));
    }
}
