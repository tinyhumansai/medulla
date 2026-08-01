//! The `[statusLine]` config section: which fields a harness row on the Agents
//! rail shows, where each one sits, and how each one is spelled.
//!
//! A harness row is the only place the operator learns what a session *is* —
//! which CLI, whose it is, which checkout, which branch. Different operators
//! need different parts of that: someone running four checkouts of one repo
//! needs the path and nothing else, someone handing sessions back and forth
//! needs the control state above all. Rather than pick for them, every field
//! declares its own [`FieldPlacement`], and the ones with more than one
//! reasonable spelling declare a style alongside it.
//!
//! Three lines is the ceiling, deliberately. The rail is a list beside the
//! surface the operator is actually reading; a row that can grow without bound
//! turns that list into a second transcript.

use super::*;

/// Where one status-line field is drawn, or that it is not drawn at all.
///
/// "Hidden" is a placement rather than a separate boolean so every field is
/// configured the same way and the settings page can cycle one control through
/// all three states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FieldPlacement {
    /// The first line of the harness row.
    #[default]
    Line1,
    /// The second line, indented under the first.
    Line2,
    /// The third line, indented under the first.
    Line3,
    /// Not drawn.
    Hidden,
}

impl FieldPlacement {
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            FieldPlacement::Line1 => "line 1",
            FieldPlacement::Line2 => "line 2",
            FieldPlacement::Line3 => "line 3",
            FieldPlacement::Hidden => "hidden",
        }
    }

    /// The next placement in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        const ORDER: [FieldPlacement; 4] = [
            FieldPlacement::Line1,
            FieldPlacement::Line2,
            FieldPlacement::Line3,
            FieldPlacement::Hidden,
        ];
        cycle(&ORDER, self, forward)
    }

    /// Which line index this field belongs on, if it is drawn at all.
    pub fn line(self) -> Option<usize> {
        match self {
            FieldPlacement::Line1 => Some(0),
            FieldPlacement::Line2 => Some(1),
            FieldPlacement::Line3 => Some(2),
            FieldPlacement::Hidden => None,
        }
    }
}

/// When a placed status-line field is actually drawn.
///
/// Placement answers *where*; this answers *whether, right now*. The two are
/// separate because the interesting cases are conditional: a working directory
/// that only appears on the row you have selected costs the other rows nothing,
/// and an error field that only appears when there is an error is invisible
/// until the moment it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FieldVisibility {
    /// Drawn on every harness row.
    #[default]
    Always,
    /// Drawn only on the row the rail cursor is on.
    Active,
    /// Drawn only when the harness needs attention — it failed, exited
    /// non-zero, or recorded an error.
    Alert,
}

impl FieldVisibility {
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            FieldVisibility::Always => "always",
            FieldVisibility::Active => "when selected",
            FieldVisibility::Alert => "on alert",
        }
    }

    /// The next visibility in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        const ORDER: [FieldVisibility; 3] = [
            FieldVisibility::Always,
            FieldVisibility::Active,
            FieldVisibility::Alert,
        ];
        cycle(&ORDER, self, forward)
    }

    /// Whether a field with this visibility is drawn on a row that is currently
    /// `active` (the rail cursor is on it) and/or in an `alert` state.
    pub fn shows(self, active: bool, alert: bool) -> bool {
        match self {
            FieldVisibility::Always => true,
            FieldVisibility::Active => active,
            FieldVisibility::Alert => alert,
        }
    }
}

/// How the harness provider is spelled on a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HarnessNameStyle {
    /// The product name — "Claude Code".
    Long,
    /// The command name — "claude". The historical rail spelling.
    #[default]
    Short,
    /// A single glyph, for operators who already know their harnesses apart.
    Icon,
}

impl HarnessNameStyle {
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            HarnessNameStyle::Long => "long",
            HarnessNameStyle::Short => "short",
            HarnessNameStyle::Icon => "icon",
        }
    }

    /// The next style in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        const ORDER: [HarnessNameStyle; 3] = [
            HarnessNameStyle::Long,
            HarnessNameStyle::Short,
            HarnessNameStyle::Icon,
        ];
        cycle(&ORDER, self, forward)
    }
}

/// How the managed/unmanaged control state is spelled.
///
/// Hiding it is [`FieldPlacement::Hidden`] rather than a third variant here, so
/// the "where" and the "how" stay separate questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ControlStyle {
    /// Words — "unmanaged" / "orchestrator".
    #[default]
    Text,
    /// A single glyph, for operators who have learned the two marks.
    Icon,
}

impl ControlStyle {
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            ControlStyle::Text => "text",
            ControlStyle::Icon => "icon",
        }
    }

    /// The next style in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        const ORDER: [ControlStyle; 2] = [ControlStyle::Text, ControlStyle::Icon];
        cycle(&ORDER, self, forward)
    }
}

/// How much of a harness's working directory is spelled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PathStyle {
    /// The whole path, cut at the right edge only when it does not fit.
    Full,
    /// `$HOME` collapsed to `~` and leading segments elided with `…`, keeping
    /// the tail that names the checkout. The historical rail spelling.
    #[default]
    Shortened,
    /// The last path segment alone — usually the directory name.
    Last,
}

impl PathStyle {
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            PathStyle::Full => "full",
            PathStyle::Shortened => "shortened",
            PathStyle::Last => "last segment",
        }
    }

    /// The next style in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        const ORDER: [PathStyle; 3] = [PathStyle::Full, PathStyle::Shortened, PathStyle::Last];
        cycle(&ORDER, self, forward)
    }
}

/// Step through a fixed option list, wrapping at either end.
///
/// Shared by every style enum above so `←`/`→` behave identically on every row
/// of the settings page. An unrecognized current value lands on the first
/// option, which cannot happen through the type system but keeps this total.
/// The camelCase string serde writes for `value`, for persisting one key.
///
/// Derived from the `Serialize` impl rather than hand-written per enum, so a
/// renamed variant cannot leave the writer and the reader disagreeing about the
/// spelling. Every enum here serializes as a plain string, so the unwrap is
/// unreachable; it falls back to the default's spelling rather than panicking
/// in a draw path.
pub fn wire_value<T: Serialize + Default>(value: &T) -> String {
    fn as_string<T: Serialize>(value: &T) -> Option<String> {
        match serde_json::to_value(value).ok()? {
            Value::String(text) => Some(text),
            _ => None,
        }
    }
    as_string(value)
        .or_else(|| as_string(&T::default()))
        .unwrap_or_default()
}

fn cycle<T: Copy + PartialEq>(order: &[T], current: T, forward: bool) -> T {
    let index = order.iter().position(|item| *item == current).unwrap_or(0);
    let len = order.len();
    let next = if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    };
    order[next]
}

/// The operator's harness status-line layout.
///
/// Defaults reproduce the row exactly as it was before this section existed —
/// `● codex · unmanaged · main · ~/work/medulla`, all on one line — so an
/// installation that never opens the settings page sees no change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatusLineConfig {
    /// Where the run-state glyph (`●` / `✓` / `✕`) is drawn.
    pub state: FieldPlacement,
    /// When the run-state glyph is drawn.
    pub state_when: FieldVisibility,
    /// Where the harness provider name is drawn.
    pub harness: FieldPlacement,
    /// When the harness provider name is drawn.
    pub harness_when: FieldVisibility,
    /// How the harness provider name is spelled.
    pub harness_style: HarnessNameStyle,
    /// Where the managed/unmanaged control state is drawn.
    pub control: FieldPlacement,
    /// When the control state is drawn.
    pub control_when: FieldVisibility,
    /// How the control state is spelled.
    pub control_style: ControlStyle,
    /// Where the Git branch is drawn. Rows for a non-repository working
    /// directory omit it regardless.
    pub branch: FieldPlacement,
    /// When the Git branch is drawn.
    pub branch_when: FieldVisibility,
    /// Where the working directory is drawn.
    pub path: FieldPlacement,
    /// When the working directory is drawn.
    pub path_when: FieldVisibility,
    /// How much of the working directory is spelled out.
    pub path_style: PathStyle,
}

impl Default for StatusLineConfig {
    fn default() -> Self {
        Self {
            state: FieldPlacement::Line1,
            state_when: FieldVisibility::Always,
            harness: FieldPlacement::Line1,
            harness_when: FieldVisibility::Always,
            harness_style: HarnessNameStyle::Short,
            control: FieldPlacement::Line1,
            control_when: FieldVisibility::Always,
            control_style: ControlStyle::Text,
            branch: FieldPlacement::Line1,
            branch_when: FieldVisibility::Always,
            path: FieldPlacement::Line1,
            path_when: FieldVisibility::Always,
            path_style: PathStyle::Shortened,
        }
    }
}

impl StatusLineConfig {
    /// The layout implied by the older `[appearance]` booleans.
    ///
    /// `showHarnessBranch` and `showHarnessPath` were the whole of this feature
    /// before it grew a page, and an operator who turned one off meant it. A
    /// config that has never written `[statusLine]` is read through this so the
    /// upgrade is invisible; the first edit on the settings page writes the new
    /// section, which then wins outright.
    pub fn from_appearance(appearance: &AppearanceConfig) -> Self {
        Self {
            branch: placement_of(appearance.show_harness_branch),
            path: placement_of(appearance.show_harness_path),
            ..Self::default()
        }
    }
}

/// A legacy boolean read as a placement.
fn placement_of(shown: bool) -> FieldPlacement {
    if shown {
        FieldPlacement::Line1
    } else {
        FieldPlacement::Hidden
    }
}
