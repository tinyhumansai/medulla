//! Data types for the configurable harness status line.

use serde::{Deserialize, Serialize};

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
    /// The whole path, cut at the right edge only when it does not fit.
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
    /// Where the Git branch is drawn.
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
