//! Data types for the configurable harness status line.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::AppearanceConfig;

/// Where one status-line field is drawn, or that it is not drawn at all.
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

/// When a placed status-line field is actually drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FieldVisibility {
    /// Drawn on every harness row.
    #[default]
    Always,
    /// Drawn only on the row the rail cursor is on.
    Active,
    /// Drawn only when the harness needs attention.
    Alert,
}

/// How the harness provider is spelled on a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HarnessNameStyle {
    /// The product name, such as "Claude Code".
    Long,
    /// The command name, such as "claude".
    #[default]
    Short,
    /// A single provider glyph.
    Icon,
}

/// How the managed or unmanaged control state is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ControlStyle {
    /// Words such as "unmanaged" or "orchestrator".
    #[default]
    Text,
    /// A single control-state glyph.
    Icon,
}

/// How much of a harness's working directory is spelled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PathStyle {
    /// The whole path, with its leading edge elided when it does not fit so the
    /// checkout name remains visible.
    Full,
    /// The home directory collapsed and leading segments elided as needed.
    #[default]
    Shortened,
    /// The last path segment alone.
    Last,
}

/// The operator's harness status-line layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatusLineConfig {
    /// Where the run-state glyph is drawn.
    pub state: FieldPlacement,
    /// When the run-state glyph is drawn.
    pub state_when: FieldVisibility,
    /// Where the harness provider name is drawn.
    pub harness: FieldPlacement,
    /// When the harness provider name is drawn.
    pub harness_when: FieldVisibility,
    /// How the harness provider name is spelled.
    pub harness_style: HarnessNameStyle,
    /// Where the managed or unmanaged control state is drawn.
    pub control: FieldPlacement,
    /// When the control state is drawn.
    pub control_when: FieldVisibility,
    /// How the control state is spelled.
    pub control_style: ControlStyle,
    /// Where the harness's human-facing thread name is drawn.
    pub thread: FieldPlacement,
    /// When the thread name is drawn.
    pub thread_when: FieldVisibility,
    /// Where the Git branch is drawn.
    pub branch: FieldPlacement,
    /// When the Git branch is drawn.
    pub branch_when: FieldVisibility,
    /// Where the linked worktree's name is drawn.
    ///
    /// Costs nothing on a row that has no worktree to name: a session in a
    /// repository's primary checkout draws this field as nothing at all, which
    /// is the whole reason it can default to visible.
    pub worktree: FieldPlacement,
    /// When the linked worktree's name is drawn.
    pub worktree_when: FieldVisibility,
    /// Where the working directory is drawn.
    pub path: FieldPlacement,
    /// When the working directory is drawn.
    pub path_when: FieldVisibility,
    /// How much of the working directory is spelled out.
    pub path_style: PathStyle,
}

impl FieldPlacement {
    /// Every placement, in the order `cycled` steps through them.
    pub const ALL: [Self; 4] = [Self::Line1, Self::Line2, Self::Line3, Self::Hidden];

    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::Line1 => "line 1",
            Self::Line2 => "line 2",
            Self::Line3 => "line 3",
            Self::Hidden => "hidden",
        }
    }
    /// The next placement in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        cycle(&Self::ALL, self, forward)
    }
    /// Which line index this field belongs on, if it is drawn at all.
    pub fn line(self) -> Option<usize> {
        match self {
            Self::Line1 => Some(0),
            Self::Line2 => Some(1),
            Self::Line3 => Some(2),
            Self::Hidden => None,
        }
    }
}

impl FieldVisibility {
    /// Every visibility, in the order `cycled` steps through them.
    pub const ALL: [Self; 3] = [Self::Always, Self::Active, Self::Alert];
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Active => "when selected",
            Self::Alert => "on alert",
        }
    }
    /// The next visibility in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        cycle(&Self::ALL, self, forward)
    }
    /// Whether this visibility applies to the current row state.
    pub fn shows(self, active: bool, alert: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Active => active,
            Self::Alert => alert,
        }
    }
}

impl HarnessNameStyle {
    /// Every spelling, in the order `cycled` steps through them.
    pub const ALL: [Self; 3] = [Self::Long, Self::Short, Self::Icon];
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
            Self::Icon => "icon",
        }
    }
    /// The next style in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        cycle(&Self::ALL, self, forward)
    }
}
impl ControlStyle {
    /// Every spelling, in the order `cycled` steps through them.
    pub const ALL: [Self; 2] = [Self::Text, Self::Icon];
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Icon => "icon",
        }
    }
    /// The next style in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        cycle(&Self::ALL, self, forward)
    }
}
impl PathStyle {
    /// Every spelling, in the order `cycled` steps through them.
    pub const ALL: [Self; 3] = [Self::Full, Self::Shortened, Self::Last];
    /// The label shown on the Status line settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Shortened => "shortened",
            Self::Last => "last segment",
        }
    }
    /// The next style in the cycle, forwards or backwards.
    pub fn cycled(self, forward: bool) -> Self {
        cycle(&Self::ALL, self, forward)
    }
}

/// The camelCase string serde writes for `value`, for persisting one key.
///
/// If `value` cannot serialize to a string, this retries with [`Default::default`].
/// If neither value serializes to a string, it returns an empty string rather
/// than reporting a serialization error.
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

/// Steps through a fixed option list, wrapping at its ends.
fn cycle<T: Copy + PartialEq>(order: &[T], current: T, forward: bool) -> T {
    let index = order.iter().position(|item| *item == current).unwrap_or(0);
    let len = order.len();
    order[if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    }]
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
            thread: FieldPlacement::Line2,
            thread_when: FieldVisibility::Always,
            branch: FieldPlacement::Line1,
            branch_when: FieldVisibility::Always,
            worktree: FieldPlacement::Line1,
            worktree_when: FieldVisibility::Always,
            path: FieldPlacement::Line1,
            path_when: FieldVisibility::Always,
            path_style: PathStyle::Shortened,
        }
    }
}
impl StatusLineConfig {
    /// The layout implied by the older `[appearance]` booleans.
    pub fn from_appearance(appearance: &AppearanceConfig) -> Self {
        Self {
            branch: placement_of(appearance.show_harness_branch),
            // Follows the branch rather than defaulting to shown: an operator
            // who turned the Git detail off on the legacy switch meant the Git
            // detail, and a worktree name appearing where a branch used to be
            // suppressed would read as the setting having stopped working.
            worktree: placement_of(appearance.show_harness_branch),
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
