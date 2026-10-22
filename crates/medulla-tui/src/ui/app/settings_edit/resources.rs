//! Btop-style editor for the Resources configuration.

use medulla::config::{ResourcesConfig, MAX_RESOURCES_UPDATE_MS, MIN_RESOURCES_UPDATE_MS};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{layout::Rect, Frame};

use crate::ui::app::types::App;

#[derive(Clone, Copy)]
enum Kind {
    Flag,
    Count(u64, u64, u64),
    Choice(&'static [&'static str]),
    OptionalFlag,
    OptionalText,
}

#[derive(Clone, Copy)]
struct Row {
    group: &'static str,
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
}

const ROWS: &[Row] = &[
    row(
        "GENERAL",
        "updateMs",
        "Sample interval",
        "Milliseconds between resource samples.",
        Kind::Count(MIN_RESOURCES_UPDATE_MS, MAX_RESOURCES_UPDATE_MS, 100),
    ),
    row(
        "GENERAL",
        "shownBoxes",
        "Visible boxes",
        "Boxes shown when Resources opens.",
        Kind::Choice(&[
            "cpu,mem,net,agents",
            "cpu,mem,net",
            "cpu,mem",
            "cpu,net",
            "mem,net",
            "cpu",
            "mem",
            "net",
            "agents",
        ]),
    ),
    row(
        "GENERAL",
        "presets",
        "Layout presets",
        "Whitespace-separated btop-style layout presets.",
        Kind::Choice(&[
            "cpu:0:default,mem:0:default,net:0:default,agents:1:default",
            "cpu:1:block,mem:1:block,net:0:block,agents:0:tty",
        ]),
    ),
    row(
        "GENERAL",
        "preset",
        "Selected preset",
        "Preset selected when Resources opens.",
        Kind::Count(0, 9, 1),
    ),
    row(
        "GENERAL",
        "cpuBottom",
        "CPU below",
        "Place CPU below the other boxes; Auto follows the preset.",
        Kind::OptionalFlag,
    ),
    row(
        "GENERAL",
        "memBelowNet",
        "Memory below network",
        "Place memory below network; Auto follows the preset.",
        Kind::OptionalFlag,
    ),
    row(
        "CPU",
        "cpuGraphUpper",
        "Upper graph",
        "CPU field drawn by the upper graph.",
        Kind::Choice(&[
            "total", "user", "system", "nice", "iowait", "irq", "softirq", "steal",
        ]),
    ),
    row(
        "CPU",
        "cpuGraphLower",
        "Lower graph",
        "CPU field drawn by the lower graph.",
        Kind::Choice(&[
            "iowait", "total", "user", "system", "nice", "irq", "softirq", "steal",
        ]),
    ),
    row(
        "CPU",
        "cpuInvertLower",
        "Invert lower graph",
        "Draw the lower CPU graph upside down.",
        Kind::Flag,
    ),
    row(
        "CPU",
        "cpuSingleGraph",
        "Single graph",
        "Collapse CPU fields into one graph.",
        Kind::Flag,
    ),
    row(
        "CPU",
        "tempScale",
        "Temperature scale",
        "Temperature display units.",
        Kind::Choice(&["celsius", "fahrenheit"]),
    ),
    row(
        "CPU",
        "freqMode",
        "Frequency units",
        "CPU frequency presentation.",
        Kind::Choice(&["auto", "mhz", "ghz"]),
    ),
    row(
        "CPU",
        "showBattery",
        "Show battery",
        "Show battery percentage when available.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "memGraphs",
        "Memory graphs",
        "Draw memory and swap history graphs.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "showDisks",
        "Show disks",
        "Show mounted disks in the memory panel.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "showSwap",
        "Show swap",
        "Show swap usage in the memory panel.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "swapDisk",
        "Swap device as disk",
        "Show a swap device as a disk row when available.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "onlyPhysical",
        "Physical disks only",
        "Hide virtual and loop devices.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "useFstab",
        "Use fstab",
        "Prefer filesystems listed in /etc/fstab.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "disksFilter",
        "Disk filters",
        "Comma-separated mount filters.",
        Kind::Choice(&["", "exclude=/boot", "exclude=/boot,exclude=/tmp"]),
    ),
    row(
        "MEM",
        "ioMode",
        "Disk busy percentage",
        "Show disk busy percentage in the disks pane.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "ioGraphCombined",
        "Combined IO graph",
        "Combine read and write series.",
        Kind::Flag,
    ),
    row(
        "MEM",
        "ioGraphSpeeds",
        "IO graph ceilings",
        "Per-mount IO graph ceilings in MiB/s.",
        Kind::Choice(&["", "/:20", "/:100"]),
    ),
    row(
        "NET",
        "netAuto",
        "Automatic network scale",
        "Scale network graphs to recent traffic.",
        Kind::Flag,
    ),
    row(
        "NET",
        "netSync",
        "Synchronized scales",
        "Use the larger direction scale for both graphs.",
        Kind::Flag,
    ),
    row(
        "NET",
        "netDownload",
        "Download ceiling",
        "Download graph ceiling in KiB/s.",
        Kind::Count(1, 1_000_000, 100),
    ),
    row(
        "NET",
        "netUpload",
        "Upload ceiling",
        "Upload graph ceiling in KiB/s.",
        Kind::Count(1, 1_000_000, 100),
    ),
    row(
        "NET",
        "netIface",
        "Network interface",
        "Preferred interface; Auto selects the active default.",
        Kind::OptionalText,
    ),
    row(
        "NET",
        "swapUploadDownload",
        "Swap directions",
        "Swap displayed download and upload directions.",
        Kind::Flag,
    ),
    row(
        "AGENTS",
        "agentsLeft",
        "Agents on left",
        "Place Agents on the left; Auto follows the preset.",
        Kind::OptionalFlag,
    ),
];

const fn row(
    group: &'static str,
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
) -> Row {
    Row {
        group,
        key,
        label,
        help,
        kind,
    }
}

impl App {
    pub(in crate::ui::app) fn draw_resources_settings(&mut self, f: &mut Frame, area: Rect) {
        let selected = self.resources_settings_index.min(ROWS.len() - 1);
        let cfg = &self.loaded.config.resources;
        let mut lines = Vec::new();
        let mut group = "";
        let mut selected_line = 0;
        for (index, row) in ROWS.iter().enumerate() {
            if row.group != group {
                group = row.group;
                lines.push(Line::from(Span::styled(
                    group,
                    Style::default().add_modifier(Modifier::BOLD),
                )));
            }
            let value = display(cfg, *row);
            if index == selected {
                selected_line = lines.len();
            }
            let style = if index == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let control = match row.kind {
                Kind::Flag | Kind::OptionalFlag => "toggle".to_owned(),
                Kind::Count(min, max, step) => format!("{min}–{max}, step {step}"),
                Kind::Choice(values) => format!("{} choices", values.len()),
                Kind::OptionalText => "Auto or interface".to_owned(),
            };
            lines.push(Line::from(vec![
                Span::styled(if index == selected { "▸ " } else { "  " }, style),
                Span::styled(format!("{:<25}", row.label), style),
                Span::styled(format!("‹ {value} ›   {} · {control}", row.key), style),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(ROWS[selected].help));
        lines.push(Line::from(
            "↑↓ select · ←→ change · Enter toggle · writes [resources]",
        ));
        let visible = usize::from(area.height.saturating_sub(4)).max(1);
        let scroll = selected_line.saturating_sub(visible.saturating_sub(1));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((scroll.min(u16::MAX as usize) as u16, 0))
                .block(self.content_panel("Resources")),
            area,
        );
    }

    pub(in crate::ui::app) fn move_resources_settings(&mut self, up: bool) {
        self.resources_settings_index = if up {
            self.resources_settings_index.saturating_sub(1)
        } else {
            (self.resources_settings_index + 1).min(ROWS.len() - 1)
        };
    }

    /// Toggle the selected resource setting only when it is a flag row.
    pub(in crate::ui::app) fn toggle_resources_setting(&mut self) -> String {
        let row = ROWS[self.resources_settings_index.min(ROWS.len() - 1)];
        if matches!(row.kind, Kind::Flag | Kind::OptionalFlag) {
            self.adjust_resources_setting(0)
        } else {
            format!("Resources · {} needs ← or → to change", row.label)
        }
    }

    pub(in crate::ui::app) fn adjust_resources_setting(&mut self, delta: i32) -> String {
        let row = ROWS[self.resources_settings_index.min(ROWS.len() - 1)];
        let cfg = &mut self.loaded.config.resources;
        match row.key {
            "updateMs" => {
                cfg.update_ms = step(
                    cfg.update_ms,
                    delta,
                    MIN_RESOURCES_UPDATE_MS,
                    MAX_RESOURCES_UPDATE_MS,
                    100,
                )
            }
            "shownBoxes" => {
                cfg.shown_boxes = cycle(&cfg.shown_boxes, choices(row), delta).to_owned()
            }
            "presets" => cfg.presets = cycle(&cfg.presets, choices(row), delta).to_owned(),
            "preset" => {
                let max = cfg
                    .presets
                    .split_whitespace()
                    .count()
                    .saturating_sub(1)
                    .min(9) as u64;
                cfg.preset = step(cfg.preset as u64, delta, 0, max, 1) as usize;
            }
            "cpuBottom" => cfg.cpu_bottom = optional_flag(cfg.cpu_bottom, delta),
            "memBelowNet" => cfg.mem_below_net = optional_flag(cfg.mem_below_net, delta),
            "agentsLeft" => cfg.agents_left = optional_flag(cfg.agents_left, delta),
            "cpuGraphUpper" => {
                cfg.cpu_graph_upper = cycle(&cfg.cpu_graph_upper, choices(row), delta).into()
            }
            "cpuGraphLower" => {
                cfg.cpu_graph_lower = cycle(&cfg.cpu_graph_lower, choices(row), delta).into()
            }
            "cpuInvertLower" => cfg.cpu_invert_lower = !cfg.cpu_invert_lower,
            "cpuSingleGraph" => cfg.cpu_single_graph = !cfg.cpu_single_graph,
            "tempScale" => cfg.temp_scale = cycle(&cfg.temp_scale, choices(row), delta).into(),
            "freqMode" => cfg.freq_mode = cycle(&cfg.freq_mode, choices(row), delta).into(),
            "showBattery" => cfg.show_battery = !cfg.show_battery,
            "memGraphs" => cfg.mem_graphs = !cfg.mem_graphs,
            "showDisks" => cfg.show_disks = !cfg.show_disks,
            "showSwap" => cfg.show_swap = !cfg.show_swap,
            "swapDisk" => cfg.swap_disk = !cfg.swap_disk,
            "onlyPhysical" => cfg.only_physical = !cfg.only_physical,
            "useFstab" => cfg.use_fstab = !cfg.use_fstab,
            "disksFilter" => {
                cfg.disks_filter = cycle(&cfg.disks_filter, choices(row), delta).into()
            }
            "ioMode" => cfg.io_mode = !cfg.io_mode,
            "ioGraphCombined" => cfg.io_graph_combined = !cfg.io_graph_combined,
            "ioGraphSpeeds" => {
                cfg.io_graph_speeds = cycle(&cfg.io_graph_speeds, choices(row), delta).into()
            }
            "netAuto" => cfg.net_auto = !cfg.net_auto,
            "netSync" => cfg.net_sync = !cfg.net_sync,
            "netDownload" => cfg.net_download = step(cfg.net_download, delta, 1, 1_000_000, 100),
            "netUpload" => cfg.net_upload = step(cfg.net_upload, delta, 1, 1_000_000, 100),
            "netIface" => {
                let interfaces: Vec<_> = self
                    .resource_snapshot
                    .networks
                    .iter()
                    .map(|network| network.interface.as_str())
                    .collect();
                let current = cfg
                    .net_iface
                    .as_deref()
                    .and_then(|name| interfaces.iter().position(|item| *item == name))
                    .map(|index| index + 1)
                    .unwrap_or(0);
                let choices = interfaces.len() + 1;
                let next = if delta < 0 {
                    (current + choices - 1) % choices
                } else {
                    (current + 1) % choices
                };
                cfg.net_iface = next
                    .checked_sub(1)
                    .and_then(|index| interfaces.get(index))
                    .map(|name| (*name).to_owned());
            }
            "swapUploadDownload" => cfg.swap_upload_download = !cfg.swap_upload_download,
            _ => unreachable!("all resource settings rows are mapped"),
        }
        let clear_auto = match row.key {
            "cpuBottom" => cfg.cpu_bottom.is_none(),
            "memBelowNet" => cfg.mem_below_net.is_none(),
            "agentsLeft" => cfg.agents_left.is_none(),
            _ => false,
        };
        let value = persist_value(cfg, row);
        if let Some(path) = self.config_path.as_deref() {
            let persisted = if clear_auto {
                medulla::config::clear_setting(path, "resources", row.key)
            } else {
                medulla::config::persist_setting(path, "resources", row.key, value)
            };
            if let Err(error) = persisted {
                return format!("Resources · changed in memory; could not save: {error}");
            }
        }
        let updated = super::super::resources::ResourcesState::from_config(cfg);
        self.resources.shown = updated.shown;
        self.resources.opts = updated.opts;
        self.resources.interval_ms = updated.interval_ms;
        self.resources.preset = updated.preset;
        self.resources.presets = updated.presets;
        self.resources.show_disks = updated.show_disks;
        self.resources.io_mode = updated.io_mode;
        self.resources.net_auto = updated.net_auto;
        self.resources.net_sync = updated.net_sync;
        self.resources.net_download = updated.net_download;
        self.resources.net_upload = updated.net_upload;
        self.resources.net_iface = updated.net_iface;
        self.resources.swap_upload_download = updated.swap_upload_download;
        format!("Resources · {} → {}", row.label, display(cfg, row))
    }
}

fn step(current: u64, delta: i32, min: u64, max: u64, amount: u64) -> u64 {
    if delta == 0 {
        return if current < max { current + amount } else { min };
    }
    (current as i64 + i64::from(delta) * amount as i64).clamp(min as i64, max as i64) as u64
}
fn optional_flag(value: Option<bool>, delta: i32) -> Option<bool> {
    match (value, delta < 0) {
        (None, false) => Some(true),
        (Some(true), false) => Some(false),
        (Some(false), false) => None,
        (None, true) => Some(false),
        (Some(false), true) => Some(true),
        (Some(true), true) => None,
    }
}
fn choices(row: Row) -> &'static [&'static str] {
    if let Kind::Choice(values) = row.kind {
        values
    } else {
        &[]
    }
}
fn cycle<'a>(current: &str, values: &'a [&'static str], delta: i32) -> &'a str {
    let index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    let next = if delta < 0 {
        (index + values.len() - 1) % values.len()
    } else {
        (index + 1) % values.len()
    };
    values[next]
}
fn display(cfg: &ResourcesConfig, row: Row) -> String {
    match row.key {
        "updateMs" => cfg.update_ms.to_string(),
        "shownBoxes" => cfg.shown_boxes.clone(),
        "presets" => cfg.presets.clone(),
        "preset" => cfg.preset.to_string(),
        "cpuBottom" => opt_flag(cfg.cpu_bottom),
        "memBelowNet" => opt_flag(cfg.mem_below_net),
        "agentsLeft" => opt_flag(cfg.agents_left),
        "cpuGraphUpper" => cfg.cpu_graph_upper.clone(),
        "cpuGraphLower" => cfg.cpu_graph_lower.clone(),
        "cpuInvertLower" => flag(cfg.cpu_invert_lower),
        "cpuSingleGraph" => flag(cfg.cpu_single_graph),
        "tempScale" => cfg.temp_scale.clone(),
        "freqMode" => cfg.freq_mode.clone(),
        "showBattery" => flag(cfg.show_battery),
        "memGraphs" => flag(cfg.mem_graphs),
        "showDisks" => flag(cfg.show_disks),
        "showSwap" => flag(cfg.show_swap),
        "swapDisk" => flag(cfg.swap_disk),
        "onlyPhysical" => flag(cfg.only_physical),
        "useFstab" => flag(cfg.use_fstab),
        "disksFilter" => nonempty(&cfg.disks_filter),
        "ioMode" => flag(cfg.io_mode),
        "ioGraphCombined" => flag(cfg.io_graph_combined),
        "ioGraphSpeeds" => nonempty(&cfg.io_graph_speeds),
        "netAuto" => flag(cfg.net_auto),
        "netSync" => flag(cfg.net_sync),
        "netDownload" => cfg.net_download.to_string(),
        "netUpload" => cfg.net_upload.to_string(),
        "netIface" => cfg
            .net_iface
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Auto".into()),
        "swapUploadDownload" => flag(cfg.swap_upload_download),
        _ => unreachable!(),
    }
}
fn flag(value: bool) -> String {
    if value { "On" } else { "Off" }.into()
}
fn opt_flag(value: Option<bool>) -> String {
    value.map(flag).unwrap_or_else(|| "Auto".into())
}
fn nonempty(value: &str) -> String {
    if value.is_empty() {
        "None".into()
    } else {
        value.into()
    }
}
fn persist_value(cfg: &ResourcesConfig, row: Row) -> toml::Value {
    use toml::Value as V;
    match row.key {
        "updateMs" => V::Integer(cfg.update_ms as i64),
        "shownBoxes" => V::String(cfg.shown_boxes.clone()),
        "presets" => V::String(cfg.presets.clone()),
        "preset" => V::Integer(cfg.preset as i64),
        "cpuBottom" => V::Boolean(cfg.cpu_bottom.unwrap_or(false)),
        "memBelowNet" => V::Boolean(cfg.mem_below_net.unwrap_or(false)),
        "agentsLeft" => V::Boolean(cfg.agents_left.unwrap_or(false)),
        "cpuGraphUpper" => V::String(cfg.cpu_graph_upper.clone()),
        "cpuGraphLower" => V::String(cfg.cpu_graph_lower.clone()),
        "cpuInvertLower" => V::Boolean(cfg.cpu_invert_lower),
        "cpuSingleGraph" => V::Boolean(cfg.cpu_single_graph),
        "tempScale" => V::String(cfg.temp_scale.clone()),
        "freqMode" => V::String(cfg.freq_mode.clone()),
        "showBattery" => V::Boolean(cfg.show_battery),
        "memGraphs" => V::Boolean(cfg.mem_graphs),
        "showDisks" => V::Boolean(cfg.show_disks),
        "showSwap" => V::Boolean(cfg.show_swap),
        "swapDisk" => V::Boolean(cfg.swap_disk),
        "onlyPhysical" => V::Boolean(cfg.only_physical),
        "useFstab" => V::Boolean(cfg.use_fstab),
        "disksFilter" => V::String(cfg.disks_filter.clone()),
        "ioMode" => V::Boolean(cfg.io_mode),
        "ioGraphCombined" => V::Boolean(cfg.io_graph_combined),
        "ioGraphSpeeds" => V::String(cfg.io_graph_speeds.clone()),
        "netAuto" => V::Boolean(cfg.net_auto),
        "netSync" => V::Boolean(cfg.net_sync),
        "netDownload" => V::Integer(cfg.net_download as i64),
        "netUpload" => V::Integer(cfg.net_upload as i64),
        "netIface" => V::String(cfg.net_iface.clone().unwrap_or_default()),
        "swapUploadDownload" => V::Boolean(cfg.swap_upload_download),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_resources_config_field_has_an_editor_row() {
        let config = serde_json::to_value(ResourcesConfig::default()).unwrap();
        let fields = config.as_object().unwrap();
        for field in fields.keys() {
            let camel = field.clone();
            assert!(
                ROWS.iter().any(|row| row.key == camel),
                "no Resources setting for {field}"
            );
        }
        assert_eq!(ROWS.len(), fields.len());
    }

    #[test]
    fn optional_flags_cycle_between_auto_on_and_off_in_both_directions() {
        let auto = None;
        let on = Some(true);
        let off = Some(false);

        assert_eq!(optional_flag(auto, 1), on);
        assert_eq!(optional_flag(on, 1), off);
        assert_eq!(optional_flag(off, 1), auto);
        assert_eq!(optional_flag(auto, -1), off);
        assert_eq!(optional_flag(off, -1), on);
        assert_eq!(optional_flag(on, -1), auto);
    }
}
