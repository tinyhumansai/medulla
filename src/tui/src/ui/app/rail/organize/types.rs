//! Data types that describe the rail's organized section tree.

use super::super::{AgentGroup, GroupRailRow, HostRailRow};

/// What heads one section of the rail.
pub(in crate::ui::app::rail) enum SectionHeader {
    /// A host row — emitted only when a second host exists to tell it from.
    Host(HostRailRow),
    /// A grouping header: a workspace directory, or a harness name.
    Group(GroupRailRow),
    /// No header at all: the agents sit at the top level.
    None,
}

/// One section of the rail: a header and the agents under it.
pub(in crate::ui::app::rail) struct Section {
    /// What heads it, if anything.
    pub header: SectionHeader,
    /// The agents in it, already ordered.
    pub agents: Vec<AgentGroup>,
}
