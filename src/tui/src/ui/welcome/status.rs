//! Turning a backgrounded share's events into status-line text.
//!
//! Lives in the library rather than beside the event loop so the rule it
//! encodes — which events settle onboarding — is unit-testable. Getting that
//! wrong silently costs a user credit they earned, so it must not sit in the
//! binary crate where tests cannot reach it.

use std::path::Path;

use super::types::{format_usd, WelcomeEvent};

/// The status line for one backgrounded-share event, or `None` when the event is
/// not worth interrupting the user for.
///
/// Records onboarding as complete when — and only when — a claim settles. The
/// user consented before the upload began, but consent alone must not burn the
/// offer: if the transfer or the claim fails they keep it, and the flow is
/// offered again next launch. A claim that awarded nothing still settles, since
/// the offer *was* answered; it just scored zero.
///
/// `persist` is the side effect that records onboarding, injected so tests can
/// observe it without touching a real config file. It is called at most once,
/// on a [`WelcomeEvent::Claimed`].
pub fn share_status(
    ev: &WelcomeEvent,
    persist: impl FnOnce() -> Result<(), String>,
) -> Option<String> {
    match ev {
        WelcomeEvent::UploadProgress {
            uploaded, total, ..
        } => Some(format!("sharing history · {uploaded}/{total} transcripts")),
        WelcomeEvent::Claimed { awarded_usd, .. } => {
            let awarded = if *awarded_usd > 0.0 {
                format!(
                    "{} in free credits added to your balance",
                    format_usd(*awarded_usd)
                )
            } else {
                "history shared — thanks!".to_string()
            };
            match persist() {
                Ok(()) => Some(awarded),
                Err(e) => Some(format!("{awarded} (could not save onboarding state: {e})")),
            }
        }
        // Deliberately does not persist: the offer stays open so a transient
        // failure does not cost the user their credit.
        WelcomeEvent::Failed(msg) => Some(format!("history share failed: {msg}")),
        // The scan finished long before this channel was handed over; nothing to
        // say about it now.
        WelcomeEvent::ScanReady(_) => None,
    }
}

/// Whether this event ends the share, after which the channel says nothing more.
pub fn settles_share(ev: &WelcomeEvent) -> bool {
    matches!(ev, WelcomeEvent::Claimed { .. } | WelcomeEvent::Failed(_))
}

/// Record onboarding as complete at `path`, as a [`share_status`] `persist`.
pub fn persist_onboarding(path: &Path) -> Result<(), String> {
    medulla::config::persist_welcome_completed(path, true).map_err(|e| e.to_string())
}
