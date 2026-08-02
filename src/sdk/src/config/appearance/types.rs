//! Data types for selecting process and whole-device resource display formats.

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

/// TUI display preferences retained under the `[appearance]` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceConfig {
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
}

impl AppearanceConfig {
    /// Defaults used when the section is absent, including legacy harness fields.
    pub const fn with_defaults() -> Self {
        Self {
            show_harness_branch: true,
            show_harness_path: true,
            cpu: ResourceDisplay::Off,
            ram: ResourceDisplay::Off,
            disk_io: ResourceDisplay::Off,
            device_cpu: ResourceDisplay::Off,
            device_ram: ResourceDisplay::Off,
            device_disk: ResourceDisplay::Off,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self::with_defaults()
    }
}
