//! Row metadata and mutations for the Status line settings page.

use medulla::config::{
    wire_value, ControlStyle, FieldPlacement, FieldVisibility, HarnessNameStyle, PathStyle,
    StatusLineConfig,
};

use super::super::types::App;
use super::{StatusLineField, StatusLineGroup, StatusLineRow};

/// The page's rows, in display order, grouped by the field they describe.
pub(in crate::ui::app) const STATUS_LINE_ROWS: [StatusLineRow; 17] = [
    row(
        "position",
        StatusLineField::State,
        head("State glyph", "the ●/✓/✕ dot: running, finished, or failed"),
        "Which line the state glyph sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::StateWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "position",
        StatusLineField::Harness,
        head("Harness name", "which CLI is driving the session"),
        "Which line the harness name sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::HarnessWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "spelled",
        StatusLineField::HarnessStyle,
        None,
        "Claude Code, claude, or just the provider icon.",
    ),
    row(
        "position",
        StatusLineField::Control,
        head(
            "Managed / unmanaged",
            "whether Medulla or you are driving the session",
        ),
        "Which line the control state sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::ControlWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "spelled",
        StatusLineField::ControlStyle,
        None,
        "Words (unmanaged), or the ⊘ / ⊙ symbols.",
    ),
    row(
        "position",
        StatusLineField::Thread,
        head("Thread name", "the conversation this session is working on"),
        "Which line the thread name sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::ThreadWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "position",
        StatusLineField::Branch,
        head("Git branch", "the branch the checkout is on"),
        "Which line the branch sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::BranchWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "position",
        StatusLineField::Worktree,
        head(
            "Worktree",
            "the linked worktree the checkout is, when it is one",
        ),
        "Which line the worktree name sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::WorktreeWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "position",
        StatusLineField::Path,
        head("Working path", "the checkout the harness is running in"),
        "Which line the working path sits on, or hide it.",
    ),
    row(
        "shown",
        StatusLineField::PathWhen,
        None,
        "On every row, only the selected one, or only alerts.",
    ),
    row(
        "spelled",
        StatusLineField::PathStyle,
        None,
        "The whole path, ~/…/tail, or the last segment alone.",
    ),
];
/// Number of selectable rows on the Status line page.
pub(in crate::ui::app) const STATUS_LINE_ROW_COUNT: usize = STATUS_LINE_ROWS.len();
/// `const fn` shorthand so the table above reads as a table.
const fn row(
    label: &'static str,
    field: StatusLineField,
    group: Option<StatusLineGroup>,
    help: &'static str,
) -> StatusLineRow {
    StatusLineRow {
        label,
        field,
        group,
        help,
    }
}
/// `const fn` shorthand for the heading a group's first row opens.
const fn head(title: &'static str, description: &'static str) -> Option<StatusLineGroup> {
    Some(StatusLineGroup { title, description })
}

impl StatusLineField {
    /// Every value this field can take, in the order `←/→` walks them.
    pub(in crate::ui::app) fn choices(self) -> Vec<&'static str> {
        match self {
            Self::State
            | Self::Harness
            | Self::Control
            | Self::Thread
            | Self::Branch
            | Self::Worktree
            | Self::Path => FieldPlacement::ALL.iter().map(|v| v.label()).collect(),
            Self::StateWhen
            | Self::HarnessWhen
            | Self::ControlWhen
            | Self::ThreadWhen
            | Self::BranchWhen
            | Self::WorktreeWhen
            | Self::PathWhen => FieldVisibility::ALL.iter().map(|v| v.label()).collect(),
            Self::HarnessStyle => HarnessNameStyle::ALL.iter().map(|v| v.label()).collect(),
            Self::ControlStyle => ControlStyle::ALL.iter().map(|v| v.label()).collect(),
            Self::PathStyle => PathStyle::ALL.iter().map(|v| v.label()).collect(),
        }
    }
    /// The config key this field persists under, matching serde's camelCase spelling.
    pub(in crate::ui::app) fn key(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::StateWhen => "stateWhen",
            Self::Harness => "harness",
            Self::HarnessWhen => "harnessWhen",
            Self::HarnessStyle => "harnessStyle",
            Self::Control => "control",
            Self::ControlWhen => "controlWhen",
            Self::ControlStyle => "controlStyle",
            Self::Thread => "thread",
            Self::ThreadWhen => "threadWhen",
            Self::Branch => "branch",
            Self::BranchWhen => "branchWhen",
            Self::Worktree => "worktree",
            Self::WorktreeWhen => "worktreeWhen",
            Self::Path => "path",
            Self::PathWhen => "pathWhen",
            Self::PathStyle => "pathStyle",
        }
    }
    /// This field's full name, for the status bar.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::State => "state glyph",
            Self::StateWhen => "state glyph shown",
            Self::Harness => "harness name",
            Self::HarnessWhen => "harness name shown",
            Self::HarnessStyle => "harness name spelled",
            Self::Control => "managed / unmanaged",
            Self::ControlWhen => "managed / unmanaged shown",
            Self::ControlStyle => "managed / unmanaged spelled",
            Self::Thread => "thread name",
            Self::ThreadWhen => "thread name shown",
            Self::Branch => "git branch",
            Self::BranchWhen => "git branch shown",
            Self::Worktree => "worktree",
            Self::WorktreeWhen => "worktree shown",
            Self::Path => "working path",
            Self::PathWhen => "working path shown",
            Self::PathStyle => "working path spelled",
        }
    }
    /// This field's current display label and persisted wire value.
    pub(in crate::ui::app) fn value(self, cfg: &StatusLineConfig) -> (&'static str, String) {
        match self {
            Self::State => (cfg.state.label(), wire_value(&cfg.state)),
            Self::StateWhen => (cfg.state_when.label(), wire_value(&cfg.state_when)),
            Self::Harness => (cfg.harness.label(), wire_value(&cfg.harness)),
            Self::HarnessWhen => (cfg.harness_when.label(), wire_value(&cfg.harness_when)),
            Self::HarnessStyle => (cfg.harness_style.label(), wire_value(&cfg.harness_style)),
            Self::Control => (cfg.control.label(), wire_value(&cfg.control)),
            Self::ControlWhen => (cfg.control_when.label(), wire_value(&cfg.control_when)),
            Self::ControlStyle => (cfg.control_style.label(), wire_value(&cfg.control_style)),
            Self::Thread => (cfg.thread.label(), wire_value(&cfg.thread)),
            Self::ThreadWhen => (cfg.thread_when.label(), wire_value(&cfg.thread_when)),
            Self::Branch => (cfg.branch.label(), wire_value(&cfg.branch)),
            Self::BranchWhen => (cfg.branch_when.label(), wire_value(&cfg.branch_when)),
            Self::Worktree => (cfg.worktree.label(), wire_value(&cfg.worktree)),
            Self::WorktreeWhen => (cfg.worktree_when.label(), wire_value(&cfg.worktree_when)),
            Self::Path => (cfg.path.label(), wire_value(&cfg.path)),
            Self::PathWhen => (cfg.path_when.label(), wire_value(&cfg.path_when)),
            Self::PathStyle => (cfg.path_style.label(), wire_value(&cfg.path_style)),
        }
    }
    /// Step this field's value on `cfg` one place, forwards or backwards.
    fn cycle(self, cfg: &mut StatusLineConfig, forward: bool) {
        match self {
            Self::State => cfg.state = cfg.state.cycled(forward),
            Self::StateWhen => cfg.state_when = cfg.state_when.cycled(forward),
            Self::Harness => cfg.harness = cfg.harness.cycled(forward),
            Self::HarnessWhen => cfg.harness_when = cfg.harness_when.cycled(forward),
            Self::HarnessStyle => cfg.harness_style = cfg.harness_style.cycled(forward),
            Self::Control => cfg.control = cfg.control.cycled(forward),
            Self::ControlWhen => cfg.control_when = cfg.control_when.cycled(forward),
            Self::ControlStyle => cfg.control_style = cfg.control_style.cycled(forward),
            Self::Thread => cfg.thread = cfg.thread.cycled(forward),
            Self::ThreadWhen => cfg.thread_when = cfg.thread_when.cycled(forward),
            Self::Branch => cfg.branch = cfg.branch.cycled(forward),
            Self::BranchWhen => cfg.branch_when = cfg.branch_when.cycled(forward),
            Self::Worktree => cfg.worktree = cfg.worktree.cycled(forward),
            Self::WorktreeWhen => cfg.worktree_when = cfg.worktree_when.cycled(forward),
            Self::Path => cfg.path = cfg.path.cycled(forward),
            Self::PathWhen => cfg.path_when = cfg.path_when.cycled(forward),
            Self::PathStyle => cfg.path_style = cfg.path_style.cycled(forward),
        }
    }
}

impl App {
    /// The status-line layout the settings page is editing.
    pub(in crate::ui::app) fn status_line_config(&self) -> StatusLineConfig {
        self.loaded.config.status_line()
    }
    /// Move the Status line page's selection.
    pub(in crate::ui::app) fn move_status_line_index(&mut self, up: bool) {
        self.status_line_index = if up {
            self.status_line_index.saturating_sub(1)
        } else {
            (self.status_line_index + 1).min(STATUS_LINE_ROW_COUNT - 1)
        };
    }
    /// Cycle the selected row's value, apply it live, and persist it.
    pub(in crate::ui::app) fn cycle_status_line_row(&mut self, forward: bool) {
        let row = STATUS_LINE_ROWS[self.status_line_index.min(STATUS_LINE_ROW_COUNT - 1)];
        let mut cfg = self.status_line_config();
        row.field.cycle(&mut cfg, forward);
        self.loaded.config.status_line = Some(cfg);
        let (label, wire) = row.field.value(&cfg);
        self.persist_status_line_now(&cfg, row.field.key(), row.field.name(), label, wire);
    }
    /// Write one status-line key to the injected config path.
    fn persist_status_line_now(
        &mut self,
        cfg: &StatusLineConfig,
        key: &str,
        row: &str,
        label: &str,
        wire: String,
    ) {
        match &self.config_path {
            Some(path) => {
                let result = if self.status_line_promotion_pending {
                    toml::Value::try_from(cfg)
                        .map_err(anyhow::Error::from)
                        .and_then(|value| match value {
                            toml::Value::Table(table) => {
                                medulla::config::persist_section(path, "statusLine", table)
                            }
                            _ => Err(anyhow::anyhow!(
                                "status-line config must serialize as a table"
                            )),
                        })
                } else {
                    medulla::config::persist_setting(
                        path,
                        "statusLine",
                        key,
                        toml::Value::String(wire),
                    )
                };
                match result {
                    Ok(()) => {
                        self.status_line_promotion_pending = false;
                        self.set_status(format!("Status line · {row} → {label} (saved)"));
                    }
                    Err(error) => self.set_status(format!("Status line · save failed: {error}")),
                }
            }
            None => self.set_status(format!("Status line · {row} → {label} (not persisted)")),
        }
    }
}
