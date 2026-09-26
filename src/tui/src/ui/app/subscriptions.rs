//! Subscription-meter settings: which usage meters the sidebar shows, how each
//! one is drawn, and how often they are re-sampled.
//!
//! The page is deliberately its own rather than another group on Appearance.
//! Appearance decides how Medulla *looks*; these rows decide which providers
//! Medulla *asks about* — two of them over the network, on the operator's own
//! subscriptions. A row here has a consequence beyond the screen, and it should
//! not be reachable by scrolling past a colour picker.

use medulla::config::{MeterId, ResourceDisplay};

use super::types::App;

/// The refresh intervals the page offers, in seconds.
///
/// A short list, because the useful range is narrow: below a minute the
/// providers' own rate limits become the constraint, and past an hour the
/// reading stops tracking the window it claims to. A hand-set value outside the
/// list steps into it from the nearest end rather than being lost.
const REFRESH_CHOICES: [u64; 5] = [60, 300, 900, 1_800, 3_600];

/// Displays offered for a meter that measures a share of a window.
///
/// No `Value`: a window has no amount to state — the provider reports a
/// percentage and nothing else.
const WINDOW_DISPLAYS: [ResourceDisplay; 3] = [
    ResourceDisplay::Off,
    ResourceDisplay::Percent,
    ResourceDisplay::Bar,
];

/// Displays offered for a meter that measures money left.
const BALANCE_DISPLAYS: [ResourceDisplay; 4] = [
    ResourceDisplay::Off,
    ResourceDisplay::Value,
    ResourceDisplay::Percent,
    ResourceDisplay::Bar,
];

/// The row index of the refresh-interval control, past the meters.
pub(super) const REFRESH_ROW: usize = MeterId::ALL.len();

/// Selectable rows: one per meter, plus the refresh interval.
pub(super) const SUBSCRIPTION_ROWS: usize = MeterId::ALL.len() + 1;

impl App {
    /// Move the Subscriptions cursor one row.
    pub(super) fn move_subscriptions_index(&mut self, up: bool) {
        self.subscriptions_index = if up {
            self.subscriptions_index.saturating_sub(1)
        } else {
            (self.subscriptions_index + 1).min(SUBSCRIPTION_ROWS - 1)
        };
    }

    /// Cycle the selected row and persist the section.
    ///
    /// Switching a meter on samples immediately rather than waiting for the
    /// refresh cadence: the operator just asked for a number, and an empty row
    /// for the next five minutes reads as a broken setting.
    pub(super) fn cycle_subscription_row(&mut self, forward: bool) -> Option<super::types::Cmd> {
        let row = self.subscriptions_index.min(SUBSCRIPTION_ROWS - 1);
        if row == REFRESH_ROW {
            self.cycle_refresh_interval(forward);
            return None;
        }
        let meter = MeterId::ALL[row];
        let choices: &[ResourceDisplay] = if meter.is_balance() {
            &BALANCE_DISPLAYS
        } else {
            &WINDOW_DISPLAYS
        };
        let display = self.loaded.config.subscriptions.display_mut(meter);
        let at = choices.iter().position(|choice| choice == display);
        *display = match at {
            Some(at) if forward => choices[(at + 1) % choices.len()],
            Some(at) => choices[(at + choices.len() - 1) % choices.len()],
            // A hand-configured display this meter does not offer — `value` on a
            // window, say — enters the cycle at the start rather than sticking.
            None => choices[0],
        };
        let value = format!("{:?}", *display).to_ascii_lowercase();
        let switched_on = *display != ResourceDisplay::Off;
        self.persist_subscriptions_now(meter.label(), value);
        switched_on.then(|| self.refresh_subscriptions()).flatten()
    }

    /// Step the automatic re-sample interval and persist it.
    fn cycle_refresh_interval(&mut self, forward: bool) {
        let current = self.loaded.config.subscriptions.refresh_seconds;
        let at = REFRESH_CHOICES.iter().position(|choice| *choice == current);
        let next = match at {
            Some(at) if forward => REFRESH_CHOICES[(at + 1) % REFRESH_CHOICES.len()],
            Some(at) => REFRESH_CHOICES[(at + REFRESH_CHOICES.len() - 1) % REFRESH_CHOICES.len()],
            None if forward => REFRESH_CHOICES[0],
            None => REFRESH_CHOICES[REFRESH_CHOICES.len() - 1],
        };
        self.loaded.config.subscriptions.refresh_seconds = next;
        self.persist_subscriptions_now("Refresh", refresh_label(next));
    }

    /// Write the complete `[subscriptions]` section after one row changes.
    fn persist_subscriptions_now(&mut self, name: &str, value: String) {
        let Some(path) = self.config_path.clone() else {
            self.set_status(format!("Subscriptions · {name} → {value} (not persisted)"));
            return;
        };
        // The whole section is rewritten every time, as the appearance writer
        // does: a partial `[subscriptions]` would silently reset the meters that
        // did not move on the next load.
        let subscriptions = &self.loaded.config.subscriptions;
        let mut section: toml::Table = MeterId::ALL
            .into_iter()
            .map(|meter| {
                (
                    meter.config_key().to_string(),
                    toml::Value::String(
                        format!("{:?}", subscriptions.display(meter)).to_ascii_lowercase(),
                    ),
                )
            })
            .collect();
        section.insert(
            "refreshSeconds".into(),
            toml::Value::Integer(subscriptions.refresh_seconds as i64),
        );
        // Only the *name* of the variable, never the key itself. Written back
        // so a hand-configured name survives the rewrite.
        if let Some(env) = &subscriptions.open_router_key_env {
            section.insert("openRouterKeyEnv".into(), toml::Value::String(env.clone()));
        }
        match medulla::config::persist_section(&path, "subscriptions", section) {
            Ok(()) => self.set_status(format!("Subscriptions · {name} → {value} (saved)")),
            Err(error) => self.set_status(format!("Subscriptions save failed: {error}")),
        }
    }
}

/// A refresh interval as the page spells it.
pub(super) fn refresh_label(seconds: u64) -> String {
    if seconds.is_multiple_of(3_600) {
        format!("{}h", seconds / 3_600)
    } else if seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}
