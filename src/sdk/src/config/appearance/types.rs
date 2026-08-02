//! Data types for selecting CPU, memory, and disk-I/O display formats.

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

/// Optional local-process resource indicators shown in the TUI status line.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceConfig {
    /// How to show this process's CPU utilization.
    pub cpu: ResourceDisplay,
    /// How to show this process's resident memory.
    pub ram: ResourceDisplay,
    /// How to show this process's read/write throughput.
    pub disk_io: ResourceDisplay,
}
