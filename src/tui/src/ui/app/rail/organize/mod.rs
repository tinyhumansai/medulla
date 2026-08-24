//! How the rail's tree is sectioned and ordered before it is flattened.
//!
//! The assembly in [`super`] decides *what exists*: which agents, on which
//! hosts, with which sessions under them. This module decides how that is
//! presented — the two questions an operator with more than a handful of agents
//! actually asks, and the two the Appearance page now exposes:
//!
//! * **Grouping** picks the top level. By host (the default, and drawn only once
//!   a second host exists), by the directory an agent works in, by the harness it
//!   runs, or not at all. Only the *headers* change: every agent keeps its own
//!   sessions, so nothing that had a row loses one whichever way it is set.
//! * **Sorting** orders the rows inside a section, at both levels the eye reads:
//!   the agents in a section, and the sessions under an agent.
//!
//! Kept apart from the assembly because they are genuinely separate concerns —
//! the placement rules answer to the roster and the PTY manager, these answer to
//! one config value each — and because the assembly file was already at this
//! repository's size ceiling.

use medulla::config::{SidebarGrouping, SidebarSort};
use medulla::runtime::AgentDeclaration;

use super::{AgentGroup, GroupRailRow, HostGroup, SessionRailRow};

#[cfg(test)]
mod tests;
mod types;

pub(super) use types::{Section, SectionHeader};

/// Section and order the placed tree according to the operator's preferences.
///
/// Host grouping keeps the tree as placed; every other grouping flattens the
/// hosts away first, because an agent's directory or harness is a fact about the
/// agent rather than about the machine, and keeping both levels would mean
/// reading the same checkout twice under two hosts.
pub(super) fn organize(
    hosts: Vec<HostGroup>,
    declarations: &[AgentDeclaration],
    grouping: SidebarGrouping,
    sort: SidebarSort,
) -> Vec<Section> {
    let mut sections = match grouping {
        SidebarGrouping::Host => by_host(hosts),
        SidebarGrouping::Path => by_key(
            flatten_agents(hosts, declarations, sort),
            agent_path,
            str::eq,
        ),
        SidebarGrouping::Harness => by_key(
            flatten_agents(hosts, declarations, sort),
            agent_harness,
            str::eq_ignore_ascii_case,
        ),
        SidebarGrouping::None => vec![Section {
            header: SectionHeader::None,
            agents: flatten_agents(hosts, declarations, sort),
        }],
    };
    for section in &mut sections {
        sort_agents(&mut section.agents, sort);
        for agent in &mut section.agents {
            sort_sessions(&mut agent.sessions, sort);
        }
    }
    order_sections(&mut sections, sort);
    sections
}

/// Keep the placed tree, dropping the header when there is only one host.
///
/// Progressive disclosure, unchanged from before grouping was configurable: with
/// just the local machine a permanent `mac-studio ▸` wrapper would add a level of
/// nesting to the surface an operator uses most.
fn by_host(hosts: Vec<HostGroup>) -> Vec<Section> {
    let show = hosts.len() > 1;
    hosts
        .into_iter()
        .map(|host| Section {
            header: if show {
                SectionHeader::Host(host.row)
            } else {
                SectionHeader::None
            },
            agents: host.agents,
        })
        .collect()
}

/// Section every agent by one derived key, preserving first-seen order.
fn by_key(
    agents: Vec<AgentGroup>,
    key: fn(&AgentGroup) -> String,
    keys_match: fn(&str, &str) -> bool,
) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for agent in agents {
        let label = key(&agent);
        match sections.iter_mut().find(|section| match &section.header {
            SectionHeader::Group(group) => keys_match(&group.label, &label),
            _ => false,
        }) {
            Some(section) => section.agents.push(agent),
            None => sections.push(Section {
                header: SectionHeader::Group(GroupRailRow {
                    label: label.clone(),
                }),
                agents: vec![agent],
            }),
        }
    }
    sections
}

/// Flatten host sections, restoring declaration order before any non-host
/// grouping can interleave agents from separate hosts.
fn flatten_agents(
    hosts: Vec<HostGroup>,
    declarations: &[AgentDeclaration],
    sort: SidebarSort,
) -> Vec<AgentGroup> {
    let mut agents: Vec<_> = hosts.into_iter().flat_map(|host| host.agents).collect();
    if sort == SidebarSort::Created {
        agents.sort_by_key(|agent| {
            declarations
                .iter()
                .position(|declaration| declaration.agent_id.trim() == agent.row.agent_id.trim())
                .unwrap_or(usize::MAX)
        });
    }
    agents
}

/// The directory an agent works in, as its section label.
///
/// An agent the projection knows nothing about — a lane the backend rosters,
/// a peer session — has no directory, and is collected under one heading rather
/// than given a section of its own per agent.
fn agent_path(agent: &AgentGroup) -> String {
    agent
        .row
        .workspace()
        .map(normalize_path_label)
        .filter(|workspace| !workspace.is_empty())
        .unwrap_or_else(|| "no path".to_string())
}

/// Normalize a workspace directory for grouping without changing the root.
///
/// Declarations and spawned working directories commonly differ only by a
/// trailing separator. Grouping uses the same comparison rule as session
/// ownership resolution so that representation detail does not create a second
/// section header.
fn normalize_path_label(path: &str) -> String {
    let trimmed = path.trim();
    let stripped = trimmed.trim_end_matches('/');
    if stripped.is_empty() {
        trimmed.to_string()
    } else {
        stripped.to_string()
    }
}

/// The harness an agent runs, as its section label.
fn agent_harness(agent: &AgentGroup) -> String {
    agent
        .row
        .harness()
        .or(agent.harness_label.as_deref())
        .map(str::trim)
        .filter(|harness| !harness.is_empty())
        .map_or_else(|| "no harness".to_string(), str::to_string)
}

/// Order the sections themselves.
///
/// Alphabetical for the orders that are about identity, most-recently-active
/// first for the one that is about activity. Host sections are left in the
/// projection's order whatever the sort: that order is the tree's, shared with
/// the Hosts tab, and the two lenses are meant to list hosts the same way.
fn order_sections(sections: &mut [Section], sort: SidebarSort) {
    if sections
        .iter()
        .any(|section| !matches!(section.header, SectionHeader::Group(_)))
    {
        return;
    }
    match sort {
        SidebarSort::Recent => sections.sort_by_key(|section| {
            std::cmp::Reverse(
                section
                    .agents
                    .iter()
                    .map(agent_activity)
                    .max()
                    .unwrap_or(i64::MIN),
            )
        }),
        SidebarSort::Created | SidebarSort::Name => {
            sections.sort_by_key(|section| match &section.header {
                SectionHeader::Group(group) => group.label.to_lowercase(),
                _ => String::new(),
            })
        }
    }
}

/// Order the agents inside one section.
///
/// `Created` leaves the projection's order alone — declaration order, which is
/// what the operator wrote — because an agent has no creation timestamp to sort
/// by and re-ordering it would only shuffle the list without answering anything.
fn sort_agents(agents: &mut [AgentGroup], sort: SidebarSort) {
    match sort {
        SidebarSort::Created => {}
        SidebarSort::Recent => {
            agents.sort_by_key(|agent| std::cmp::Reverse(agent_activity(agent)));
        }
        SidebarSort::Name => agents.sort_by_key(|agent| agent.label().to_lowercase()),
    }
}

/// Order the sessions under one agent.
///
/// A dispatched session with no live pty carries no start time, so `Created`
/// sorts it to the front and the stable sort leaves those in the order the fold
/// produced them — which is the order the rail listed them in before grouping
/// existed, so the default arrangement is unchanged.
pub(super) fn sort_sessions(sessions: &mut [SessionRailRow], sort: SidebarSort) {
    match sort {
        SidebarSort::Created => sessions.sort_by_key(session_started),
        SidebarSort::Recent => {
            sessions.sort_by_key(|session| std::cmp::Reverse(session_activity(session)));
        }
        SidebarSort::Name => sessions.sort_by_key(|session| session_label(session).to_lowercase()),
    }
}

/// When a local-only session started, or [`i64::MIN`] for dispatches.
///
/// A task may be enriched with a local PTY after `ordered_tasks` established
/// its running-first fold order. Treating that row as locally started would
/// reorder it behind task-only rows, so task-backed rows deliberately retain
/// their stable fold position.
fn session_started(session: &SessionRailRow) -> i64 {
    if session.task.is_some() {
        i64::MIN
    } else {
        session
            .local
            .as_ref()
            .map_or(i64::MIN, |local| local.started_at)
    }
}

/// The most recent thing known about a session: its last output byte, or the
/// last event its task folded.
fn session_activity(session: &SessionRailRow) -> i64 {
    let local = session.local.as_ref().map(|local| local.last_output_at);
    let task = session.task.as_ref().map(|task| task.last_at);
    local.into_iter().chain(task).max().unwrap_or(i64::MIN)
}

/// What a session sorts as by name: what the operator called it, else its task,
/// then its terminal title, else the pty's own label.
fn session_label(session: &SessionRailRow) -> String {
    if let Some(name) = session.name() {
        return name.to_string();
    }
    if let Some(task) = &session.task {
        return task.task_id.clone();
    }
    session
        .local
        .as_ref()
        .map(|local| {
            local
                .thread_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&local.label)
                .to_string()
        })
        .unwrap_or_default()
}

/// The most recent activity under an agent, for the activity orders.
fn agent_activity(agent: &AgentGroup) -> i64 {
    std::iter::once(agent.last_at)
        .chain(agent.sessions.iter().map(session_activity))
        .max()
        .unwrap_or(i64::MIN)
}
