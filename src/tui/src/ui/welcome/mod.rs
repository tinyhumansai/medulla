//! The first-run welcome screen: share your coding-agent history, earn credit.
//!
//! Mirrors [`crate::ui::onboarding`] in shape — a pure state machine plus an
//! async driver — but answers a different question. Onboarding registers a
//! worker identity; this screen offers new users up to $25 of promotional credit
//! for showing how much of a power user they are, measured from the Claude Code
//! and Codex transcripts already on their machine.
//!
//! Six steps: INTRO (the pitch) → SCANNING (read local metadata) → CONSENT (show
//! exactly what would be sent, and require approval) → UPLOADING (redact, send,
//! claim) → REVEAL (power level and award), with EMPTY standing in when there is
//! no local history to share. Every step can be skipped.
//!
//! Two invariants hold across the module:
//!
//! - **Nothing leaves the machine without consent.** Scanning reads only
//!   metadata, and [`WelcomeCmd::UploadAndClaim`] is emitted from exactly one
//!   place: an explicit Enter on the consent step.
//! - **The backend owns the score.** The client uploads redacted transcripts and
//!   renders what it is told; it never computes or sends a dollar amount.
//!
//! Split by responsibility: [`types`] holds the command/event/outcome enums and
//! the [`WelcomeScreen`] struct; [`state`] the key → command and event → state
//! machine; [`draw`] the ratatui rendering; and [`run`] the terminal-driving loop
//! exposed as [`run_welcome_ui`].

mod draw;
mod run;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use run::{drive_welcome_ui, run_welcome_ui};
pub use types::{
    format_usd, ScanSummary, WelcomeCmd, WelcomeEvent, WelcomeOutcome, WelcomeScreen,
    DEFAULT_MAX_REWARD_USD,
};
