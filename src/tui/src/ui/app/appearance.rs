//! Appearance-setting mutations and persistence, including the Agents-sidebar
//! grouping and ordering controls.
//!
//! Colour roles, local-process indicators, whole-device indicators, and the
//! sidebar's grouping and sort preferences live here. The two
//! harness-row toggles this page used to carry — branch and shortened path —
//! became placements on the Status line page, which is where the rest of the row
//! is configured and the only place the effect can be previewed. Leaving them
//! here as well would have meant two controls writing two sections for one
//! choice, with the newer one silently winning.
//!
//! The old `[appearance]` keys are still honoured for a config that predates
//! that move: see
//! [`StatusLineConfig::from_appearance`](medulla::config::StatusLineConfig::from_appearance).

use crate::ui::theme::{blink_ms_from_seconds, blink_seconds, color_to_string, THEME_ROLES};

use super::types::App;

/// Non-theme rows: session titles, the process and device resource indicators,
/// and the two Agents-sidebar layout controls.
pub(super) const APPEARANCE_OPTION_ROWS: usize = 9;

/// The option offset of the sidebar grouping row, past the resource indicators.
pub(super) const SIDEBAR_GROUPING_OPTION: usize = 7;

/// The option offset of the sidebar sort row.
pub(super) const SIDEBAR_SORT_OPTION: usize = 8;

/// Number of behavior controls shown after the editable theme colors: the blink
/// toggle and the rate it blinks at.
pub(super) const ATTENTION_ROWS: usize = 2;

/// The pulse lengths the Appearance editor offers, in seconds.
///
/// A short list rather than a free-text field, because the useful range is
/// narrow and every value in it is a judgement about how insistent a stuck
/// harness should be. Anything else remains configurable by hand — the config
/// key takes any number and clamps it — and a hand-set value that is not on this
/// list steps into it from the nearest end rather than being lost.
const BLINK_SECONDS: [f64; 6] = [0.3, 0.5, 1.0, 1.5, 2.0, 3.0];

/// Number of selectable rows: theme colors, attention behavior, and options.
pub(super) const APPEARANCE_ROWS: usize =
    THEME_ROLES.len() + ATTENTION_ROWS + APPEARANCE_OPTION_ROWS;

/// The next value after `current` in `choices`, wrapping in either direction.
///
/// A value that is not in `choices` — a config hand-edited to something the
/// build does not know — starts the cycle from the first entry rather than
/// refusing to move, so the row is never stuck.
fn cycled<T: Copy + PartialEq>(choices: &[T], current: T, forward: bool) -> T {
    let at = choices.iter().position(|choice| *choice == current);
    match at {
        Some(at) if forward => choices[(at + 1) % choices.len()],
        Some(at) => choices[(at + choices.len() - 1) % choices.len()],
        None => choices[0],
    }
}

impl App {
    /// Cycle the selected colour role and persist the theme.
    pub(super) fn cycle_appearance_row(&mut self, forward: bool) {
        let index = self.appearance_index.min(APPEARANCE_ROWS - 1);
        if index < THEME_ROLES.len() {
            self.theme.cycle_role(index, forward);
            self.persist_theme_now(THEME_ROLES[index]);
        } else if index == THEME_ROLES.len() {
            self.theme.attention_blink = !self.theme.attention_blink;
            let value = if self.theme.attention_blink {
                "on"
            } else {
                "off"
            };
            self.persist_theme_value_now("Attention blink", value.into());
        } else if index == THEME_ROLES.len() + 1 {
            self.cycle_blink_rate(forward);
        } else {
            let option = index - THEME_ROLES.len() - ATTENTION_ROWS;
            match option {
                3 => self.toggle_session_titles(),
                SIDEBAR_GROUPING_OPTION => self.cycle_sidebar_grouping(forward),
                SIDEBAR_SORT_OPTION => self.cycle_sidebar_sort(forward),
                _ => {
                    let resource = if option < 3 { option } else { option - 1 };
                    self.cycle_resource_display(resource, forward);
                }
            }
        }
    }

    /// Step the attention pulse to the next offered length and persist it.
    ///
    /// A hand-configured value that is not one of the offered steps enters the
    /// list from whichever end the operator moved towards, so a first press
    /// changes the rate by a predictable amount instead of jumping to whatever
    /// happens to be nearest.
    fn cycle_blink_rate(&mut self, forward: bool) {
        let current = blink_seconds(self.theme.attention_blink_ms);
        let position = BLINK_SECONDS
            .iter()
            .position(|choice| (choice - current).abs() < f64::EPSILON);
        let next = match position {
            Some(index) if forward => BLINK_SECONDS[(index + 1) % BLINK_SECONDS.len()],
            Some(index) => BLINK_SECONDS[(index + BLINK_SECONDS.len() - 1) % BLINK_SECONDS.len()],
            None if forward => BLINK_SECONDS[0],
            None => BLINK_SECONDS[BLINK_SECONDS.len() - 1],
        };
        self.theme.attention_blink_ms = blink_ms_from_seconds(next);
        self.persist_theme_value_now("Attention blink rate", format!("{next:.1}s"));
    }

    /// Toggle extracted harness titles on orchestrator-managed agent rows.
    fn toggle_session_titles(&mut self) {
        let appearance = &mut self.loaded.config.appearance;
        appearance.show_session_titles = !appearance.show_session_titles;
        let shown = appearance.show_session_titles;
        self.persist_appearance_now("Session titles", if shown { "on" } else { "off" }.into());
    }

    /// Cycle and persist what the Agents sidebar sections its agents by.
    ///
    /// Takes effect on the next frame without any rebuild: the rail is assembled
    /// from the loaded config every time it is drawn, so the operator sees the
    /// arrangement they picked while the settings row is still under the cursor.
    fn cycle_sidebar_grouping(&mut self, forward: bool) {
        let current = self.loaded.config.appearance.sidebar_grouping;
        let next = cycled(&medulla::config::SidebarGrouping::ALL, current, forward);
        self.loaded.config.appearance.sidebar_grouping = next;
        self.persist_appearance_now("Sidebar grouping", next.label().into());
    }

    /// Cycle and persist how the Agents sidebar orders agents and sessions.
    fn cycle_sidebar_sort(&mut self, forward: bool) {
        let current = self.loaded.config.appearance.sidebar_sort;
        let next = cycled(&medulla::config::SidebarSort::ALL, current, forward);
        self.loaded.config.appearance.sidebar_sort = next;
        self.persist_appearance_now("Sidebar sort", next.label().into());
    }

    /// Cycle and persist one resource indicator, process-scoped or device-wide.
    ///
    /// `index` is the row's offset past the theme roles, in the order the
    /// renderer lists them. Each indicator offers only the formats that mean
    /// something for it: throughput has no percentage of a known total, and a
    /// device's CPU has no byte value.
    fn cycle_resource_display(&mut self, index: usize, forward: bool) {
        use medulla::config::ResourceDisplay::{Bar, Off, Percent, Value};

        let (name, display, choices): (&str, &mut _, &[_]) = match index {
            0 => (
                "Process CPU",
                &mut self.loaded.config.appearance.cpu,
                &[Off, Percent, Bar],
            ),
            1 => (
                "Process RAM",
                &mut self.loaded.config.appearance.ram,
                &[Off, Percent, Value, Bar],
            ),
            2 => (
                "Process disk I/O",
                &mut self.loaded.config.appearance.disk_io,
                &[Off, Value, Bar],
            ),
            3 => (
                "Device CPU",
                &mut self.loaded.config.appearance.device_cpu,
                &[Off, Percent, Bar],
            ),
            4 => (
                "Device RAM",
                &mut self.loaded.config.appearance.device_ram,
                &[Off, Percent, Value, Bar],
            ),
            _ => (
                "Device disk",
                &mut self.loaded.config.appearance.device_disk,
                &[Off, Percent, Value, Bar],
            ),
        };
        let current = choices
            .iter()
            .position(|choice| choice == display)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % choices.len()
        } else {
            (current + choices.len() - 1) % choices.len()
        };
        *display = choices[next];
        let value = format!("{:?}", *display).to_ascii_lowercase();

        self.persist_appearance_now(name, value);
    }

    /// Persist the complete appearance section after one live option changes.
    fn persist_appearance_now(&mut self, name: &str, value: String) {
        match &self.config_path {
            Some(path) => {
                // The whole section is rewritten on every change, so every
                // indicator key is written even when only one row moved — a
                // partial `[appearance]` would silently reset the rest on the
                // next load.
                let appearance = &self.loaded.config.appearance;
                let mut section: toml::Table = [
                    ("cpu", appearance.cpu),
                    ("ram", appearance.ram),
                    ("diskIo", appearance.disk_io),
                    ("deviceCpu", appearance.device_cpu),
                    ("deviceRam", appearance.device_ram),
                    ("deviceDisk", appearance.device_disk),
                ]
                .into_iter()
                .map(|(key, display)| {
                    (
                        key.to_string(),
                        toml::Value::String(format!("{display:?}").to_ascii_lowercase()),
                    )
                })
                .collect();
                section.insert(
                    "showSessionTitles".into(),
                    toml::Value::Boolean(appearance.show_session_titles),
                );
                section.insert(
                    "showHarnessBranch".into(),
                    toml::Value::Boolean(self.loaded.config.appearance.show_harness_branch),
                );
                section.insert(
                    "showHarnessPath".into(),
                    toml::Value::Boolean(self.loaded.config.appearance.show_harness_path),
                );
                section.insert(
                    "sidebarGrouping".into(),
                    toml::Value::String(
                        format!("{:?}", appearance.sidebar_grouping).to_lowercase(),
                    ),
                );
                section.insert(
                    "sidebarSort".into(),
                    toml::Value::String(format!("{:?}", appearance.sidebar_sort).to_lowercase()),
                );
                match medulla::config::persist_section(path, "appearance", section) {
                    Ok(()) => self.set_status(format!("Appearance · {name} → {value} (saved)")),
                    Err(error) => self.set_status(format!("Appearance save failed: {error}")),
                }
            }
            None => self.set_status(format!("Appearance · {name} → {value} (not persisted)")),
        }
    }

    /// Write the current theme to the injected config path.
    fn persist_theme_now(&mut self, role: &str) {
        let value = color_to_string(self.theme.role(self.appearance_index));
        self.persist_theme_value_now(role, value);
    }

    /// Persist the live theme and report the edited value in the status line.
    fn persist_theme_value_now(&mut self, setting: &str, value: String) {
        match &self.config_path {
            Some(path) => match crate::ui::theme::persist_theme(path, &self.theme) {
                Ok(()) => self.set_status(format!("Appearance · {setting} → {value} (saved)")),
                Err(error) => self.set_status(format!("Appearance · save failed: {error}")),
            },
            None => self.set_status(format!("Appearance · {setting} → {value} (not persisted)")),
        }
    }
}
