//! Layout, sampling, and styling for the Sessions rail's footer: the device
//! health readings, and the subscription-usage meters beneath them.
//!
//! Both blocks answer a version of the same question an operator asks before
//! sending more work out — can this machine take it, and can the accounts pay
//! for it — so they share one footer and one line budget. The device readings
//! come first: they move minute to minute, while a subscription window moves
//! over hours.

use medulla::config::ResourceDisplay;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::types::App;

use super::types::DeviceFooter;

/// Navigation lines the ambient footer may never take from the rail.
const MIN_RAIL_LINES: usize = 3;

/// Device readings the footer can show at most: CPU, RAM, and disk.
const DEVICE_METRICS: usize = 3;

/// Subscription meters the footer can show at most.
///
/// Capped below the number of meters that exist: a rail given over entirely to
/// balances is a rail you cannot navigate, and an operator who switches on more
/// than this many is better served by the Subscriptions settings page, which
/// shows every one of them with its reset time.
const SUBSCRIPTION_METRICS: usize = 4;

impl DeviceFooter {
    /// Sample enabled metrics and budget the footer without displacing the
    /// minimum usable navigation area.
    pub(super) fn prepare(app: &mut App, width: usize, rail_height: usize) -> Self {
        let budget = rail_height
            .saturating_sub(MIN_RAIL_LINES + 1)
            .min(DEVICE_METRICS + SUBSCRIPTION_METRICS);
        let device_budget = budget.min(DEVICE_METRICS);
        let appearance = &app.loaded.config.appearance;
        let enabled = appearance.device_cpu != ResourceDisplay::Off
            || appearance.device_ram != ResourceDisplay::Off
            || appearance.device_disk != ResourceDisplay::Off;
        let metrics_before_disk = [appearance.device_cpu, appearance.device_ram]
            .into_iter()
            .filter(|display| *display != ResourceDisplay::Off)
            .count();
        let disk_visible =
            appearance.device_disk != ResourceDisplay::Off && device_budget > metrics_before_disk;
        let snapshot = if enabled {
            app.device_monitor.sample_for(disk_visible)
        } else {
            Default::default()
        };
        let mut lines =
            crate::ui::resources::device_lines(appearance, snapshot, width, device_budget);
        // Whatever the device readings did not use is the meters' to spend, so
        // a sidebar with every device indicator off still shows four meters.
        let meter_budget = budget.saturating_sub(lines.len()).min(SUBSCRIPTION_METRICS);
        lines.extend(crate::ui::subscriptions::subscription_lines(
            &app.loaded.config.subscriptions,
            app.subscriptions(),
            width,
            meter_budget,
        ));
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
