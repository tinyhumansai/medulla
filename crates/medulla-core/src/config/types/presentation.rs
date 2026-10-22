//! TUI appearance, theme, and onboarding configuration types.

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
    /// Color used when a harness or task needs operator attention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<String>,
    /// Whether operator-attention cues blink.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_blink: Option<bool>,
    /// How long one attention pulse takes, in seconds.
    ///
    /// A full cycle: the cue is bright for half of it and dim for the other
    /// half. Expressed in seconds because that is the unit the effect is judged
    /// in — "blink about once a second" is a thing an operator can ask for,
    /// "eleven frames" is not. Values outside the sane range are clamped rather
    /// than rejected, so a typo slows the pulse instead of disabling the config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_blink_seconds: Option<f64>,
    /// Optional btop-style resource panel colors and gradients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesThemeConfig>,
}

/// Optional color overrides for the resource panels. Keys use camelCase in
/// configuration, for example `cpuBox` and `cpuStart`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourcesThemeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_box: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_box: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_box: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proc_box: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub div_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter_bg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inactive_fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_end: Option<String>,
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
