//! Sharing local coding-agent history to earn onboarding credit.
//!
//! The welcome flow offers new users up to $25 of promotional credit for showing
//! how much of a power user they are. This module owns the client half of that
//! exchange: find the transcripts Claude Code and Codex wrote on this machine,
//! scrub secrets out of them, and hand them to the caller to upload. Scoring is
//! deliberately *not* here — the backend derives the award from what it receives,
//! so the client cannot inflate its own payout.
//!
//! Split by responsibility: [`types`] holds the data model, [`scan`] locates and
//! reads transcripts, and [`redact`] removes secrets before anything leaves the
//! machine. Public items are re-exported so callers use
//! `medulla::history_upload::*`.
//!
//! ```no_run
//! # use std::collections::HashMap;
//! let env: HashMap<String, String> = std::env::vars().collect();
//! let scan = medulla::history_upload::scan_local_history(&env);
//! // Show `scan.tallies()` on a consent screen, then upload only on approval.
//! for file in &scan.files {
//!     if let Some(session) = medulla::history_upload::read_redacted_session(file) {
//!         // client.upload_history_session(&session).await?;
//!         let _ = session;
//!     }
//! }
//! ```

mod redact;
mod scan;
mod types;

#[cfg(test)]
mod tests;

pub use redact::{redact_text, REDACTED};
pub use scan::{read_redacted_session, scan_local_history, MAX_SESSION_BYTES, MAX_UPLOAD_SESSIONS};
pub use types::{AgentTally, HistoryScan, HistorySessionFile, RedactedSession};
