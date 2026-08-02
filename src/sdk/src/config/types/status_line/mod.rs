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

mod types;

#[cfg(test)]
mod tests;

pub use types::*;

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

/// Steps through a fixed option list, wrapping forwards or backwards at its ends.
///
/// Shared by every style enum above so `←`/`→` behave identically on every row
/// of the settings page. An unrecognized current value starts from the first
/// option, which cannot happen for these enums but keeps this helper total.
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
