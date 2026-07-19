//! The welcome screen's data model: the command/event/outcome enums the driver
//! exchanges with the screen, the internal step marker, and the
//! [`WelcomeScreen`] struct with its constructor and trivial accessors.
//!
//! Fields, the [`Step`] marker, and the spinner helper are `pub(super)` so the
//! sibling state ([`super::state`]) and rendering ([`super::draw`]) modules can
//! drive and read them; nothing internal escapes the `welcome` module tree.
//!
//! The screen deliberately holds only display numbers. The actual transcripts
//! stay with the driver ([`super::run`]), so the pure state machine never owns
//! file contents and stays trivially testable.

use medulla::ui::util::SPINNER;

/// Default advertised ceiling, used until the backend states its own.
pub const DEFAULT_MAX_REWARD_USD: f64 = 25.0;

/// Format a USD amount, dropping the cents when it is a whole number of dollars.
///
/// Shared by the reveal panel and the post-flow startup status so a user sees
/// the same "$7" in both places rather than "$7" and "$7.00".
pub fn format_usd(amount: f64) -> String {
    if (amount - amount.round()).abs() < f64::EPSILON {
        format!("${}", amount.round() as i64)
    } else {
        format!("${amount:.2}")
    }
}

/// The terminal outcome of the welcome screen.
#[derive(Debug, Clone, PartialEq)]
pub enum WelcomeOutcome {
    /// The flow ran to completion and the reward was settled.
    Completed {
        /// USD granted (0.0 when the history scored nothing).
        awarded_usd: f64,
        /// Human-facing power-level label, when the backend supplied one.
        tier: Option<String>,
    },
    /// The user declined, or had nothing to share. Onboarding is still marked
    /// done so the screen does not reappear on every launch.
    Skipped,
}

/// An async action the driver must run on the screen's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WelcomeCmd {
    /// Read local transcript metadata. Read-only and local — no consent needed
    /// yet, since nothing leaves the machine.
    Scan,
    /// Redact, upload, and claim. Only ever emitted after explicit consent.
    UploadAndClaim,
}

/// What scanning found, in display terms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Per-agent rows, pre-rendered as `(agent, session_count)`.
    pub per_agent: Vec<(String, usize)>,
    /// Total transcripts that would be uploaded.
    pub session_count: usize,
    /// Combined size of those transcripts.
    pub total_bytes: u64,
    /// Transcripts skipped for being too large to upload.
    pub skipped_oversize: usize,
}

/// An event fed back from spawned async work into [`WelcomeScreen::apply`].
#[derive(Debug, Clone)]
pub enum WelcomeEvent {
    /// Scanning finished.
    ScanReady(ScanSummary),
    /// One more transcript finished uploading.
    UploadProgress {
        /// Transcripts uploaded so far.
        uploaded: usize,
        /// Transcripts in this upload.
        total: usize,
        /// Running count of secrets scrubbed before sending.
        redactions: usize,
    },
    /// The reward was scored and settled.
    Claimed {
        /// USD granted.
        awarded_usd: f64,
        /// Power-level label.
        tier: Option<String>,
        /// Per-metric contributions, as `(label, usd)` rows.
        breakdown: Vec<(String, f64)>,
        /// The backend's advertised ceiling.
        max_reward_usd: f64,
        /// True when the reward had already been granted before this run.
        already_claimed: bool,
    },
    /// The flow failed; the message is shown and the user can skip out.
    Failed(String),
}

/// Which step the flow is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// The pitch: earn up to $25.
    Intro,
    /// Reading local history (spinner).
    Scanning,
    /// Showing exactly what would be sent, awaiting explicit approval.
    Consent,
    /// Redacting, uploading, and claiming (progress).
    Uploading,
    /// The celebratory result.
    Reveal,
    /// Nothing was found locally, so there is nothing to share.
    Empty,
}

/// The pure welcome-screen state machine.
pub struct WelcomeScreen {
    pub(super) step: Step,
    pub(super) scan: ScanSummary,
    pub(super) uploaded: usize,
    pub(super) upload_total: usize,
    pub(super) redactions: usize,
    pub(super) awarded_usd: f64,
    pub(super) tier: Option<String>,
    pub(super) breakdown: Vec<(String, f64)>,
    pub(super) max_reward_usd: f64,
    pub(super) already_claimed: bool,
    pub(super) error: Option<String>,
    pub(super) frame: usize,
    pub(super) outcome: Option<WelcomeOutcome>,
}

impl WelcomeScreen {
    /// A fresh screen at the intro step, advertising `max_reward_usd`.
    pub fn new(max_reward_usd: f64) -> Self {
        WelcomeScreen {
            step: Step::Intro,
            scan: ScanSummary::default(),
            uploaded: 0,
            upload_total: 0,
            redactions: 0,
            awarded_usd: 0.0,
            tier: None,
            breakdown: Vec::new(),
            max_reward_usd: if max_reward_usd > 0.0 {
                max_reward_usd
            } else {
                DEFAULT_MAX_REWARD_USD
            },
            already_claimed: false,
            error: None,
            frame: 0,
            outcome: None,
        }
    }

    /// The terminal outcome, once reached.
    pub fn outcome(&self) -> Option<WelcomeOutcome> {
        self.outcome.clone()
    }

    /// Advance the spinner and reveal animation (called on the driver tick).
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// The current spinner glyph.
    pub(super) fn spinner(&self) -> &'static str {
        SPINNER[self.frame % SPINNER.len()]
    }
}

impl Default for WelcomeScreen {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_REWARD_USD)
    }
}
