//! Resource sampler timing configuration.

use serde::{Deserialize, Serialize};

/// Default resource sample interval, in milliseconds.
pub const DEFAULT_RESOURCES_UPDATE_MS: u64 = 2_000;
/// Smallest supported resource sample interval, in milliseconds.
pub const MIN_RESOURCES_UPDATE_MS: u64 = 100;
/// Largest supported resource sample interval, in milliseconds.
pub const MAX_RESOURCES_UPDATE_MS: u64 = 10_000;

fn default_update_ms() -> u64 {
    DEFAULT_RESOURCES_UPDATE_MS
}
fn default_shown_boxes() -> String {
    "cpu,mem,net,agents".into()
}
fn default_presets() -> String {
    "cpu:0:default,mem:0:default,net:0:default,agents:1:default".into()
}

fn deserialize_update_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    Ok(value.clamp(MIN_RESOURCES_UPDATE_MS, MAX_RESOURCES_UPDATE_MS))
}

/// Settings for the background resource sampler.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourcesConfig {
    /// Milliseconds between system resource samples.
    #[serde(
        default = "default_update_ms",
        deserialize_with = "deserialize_update_ms"
    )]
    pub update_ms: u64,
    /// Comma-separated boxes visible when the tab first opens.
    #[serde(default = "default_shown_boxes")]
    pub shown_boxes: String,
    /// Whitespace-separated btop-style box presets.
    #[serde(default = "default_presets")]
    pub presets: String,
    /// Index of the selected preset in `presets`.
    pub preset: usize,
    /// Explicit CPU placement override. When absent, the selected preset wins.
    pub cpu_bottom: Option<bool>,
    /// Explicit Mem/Net placement override. When absent, the selected preset wins.
    pub mem_below_net: Option<bool>,
    /// Explicit Agents placement override. When absent, the selected preset wins.
    pub agents_left: Option<bool>,
    /// CPU field drawn by the upper graph.
    pub cpu_graph_upper: String,
    /// CPU field drawn by the lower graph.
    pub cpu_graph_lower: String,
    /// Draw the lower CPU graph upside down.
    pub cpu_invert_lower: bool,
    /// Collapse CPU fields into one graph.
    pub cpu_single_graph: bool,
    /// Temperature scale (`celsius` or `fahrenheit`).
    pub temp_scale: String,
    /// Frequency presentation (`auto`, `mhz`, or `ghz`).
    pub freq_mode: String,
    /// Show battery percentage when the platform reports it.
    pub show_battery: bool,
    /// Draw memory and swap history as graphs.
    pub mem_graphs: bool,
    /// Show mounted disks in the memory panel.
    pub show_disks: bool,
    /// Show swap usage in the memory panel.
    pub show_swap: bool,
    /// Mount a swap device as a disk row when available.
    pub swap_disk: bool,
    /// Hide virtual and loop devices from the disk list.
    pub only_physical: bool,
    /// Prefer filesystems listed in /etc/fstab.
    pub use_fstab: bool,
    /// Comma-separated mount filters; entries prefixed by `exclude=` are omitted.
    pub disks_filter: String,
    /// Show disk busy percentage in the disks pane.
    pub io_mode: bool,
    /// Combine read and write series into one graph.
    pub io_graph_combined: bool,
    /// Per-mount IO graph ceilings in MiB/s (`mount:speed` pairs).
    pub io_graph_speeds: String,
    /// Automatically scale network graphs to recent traffic.
    pub net_auto: bool,
    /// Use the larger network direction scale for both graphs.
    pub net_sync: bool,
    /// Download graph ceiling in KiB/s when automatic scaling is disabled.
    pub net_download: u64,
    /// Upload graph ceiling in KiB/s when automatic scaling is disabled.
    pub net_upload: u64,
    /// Preferred network interface name, if available.
    pub net_iface: Option<String>,
    /// Swap the displayed download and upload directions.
    pub swap_upload_download: bool,
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        Self {
            update_ms: DEFAULT_RESOURCES_UPDATE_MS,
            shown_boxes: default_shown_boxes(),
            presets: default_presets(),
            preset: 0,
            cpu_bottom: None,
            mem_below_net: None,
            agents_left: None,
            cpu_graph_upper: "total".into(),
            cpu_graph_lower: "iowait".into(),
            cpu_invert_lower: false,
            cpu_single_graph: false,
            temp_scale: "celsius".into(),
            freq_mode: "auto".into(),
            show_battery: true,
            mem_graphs: true,
            show_disks: true,
            show_swap: true,
            swap_disk: true,
            only_physical: true,
            use_fstab: true,
            disks_filter: String::new(),
            io_mode: false,
            io_graph_combined: false,
            io_graph_speeds: String::new(),
            net_auto: true,
            net_sync: false,
            net_download: 100,
            net_upload: 100,
            net_iface: None,
            swap_upload_download: false,
        }
    }
}

impl ResourcesConfig {
    /// Return the supported interval, clamping programmatic values as well.
    pub fn update_interval_ms(&self) -> u64 {
        self.update_ms
            .clamp(MIN_RESOURCES_UPDATE_MS, MAX_RESOURCES_UPDATE_MS)
    }
}
