//! Appearance configuration types for resource displays and Agents-sidebar layout.

use serde::{Deserialize, Serialize};

/// Formats available for one local-process resource in the TUI status line.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResourceDisplay {
    /// Do not render this resource.
    #[default]
    Off,
    /// Render the resource as a percentage.
    Percent,
    /// Render the resource's native value, such as bytes or bytes per second.
    Value,
    /// Render a compact bar with a percentage or rate beside it.
    Bar,
}

/// How the Agents sidebar sections its agent rows.
///
/// The sidebar is a `Host → Agent → Session` tree; this chooses what the top
/// level is. Only the *sectioning* changes — every agent keeps its own sessions
/// under it — so no row disappears whichever way it is set.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidebarGrouping {
    /// Section by host, and only once a second host exists. The default: with
    /// one machine the header would say nothing the operator does not know.
    #[default]
    Host,
    /// Section by the directory an agent works in, so one checkout's agents read
    /// together however many hosts or harnesses they span.
    Path,
    /// Section by the harness an agent runs (`claude`, `codex`, …).
    Harness,
    /// No section headers at all — one flat list of agents.
    None,
}

impl SidebarGrouping {
    /// The label shown in Settings → Appearance.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Path => "path",
            Self::Harness => "harness",
            Self::None => "none",
        }
    }

    /// Every value, in the order the settings row cycles through them.
    pub const ALL: [Self; 4] = [Self::Host, Self::Path, Self::Harness, Self::None];
}

/// How the Agents sidebar orders the rows inside one section.
///
/// Applied at both levels the operator reads: the agents in a section, and the
/// sessions under an agent.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidebarSort {
    /// Declaration order for agents, start time for sessions — oldest first.
    /// The default, because it is the only order that does not move under you
    /// while you read it.
    #[default]
    Created,
    /// Most recently active first: the session that last produced output, and
    /// the agent whose most recent session did.
    Recent,
    /// Alphabetical by label.
    Name,
}

impl SidebarSort {
    /// The label shown in Settings → Appearance.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Recent => "recent",
            Self::Name => "name",
        }
    }

    /// Every value, in the order the settings row cycles through them.
    pub const ALL: [Self; 3] = [Self::Created, Self::Recent, Self::Name];
}

/// TUI display preferences retained under the `[appearance]` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceConfig {
    /// Whether orchestrator-managed harness titles appear in the Agents sidebar.
    pub show_session_titles: bool,
    /// Whether an operator-started harness row shows its Git branch.
    pub show_harness_branch: bool,
    /// Whether an operator-started harness row shows its shortened working path.
    pub show_harness_path: bool,
    /// How to show this process's CPU utilization.
    pub cpu: ResourceDisplay,
    /// How to show this process's resident memory.
    pub ram: ResourceDisplay,
    /// How to show this process's read/write throughput.
    pub disk_io: ResourceDisplay,
    /// How to show whole-device CPU utilization in the Agents sidebar.
    pub device_cpu: ResourceDisplay,
    /// How to show whole-device memory pressure in the Agents sidebar.
    pub device_ram: ResourceDisplay,
    /// How to show whole-device disk-capacity pressure in the Agents sidebar.
    pub device_disk: ResourceDisplay,
    /// What the Agents sidebar sections its agents by.
    pub sidebar_grouping: SidebarGrouping,
    /// How the Agents sidebar orders agents and the sessions under them.
    pub sidebar_sort: SidebarSort,
}

impl AppearanceConfig {
    /// Defaults used when the section is absent, including legacy harness fields.
    pub const fn with_defaults() -> Self {
        Self {
            show_session_titles: true,
            show_harness_branch: true,
            show_harness_path: true,
            cpu: ResourceDisplay::Off,
            ram: ResourceDisplay::Off,
            disk_io: ResourceDisplay::Off,
            device_cpu: ResourceDisplay::Off,
            device_ram: ResourceDisplay::Off,
            device_disk: ResourceDisplay::Off,
            sidebar_grouping: SidebarGrouping::Host,
            sidebar_sort: SidebarSort::Created,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self::with_defaults()
    }
}
