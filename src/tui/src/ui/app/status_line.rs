//! The Status line settings page's model: its rows, what changing one does, and
//! how each change is written back.
//!
//! Every row is one field of [`StatusLineConfig`], and changing any of them is
//! the same operation — cycle the value, apply it live, persist the single key
//! that moved. Keeping that uniform is what lets the page grow a row without
//! growing a code path.
//!
//! Persistence writes one key at a time rather than the whole `[statusLine]`
//! table, because the config path is usually `medulla.tui.json` and only
//! [`persist_setting`](medulla::config::persist_setting) understands both the
//! JSON and TOML forms.

use medulla::config::{wire_value, StatusLineConfig};

use super::types::App;

/// One editable row on the Status line page.
///
/// Each of a field's three questions — where, when, how spelled — is its own
/// row rather than a second axis of one row: `←/→` then means the same thing
/// everywhere, and the page reads as a list of answers rather than as a grid
/// needing another key to move within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StatusLineRow {
    /// The row's label, as shown on the page.
    pub label: &'static str,
    /// Which field of the config it edits.
    pub field: StatusLineField,
    /// Whether it qualifies the row above it, and so is indented under it.
    pub is_qualifier: bool,
}

/// Which value on [`StatusLineConfig`] a row edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusLineField {
    /// Where the run-state glyph sits.
    State,
    /// When the run-state glyph is drawn.
    StateWhen,
    /// Where the harness name sits.
    Harness,
    /// When the harness name is drawn.
    HarnessWhen,
    /// How the harness name is spelled.
    HarnessStyle,
    /// Where the control state sits.
    Control,
    /// When the control state is drawn.
    ControlWhen,
    /// How the control state is spelled.
    ControlStyle,
    /// Where the Git branch sits.
    Branch,
    /// When the Git branch is drawn.
    BranchWhen,
    /// Where the working directory sits.
    Path,
    /// When the working directory is drawn.
    PathWhen,
    /// How much of the working directory is spelled out.
    PathStyle,
}

/// The page's rows, in display order. A field's qualifiers follow it.
pub(super) const STATUS_LINE_ROWS: [StatusLineRow; 13] = [
    row("State glyph", StatusLineField::State, false),
    row("shown", StatusLineField::StateWhen, true),
    row("Harness name", StatusLineField::Harness, false),
    row("shown", StatusLineField::HarnessWhen, true),
    row("spelled", StatusLineField::HarnessStyle, true),
    row("Managed / unmanaged", StatusLineField::Control, false),
    row("shown", StatusLineField::ControlWhen, true),
    row("spelled", StatusLineField::ControlStyle, true),
    row("Git branch", StatusLineField::Branch, false),
    row("shown", StatusLineField::BranchWhen, true),
    row("Working path", StatusLineField::Path, false),
    row("shown", StatusLineField::PathWhen, true),
    row("spelled", StatusLineField::PathStyle, true),
];

/// Number of selectable rows on the Status line page.
pub(super) const STATUS_LINE_ROW_COUNT: usize = STATUS_LINE_ROWS.len();

/// `const fn` shorthand so the table above reads as a table.
const fn row(label: &'static str, field: StatusLineField, is_qualifier: bool) -> StatusLineRow {
    StatusLineRow {
        label,
        field,
        is_qualifier,
    }
}

impl StatusLineField {
    /// The config key this field persists under, matching serde's camelCase
    /// spelling of [`StatusLineConfig`].
    fn key(self) -> &'static str {
        match self {
            StatusLineField::State => "state",
            StatusLineField::StateWhen => "stateWhen",
            StatusLineField::Harness => "harness",
            StatusLineField::HarnessWhen => "harnessWhen",
            StatusLineField::HarnessStyle => "harnessStyle",
            StatusLineField::Control => "control",
            StatusLineField::ControlWhen => "controlWhen",
            StatusLineField::ControlStyle => "controlStyle",
            StatusLineField::Branch => "branch",
            StatusLineField::BranchWhen => "branchWhen",
            StatusLineField::Path => "path",
            StatusLineField::PathWhen => "pathWhen",
            StatusLineField::PathStyle => "pathStyle",
        }
    }

    /// This field's full name, for the status bar.
    ///
    /// Distinct from the row label, which is abbreviated to "shown" / "spelled"
    /// because the page indents it under the field it belongs to. A status
    /// message has no such context — "Status line · shown → always" names
    /// nothing.
    fn name(self) -> &'static str {
        match self {
            StatusLineField::State => "state glyph",
            StatusLineField::StateWhen => "state glyph shown",
            StatusLineField::Harness => "harness name",
            StatusLineField::HarnessWhen => "harness name shown",
            StatusLineField::HarnessStyle => "harness name spelled",
            StatusLineField::Control => "managed / unmanaged",
            StatusLineField::ControlWhen => "managed / unmanaged shown",
            StatusLineField::ControlStyle => "managed / unmanaged spelled",
            StatusLineField::Branch => "git branch",
            StatusLineField::BranchWhen => "git branch shown",
            StatusLineField::Path => "working path",
            StatusLineField::PathWhen => "working path shown",
            StatusLineField::PathStyle => "working path spelled",
        }
    }

    /// This field's current value on `cfg`, as the label the page shows and the
    /// string that is persisted.
    pub(super) fn value(self, cfg: &StatusLineConfig) -> (&'static str, String) {
        match self {
            StatusLineField::State => (cfg.state.label(), wire_value(&cfg.state)),
            StatusLineField::StateWhen => (cfg.state_when.label(), wire_value(&cfg.state_when)),
            StatusLineField::Harness => (cfg.harness.label(), wire_value(&cfg.harness)),
            StatusLineField::HarnessWhen => {
                (cfg.harness_when.label(), wire_value(&cfg.harness_when))
            }
            StatusLineField::HarnessStyle => {
                (cfg.harness_style.label(), wire_value(&cfg.harness_style))
            }
            StatusLineField::Control => (cfg.control.label(), wire_value(&cfg.control)),
            StatusLineField::ControlWhen => {
                (cfg.control_when.label(), wire_value(&cfg.control_when))
            }
            StatusLineField::ControlStyle => {
                (cfg.control_style.label(), wire_value(&cfg.control_style))
            }
            StatusLineField::Branch => (cfg.branch.label(), wire_value(&cfg.branch)),
            StatusLineField::BranchWhen => (cfg.branch_when.label(), wire_value(&cfg.branch_when)),
            StatusLineField::Path => (cfg.path.label(), wire_value(&cfg.path)),
            StatusLineField::PathWhen => (cfg.path_when.label(), wire_value(&cfg.path_when)),
            StatusLineField::PathStyle => (cfg.path_style.label(), wire_value(&cfg.path_style)),
        }
    }

    /// Step this field's value on `cfg` one place, forwards or backwards.
    fn cycle(self, cfg: &mut StatusLineConfig, forward: bool) {
        match self {
            StatusLineField::State => cfg.state = cfg.state.cycled(forward),
            StatusLineField::StateWhen => cfg.state_when = cfg.state_when.cycled(forward),
            StatusLineField::Harness => cfg.harness = cfg.harness.cycled(forward),
            StatusLineField::HarnessWhen => cfg.harness_when = cfg.harness_when.cycled(forward),
            StatusLineField::HarnessStyle => {
                cfg.harness_style = cfg.harness_style.cycled(forward);
            }
            StatusLineField::Control => cfg.control = cfg.control.cycled(forward),
            StatusLineField::ControlWhen => cfg.control_when = cfg.control_when.cycled(forward),
            StatusLineField::ControlStyle => {
                cfg.control_style = cfg.control_style.cycled(forward);
            }
            StatusLineField::Branch => cfg.branch = cfg.branch.cycled(forward),
            StatusLineField::BranchWhen => cfg.branch_when = cfg.branch_when.cycled(forward),
            StatusLineField::Path => cfg.path = cfg.path.cycled(forward),
            StatusLineField::PathWhen => cfg.path_when = cfg.path_when.cycled(forward),
            StatusLineField::PathStyle => cfg.path_style = cfg.path_style.cycled(forward),
        }
    }
}

impl App {
    /// The status-line layout the settings page is editing.
    ///
    /// Materializes the derived-from-`[appearance]` layout on first read so the
    /// page always has a concrete value to show, without writing anything: an
    /// operator who only *looks* at the page should not have their config
    /// rewritten.
    pub(super) fn status_line_config(&self) -> StatusLineConfig {
        self.loaded.config.status_line()
    }

    /// Move the Status line page's selection.
    pub(super) fn move_status_line_index(&mut self, up: bool) {
        self.status_line_index = if up {
            self.status_line_index.saturating_sub(1)
        } else {
            (self.status_line_index + 1).min(STATUS_LINE_ROW_COUNT - 1)
        };
    }

    /// Cycle the selected row's value, apply it live, and persist it.
    pub(super) fn cycle_status_line_row(&mut self, forward: bool) {
        let row = STATUS_LINE_ROWS[self.status_line_index.min(STATUS_LINE_ROW_COUNT - 1)];
        let mut cfg = self.status_line_config();
        row.field.cycle(&mut cfg, forward);
        // Write the whole struct back: the first edit is also what promotes a
        // config that only had the legacy `[appearance]` booleans into an
        // explicit `[statusLine]` section.
        self.loaded.config.status_line = Some(cfg);
        let (label, wire) = row.field.value(&cfg);
        self.persist_status_line_now(row.field.key(), row.field.name(), label, wire);
    }

    /// Write one status-line key to the injected config path.
    ///
    /// Reports the target in the status bar either way: config is layered, so a
    /// higher-precedence file can still override what was just written, and an
    /// operator whose change did not stick needs to know which file was tried.
    fn persist_status_line_now(&mut self, key: &str, row: &str, label: &str, wire: String) {
        match &self.config_path {
            Some(path) => {
                match medulla::config::persist_setting(
                    path,
                    "statusLine",
                    key,
                    toml::Value::String(wire),
                ) {
                    Ok(()) => self.set_status(format!("Status line · {row} → {label} (saved)")),
                    Err(error) => self.set_status(format!("Status line · save failed: {error}")),
                }
            }
            None => self.set_status(format!("Status line · {row} → {label} (not persisted)")),
        }
    }
}
