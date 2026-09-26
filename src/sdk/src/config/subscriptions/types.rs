//! Configuration types for the subscription-usage meters.

use serde::{Deserialize, Serialize};

use super::super::appearance::ResourceDisplay;

/// One usage meter Medulla can track and show in the Agents sidebar.
///
/// The identifier is the unit of configuration: every meter is independently
/// switchable, because which subscriptions an operator holds — and which of
/// their windows are worth watching — differs per machine. Meters whose source
/// is not present on the device simply report themselves unavailable rather
/// than disappearing, so a configured-on meter never silently goes quiet.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum MeterId {
    /// Claude's rolling five-hour session window.
    ClaudeSession,
    /// Claude's seven-day account window, across every model.
    ClaudeWeekly,
    /// Claude's seven-day window scoped to one model (Fable, Opus, …).
    ClaudeScoped,
    /// Claude's usage-credit spend against its monthly cap.
    ClaudeCredits,
    /// Codex's shorter rate-limit window, as the provider reports it.
    CodexSession,
    /// Codex's longer (weekly) rate-limit window.
    CodexWeekly,
    /// OpenRouter's prepaid credit balance.
    OpenRouterCredits,
    /// The TinyHumans portal's remaining inference balance.
    TinyhumansBalance,
}

impl MeterId {
    /// Every meter, in the order the sidebar and the settings page list them.
    ///
    /// Provider-grouped rather than sorted: an operator reads the page looking
    /// for one subscription, and the windows of a subscription only mean
    /// something beside each other.
    pub const ALL: [Self; 8] = [
        Self::ClaudeSession,
        Self::ClaudeWeekly,
        Self::ClaudeScoped,
        Self::ClaudeCredits,
        Self::CodexSession,
        Self::CodexWeekly,
        Self::OpenRouterCredits,
        Self::TinyhumansBalance,
    ];

    /// The short label the sidebar prefixes the reading with.
    ///
    /// Kept narrow on purpose: these lines share the rail with the device
    /// readings, and the rail is sized to its widest line.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ClaudeSession => "Claude 5h",
            Self::ClaudeWeekly => "Claude week",
            Self::ClaudeScoped => "Claude model",
            Self::ClaudeCredits => "Claude credit",
            Self::CodexSession => "Codex 5h",
            Self::CodexWeekly => "Codex week",
            Self::OpenRouterCredits => "OpenRouter",
            Self::TinyhumansBalance => "TinyHumans",
        }
    }

    /// The `[subscriptions]` key this meter's display setting is stored under.
    pub const fn config_key(self) -> &'static str {
        match self {
            Self::ClaudeSession => "claudeSession",
            Self::ClaudeWeekly => "claudeWeekly",
            Self::ClaudeScoped => "claudeScoped",
            Self::ClaudeCredits => "claudeCredits",
            Self::CodexSession => "codexSession",
            Self::CodexWeekly => "codexWeekly",
            Self::OpenRouterCredits => "openRouterCredits",
            Self::TinyhumansBalance => "tinyhumansBalance",
        }
    }

    /// The source this meter is read from, for grouping and for deciding which
    /// probes a sample has to run at all.
    pub const fn source(self) -> MeterSource {
        match self {
            Self::ClaudeSession | Self::ClaudeWeekly | Self::ClaudeScoped | Self::ClaudeCredits => {
                MeterSource::Claude
            }
            Self::CodexSession | Self::CodexWeekly => MeterSource::Codex,
            Self::OpenRouterCredits => MeterSource::OpenRouter,
            Self::TinyhumansBalance => MeterSource::Tinyhumans,
        }
    }

    /// Whether this meter reads as money rather than as a share of a window.
    ///
    /// A balance has no meaningful percentage unless a cap is known, so the
    /// formatter shows the amount left and only draws a bar when there is
    /// something to draw it against.
    pub const fn is_balance(self) -> bool {
        matches!(
            self,
            Self::OpenRouterCredits | Self::TinyhumansBalance | Self::ClaudeCredits
        )
    }
}

/// Where a group of meters is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeterSource {
    /// Claude's OAuth usage endpoint, authenticated with the local CLI's token.
    Claude,
    /// The Codex CLI's own rollout transcripts, which record its rate limits.
    Codex,
    /// OpenRouter's credits endpoint.
    OpenRouter,
    /// The TinyHumans backend account this TUI is signed in to.
    Tinyhumans,
}

/// How often the meters are re-sampled, in seconds, when nothing asks sooner.
///
/// Five minutes: the windows these meters track move over hours, and the two
/// network probes are on someone else's rate limit. Manual refresh on the
/// settings page is what covers "I want to know right now".
pub const DEFAULT_REFRESH_SECONDS: u64 = 300;

/// Shortest automatic sampling interval accepted from configuration.
///
/// Provider probes use the operator's authenticated accounts, so a malformed
/// hand-written value must not turn the UI tick into a request loop.
pub const MIN_REFRESH_SECONDS: u64 = 60;

/// The `[subscriptions]` section: which usage meters are shown and how.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct SubscriptionsConfig {
    /// Claude's rolling five-hour window.
    pub claude_session: ResourceDisplay,
    /// Claude's seven-day account window.
    pub claude_weekly: ResourceDisplay,
    /// Claude's seven-day per-model window.
    pub claude_scoped: ResourceDisplay,
    /// Claude's usage credits against their monthly cap.
    pub claude_credits: ResourceDisplay,
    /// Codex's shorter rate-limit window.
    pub codex_session: ResourceDisplay,
    /// Codex's weekly rate-limit window.
    pub codex_weekly: ResourceDisplay,
    /// OpenRouter's prepaid balance.
    pub open_router_credits: ResourceDisplay,
    /// The TinyHumans portal balance.
    pub tinyhumans_balance: ResourceDisplay,
    /// Seconds between automatic re-samples.
    pub refresh_seconds: u64,
    /// Environment variable holding the OpenRouter key the balance is read with.
    ///
    /// Named rather than inlined: this section is written back by the settings
    /// page, and a config file the TUI rewrites is the last place a secret
    /// should live.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_router_key_env: Option<String>,
}

impl SubscriptionsConfig {
    /// Defaults used when the section is absent: every meter off.
    ///
    /// Off, like the resource indicators, because two of these meters make
    /// network calls on the operator's own subscriptions. Nothing reaches a
    /// provider until a meter is switched on.
    pub const fn with_defaults() -> Self {
        Self {
            claude_session: ResourceDisplay::Off,
            claude_weekly: ResourceDisplay::Off,
            claude_scoped: ResourceDisplay::Off,
            claude_credits: ResourceDisplay::Off,
            codex_session: ResourceDisplay::Off,
            codex_weekly: ResourceDisplay::Off,
            open_router_credits: ResourceDisplay::Off,
            tinyhumans_balance: ResourceDisplay::Off,
            refresh_seconds: DEFAULT_REFRESH_SECONDS,
            open_router_key_env: None,
        }
    }

    /// How `meter` is displayed.
    pub const fn display(&self, meter: MeterId) -> ResourceDisplay {
        match meter {
            MeterId::ClaudeSession => self.claude_session,
            MeterId::ClaudeWeekly => self.claude_weekly,
            MeterId::ClaudeScoped => self.claude_scoped,
            MeterId::ClaudeCredits => self.claude_credits,
            MeterId::CodexSession => self.codex_session,
            MeterId::CodexWeekly => self.codex_weekly,
            MeterId::OpenRouterCredits => self.open_router_credits,
            MeterId::TinyhumansBalance => self.tinyhumans_balance,
        }
    }

    /// A mutable handle on `meter`'s display, for the settings page's cycling.
    pub fn display_mut(&mut self, meter: MeterId) -> &mut ResourceDisplay {
        match meter {
            MeterId::ClaudeSession => &mut self.claude_session,
            MeterId::ClaudeWeekly => &mut self.claude_weekly,
            MeterId::ClaudeScoped => &mut self.claude_scoped,
            MeterId::ClaudeCredits => &mut self.claude_credits,
            MeterId::CodexSession => &mut self.codex_session,
            MeterId::CodexWeekly => &mut self.codex_weekly,
            MeterId::OpenRouterCredits => &mut self.open_router_credits,
            MeterId::TinyhumansBalance => &mut self.tinyhumans_balance,
        }
    }

    /// Whether any meter is switched on at all.
    ///
    /// The gate on sampling: with everything off, no probe runs and no request
    /// leaves the machine.
    pub fn any_enabled(&self) -> bool {
        MeterId::ALL
            .iter()
            .any(|meter| self.display(*meter) != ResourceDisplay::Off)
    }

    /// Whether any meter reading from `source` is switched on.
    pub fn source_enabled(&self, source: MeterSource) -> bool {
        MeterId::ALL
            .iter()
            .any(|meter| meter.source() == source && self.display(*meter) != ResourceDisplay::Off)
    }

    /// The refresh interval used by the automatic scheduler.
    ///
    /// Values below one minute are clamped at the scheduling boundary. The
    /// public field remains unchanged so the settings page can show and replace
    /// a hand-written value rather than silently rewriting the file on load.
    pub fn sampling_interval_seconds(&self) -> u64 {
        self.refresh_seconds.max(MIN_REFRESH_SECONDS)
    }
}

impl Default for SubscriptionsConfig {
    fn default() -> Self {
        Self::with_defaults()
    }
}
