//! TUI appearance and onboarding configuration types.

use super::*;

/// The optional `[theme]` config section: named ratatui colors (case-insensitive)
/// or `#rrggbb` hex strings. Missing fields fall back to the default theme. The
/// Appearance settings subpage persists these keys.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeConfig {
    /// Primary highlight color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    /// Secondary accent color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// Foreground color for selected rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_fg: Option<String>,
    /// Color used for inactive or secondary borders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim_border: Option<String>,
}

/// Onboarding state: what the welcome flow has already shown this user.
///
/// Purely a display gate. Whether the user actually *earned* the history reward
/// is the backend's answer (`GET /agent-integrations/history-rewards/status`);
/// this flag only stops the welcome screen reappearing every launch, including
/// for a user who deliberately skipped it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct OnboardingConfig {
    /// True once the user has completed or skipped the welcome flow.
    pub welcome_completed: bool,
}
