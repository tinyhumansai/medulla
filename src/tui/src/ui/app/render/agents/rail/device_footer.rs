//! Layout, sampling, and styling for the Agents rail's device-health footer.

use medulla::config::ResourceDisplay;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::types::App;

/// Navigation lines the ambient footer may never take from the rail.
const MIN_RAIL_LINES: usize = 3;

/// Device readings the footer can show at most: CPU, RAM, and disk.
const DEVICE_METRICS: usize = 3;

/// Prepared footer content and the navigation capacity left above it.
pub(super) struct DeviceFooter {
    lines: Vec<String>,
    /// Number of rail lines available for selectable navigation rows.
    pub(super) navigation_capacity: usize,
    rail_height: usize,
}

impl DeviceFooter {
    /// Sample enabled metrics and budget the footer without displacing the
    /// minimum usable navigation area.
    pub(super) fn prepare(app: &mut App, width: usize, rail_height: usize) -> Self {
        let budget = rail_height
            .saturating_sub(MIN_RAIL_LINES + 1)
            .min(DEVICE_METRICS);
        let appearance = &app.loaded.config.appearance;
        let enabled = appearance.device_cpu != ResourceDisplay::Off
            || appearance.device_ram != ResourceDisplay::Off
            || appearance.device_disk != ResourceDisplay::Off;
        let snapshot = if enabled {
            app.device_monitor.sample()
        } else {
            Default::default()
        };
        let lines = crate::ui::resources::device_lines(appearance, snapshot, width, budget);
        let footer_height = if lines.is_empty() { 0 } else { lines.len() + 1 };
        let navigation_capacity = rail_height.saturating_sub(footer_height).max(1);
        Self {
            lines,
            navigation_capacity,
            rail_height,
        }
    }

    /// Pin the prepared readings to the rail bottom and apply footer styling.
    pub(super) fn append_to(self, view: &mut Vec<TLine<'static>>, style: Style) {
        if self.lines.is_empty() {
            return;
        }
        let footer_height = self.lines.len() + 1;
        while view.len() + footer_height < self.rail_height {
            view.push(TLine::from(""));
        }
        view.push(TLine::from(""));
        let style = style.add_modifier(Modifier::DIM);
        view.extend(
            self.lines
                .into_iter()
                .map(|line| TLine::from(Span::styled(line, style))),
        );
    }
}
