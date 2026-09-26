//! The Appearance page's row table, its mutations, and its persistence.
//!
//! [`APPEARANCE_TABLE`] is the page: every row's heading, label, one-line
//! explanation, and the setting it edits, in the order the page lists them.
//! `appearance_index` is an index into it, so the renderer and the cycler
//! agree on what row 12 is by reading the same table rather than by each
//! doing the same arithmetic over group sizes — which is how the two came
//! apart before, leaving a row that described one setting and changed another.
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

use medulla::config::ResourceDisplay::{self, Bar, Off, Percent, Value};

use crate::ui::theme::{
    blink_ms_from_seconds, blink_seconds, color_to_string, PALETTE, THEME_ROLES,
};

use super::types::App;

/// The pulse lengths the Appearance editor offers, in seconds.
///
/// A short list rather than a free-text field, because the useful range is
/// narrow and every value in it is a judgement about how insistent a stuck
/// harness should be. Anything else remains configurable by hand — the config
/// key takes any number and clamps it — and a hand-set value that is not on this
/// list steps into it from the nearest end rather than being lost.
const BLINK_SECONDS: [f64; 6] = [0.3, 0.5, 1.0, 1.5, 2.0, 3.0];

/// The resource indicators, in page order, each with the formats it offers.
///
/// One table, read by both the cycler and the page: an indicator that offered
/// `value` in the footer but refused to cycle to it would be a lie the two
/// could only tell by being written twice.
pub(super) const RESOURCE_INDICATORS: [(&str, &[ResourceDisplay]); 6] = [
    ("Process CPU", &[Off, Percent, Bar]),
    ("Process RAM", &[Off, Percent, Value, Bar]),
    ("Process disk I/O", &[Off, Value, Bar]),
    ("Device CPU", &[Off, Percent, Bar]),
    ("Device RAM", &[Off, Percent, Value, Bar]),
    ("Device disk", &[Off, Percent, Value, Bar]),
];

/// What one selectable row on the Appearance page changes.
///
/// The rows are addressed by position — `appearance_index` is an index into
/// [`APPEARANCE_TABLE`] — so this enum is what turns that position back into a
/// setting. Everything the page needs to draw or edit a row (its heading, its
/// label, its one-line explanation, its current value, and the values it can
/// take) hangs off the table rather than off arithmetic spread between the
/// renderer and the cycler, which is what made the two drift apart before.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AppearanceControl {
    /// One editable theme colour, by its index into [`THEME_ROLES`].
    Color(usize),
    /// Whether the attention colour pulses.
    Blink,
    /// How long one attention pulse takes.
    BlinkRate,
    /// One resource indicator, by its index into [`RESOURCE_INDICATORS`].
    Resource(usize),
    /// Whether extracted harness titles appear on agent rows.
    SessionTitles,
    /// What the Agents sidebar sections its agents by.
    Grouping,
    /// How the Agents sidebar orders rows inside a section.
    Sort,
}

/// One row of the Appearance page.
pub(super) struct AppearanceRow {
    /// The group heading this row opens, when it is a group's first row.
    pub group: Option<&'static str>,
    /// The row's name, as shown in the left column.
    pub label: &'static str,
    /// One sentence for the pinned footer: what changing this row does.
    pub help: &'static str,
    /// The setting the row edits.
    pub control: AppearanceControl,
}

/// A row-table entry, kept terse because the table is read as a table.
const fn row(
    group: Option<&'static str>,
    label: &'static str,
    help: &'static str,
    control: AppearanceControl,
) -> AppearanceRow {
    AppearanceRow {
        group,
        label,
        help,
        control,
    }
}

/// Every Appearance row, in the order the page lists them.
///
/// The groups are the four questions the page answers — what Medulla is
/// coloured in, how it asks for attention, what the status line measures, and
/// how the Sessions sidebar is arranged. A row belongs to the group whose
/// *surface* it changes, not to the config section it happens to be written to,
/// which is what split the sidebar's own settings across a "Sessions sidebar"
/// group and an "Agents sidebar" group naming a panel that has been titled
/// "Sessions" for some time.
pub(super) const APPEARANCE_TABLE: [AppearanceRow; 16] = [
    row(
        Some("Colors"),
        "Primary",
        "The colour behind a selected row, the active tab, and the panel titles.",
        AppearanceControl::Color(0),
    ),
    row(
        None,
        "Accent",
        "The secondary highlight: counts, badges, and the softer chrome.",
        AppearanceControl::Color(1),
    ),
    row(
        None,
        "Selection text",
        "The text drawn on top of a selected row's primary background.",
        AppearanceControl::Color(2),
    ),
    row(
        None,
        "Dim border",
        "Panel borders, rules, and the lines that separate one pane from the next.",
        AppearanceControl::Color(3),
    ),
    row(
        Some("Attention"),
        "Color",
        "The colour an agent that is waiting on you is marked in.",
        AppearanceControl::Color(4),
    ),
    row(
        None,
        "Blink",
        "Whether that mark pulses or simply stays lit.",
        AppearanceControl::Blink,
    ),
    row(
        None,
        "Blink rate",
        "How long one pulse takes, from bright back to bright.",
        AppearanceControl::BlinkRate,
    ),
    row(
        Some("Status line"),
        "Process CPU",
        "Medulla's own CPU use, in the status line along the bottom.",
        AppearanceControl::Resource(0),
    ),
    row(
        None,
        "Process RAM",
        "Memory Medulla itself is holding, in the status line.",
        AppearanceControl::Resource(1),
    ),
    row(
        None,
        "Process disk I/O",
        "What Medulla is reading and writing per second, in the status line.",
        AppearanceControl::Resource(2),
    ),
    row(
        Some("Sessions sidebar"),
        "Session titles",
        "Whether an orchestrator-run agent shows the title taken from its work.",
        AppearanceControl::SessionTitles,
    ),
    row(
        None,
        "Device CPU",
        "This whole machine's CPU load, under the sidebar.",
        AppearanceControl::Resource(3),
    ),
    row(
        None,
        "Device RAM",
        "This whole machine's memory pressure, under the sidebar.",
        AppearanceControl::Resource(4),
    ),
    row(
        None,
        "Device disk",
        "How full this machine's disk is, under the sidebar.",
        AppearanceControl::Resource(5),
    ),
    row(
        None,
        "Group by",
        "What the sidebar sections its rows by. No row disappears either way.",
        AppearanceControl::Grouping,
    ),
    row(
        None,
        "Sort by",
        "The order rows come in inside a section.",
        AppearanceControl::Sort,
    ),
];

/// Number of selectable rows on the page.
pub(super) const APPEARANCE_ROWS: usize = APPEARANCE_TABLE.len();

/// The lower-case name a resource format is shown and persisted under.
pub(super) fn display_label(display: ResourceDisplay) -> String {
    format!("{display:?}").to_ascii_lowercase()
}

/// The label a pulse length is shown under, in the unit it is configured in.
fn blink_rate_label(seconds: f64) -> String {
    format!("{seconds:.1}s")
}

/// How every two-state row on the page spells itself.
fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.into()
}

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
    /// The row the cursor is on, clamped to the table.
    pub(super) fn appearance_row(&self) -> &'static AppearanceRow {
        &APPEARANCE_TABLE[self.appearance_index.min(APPEARANCE_ROWS - 1)]
    }

    /// The value shown in the selected row's right-hand column.
    pub(super) fn appearance_value(&self, index: usize) -> String {
        match APPEARANCE_TABLE[index.min(APPEARANCE_ROWS - 1)].control {
            AppearanceControl::Color(role) => color_to_string(self.theme.role(role)),
            AppearanceControl::Blink => on_off(self.theme.attention_blink),
            // A rate for an effect that is not running is a number that means
            // nothing, so it reads as an em dash until the pulse is switched on.
            AppearanceControl::BlinkRate if !self.theme.attention_blink => "—".into(),
            AppearanceControl::BlinkRate => {
                blink_rate_label(blink_seconds(self.theme.attention_blink_ms))
            }
            AppearanceControl::Resource(indicator) => {
                display_label(self.resource_display(indicator))
            }
            AppearanceControl::SessionTitles => {
                on_off(self.loaded.config.appearance.show_session_titles)
            }
            AppearanceControl::Grouping => self
                .loaded
                .config
                .appearance
                .sidebar_grouping
                .label()
                .into(),
            AppearanceControl::Sort => self.loaded.config.appearance.sidebar_sort.label().into(),
        }
    }

    /// Every value the selected row can take, in the order `←/→` walk them.
    ///
    /// The page shows this set with the current member highlighted, because a
    /// lone value says nothing about what else is on offer or what the next
    /// keypress is about to do — the operator would have to press it and watch.
    pub(super) fn appearance_choices(&self, index: usize) -> Vec<String> {
        match APPEARANCE_TABLE[index.min(APPEARANCE_ROWS - 1)].control {
            AppearanceControl::Color(_) => PALETTE.iter().copied().map(color_to_string).collect(),
            AppearanceControl::Blink | AppearanceControl::SessionTitles => {
                vec!["on".into(), "off".into()]
            }
            AppearanceControl::BlinkRate => BLINK_SECONDS
                .iter()
                .copied()
                .map(blink_rate_label)
                .collect(),
            AppearanceControl::Resource(indicator) => RESOURCE_INDICATORS[indicator]
                .1
                .iter()
                .copied()
                .map(display_label)
                .collect(),
            AppearanceControl::Grouping => medulla::config::SidebarGrouping::ALL
                .iter()
                .map(|value| value.label().to_string())
                .collect(),
            AppearanceControl::Sort => medulla::config::SidebarSort::ALL
                .iter()
                .map(|value| value.label().to_string())
                .collect(),
        }
    }

    /// The current format of resource indicator `index`.
    pub(super) fn resource_display(&self, index: usize) -> ResourceDisplay {
        let appearance = &self.loaded.config.appearance;
        match index {
            0 => appearance.cpu,
            1 => appearance.ram,
            2 => appearance.disk_io,
            3 => appearance.device_cpu,
            4 => appearance.device_ram,
            _ => appearance.device_disk,
        }
    }

    /// Cycle the selected row to its next (or previous) value and persist it.
    pub(super) fn cycle_appearance_row(&mut self, forward: bool) {
        match self.appearance_row().control {
            AppearanceControl::Color(role) => {
                self.theme.cycle_role(role, forward);
                self.persist_theme_value_now(
                    THEME_ROLES[role],
                    color_to_string(self.theme.role(role)),
                );
            }
            AppearanceControl::Blink => {
                self.theme.attention_blink = !self.theme.attention_blink;
                let value = on_off(self.theme.attention_blink);
                self.persist_theme_value_now("Attention blink", value);
            }
            AppearanceControl::BlinkRate => self.cycle_blink_rate(forward),
            AppearanceControl::Resource(indicator) => {
                self.cycle_resource_display(indicator, forward)
            }
            AppearanceControl::SessionTitles => self.toggle_session_titles(),
            AppearanceControl::Grouping => self.cycle_sidebar_grouping(forward),
            AppearanceControl::Sort => self.cycle_sidebar_sort(forward),
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
        self.persist_theme_value_now("Attention blink rate", blink_rate_label(next));
    }

    /// Toggle extracted harness titles on orchestrator-managed agent rows.
    fn toggle_session_titles(&mut self) {
        let appearance = &mut self.loaded.config.appearance;
        appearance.show_session_titles = !appearance.show_session_titles;
        let shown = on_off(appearance.show_session_titles);
        self.persist_appearance_now("Session titles", shown);
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
    /// `index` is the indicator's offset into [`RESOURCE_INDICATORS`], which is
    /// also the order the page lists them in. Each indicator offers only the
    /// formats that mean something for it: throughput has no percentage of a
    /// known total, and a device's CPU has no byte value.
    fn cycle_resource_display(&mut self, index: usize, forward: bool) {
        let (name, choices) = RESOURCE_INDICATORS[index.min(RESOURCE_INDICATORS.len() - 1)];
        let display = self.resource_display_mut(index);
        let next = cycled(choices, *display, forward);
        *display = next;
        self.persist_appearance_now(name, display_label(next));
    }

    /// The live config field behind indicator `index`.
    fn resource_display_mut(&mut self, index: usize) -> &mut medulla::config::ResourceDisplay {
        let appearance = &mut self.loaded.config.appearance;
        match index {
            0 => &mut appearance.cpu,
            1 => &mut appearance.ram,
            2 => &mut appearance.disk_io,
            3 => &mut appearance.device_cpu,
            4 => &mut appearance.device_ram,
            _ => &mut appearance.device_disk,
        }
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
