//! Uploading a scanned history and claiming the reward.
//!
//! This is the orchestration behind the welcome flow's approval step: redact each
//! transcript, upload it, then claim. It lives in the SDK rather than the TUI
//! because none of it is rendering — the caller supplies a progress callback and
//! renders however it likes, which also makes the whole sequence testable against
//! a mock backend instead of a terminal.
//!
//! Callers must not invoke this before the user has consented: it is the point at
//! which data leaves the machine.

use crate::client::{HistoryRewardClaim, MedullaClient, Result};

use super::scan::read_redacted_session;
use super::types::HistorySessionFile;

/// Progress after each transcript is dealt with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareProgress {
    /// Transcripts successfully uploaded so far.
    pub uploaded: usize,
    /// Transcripts in this share.
    pub total: usize,
    /// Running count of secrets scrubbed before sending.
    pub redactions: usize,
}

/// Redacts and uploads every transcript in `files`, then claims the reward.
///
/// `on_progress` is invoked once per transcript, whether it uploaded or not, so a
/// UI can show a moving bar rather than stalling on a skipped file.
///
/// Individual failures are tolerated: a transcript that cannot be read or that
/// the backend rejects is skipped rather than aborting the share. A partial share
/// still earns credit for what landed, which is strictly better for the user than
/// failing outright — and the backend refusing further uploads (because the
/// reward already settled) is reported accurately by the claim.
///
/// Errors only when the final claim itself fails.
pub async fn share_history<F>(
    client: &MedullaClient,
    files: &[HistorySessionFile],
    mut on_progress: F,
) -> Result<HistoryRewardClaim>
where
    F: FnMut(ShareProgress),
{
    let total = files.len();
    let mut uploaded = 0usize;
    let mut redactions = 0usize;

    for file in files {
        // Reading and redacting is blocking IO/CPU; keep it off the reactor.
        let file = file.clone();
        let session = tokio::task::spawn_blocking(move || read_redacted_session(&file))
            .await
            .ok()
            .flatten();

        if let Some(session) = session {
            if client
                .upload_history_session(session.agent.as_str(), session.content)
                .await
                .is_ok()
            {
                uploaded += 1;
                redactions += session.redactions;
            }
        }

        on_progress(ShareProgress {
            uploaded,
            total,
            redactions,
        });
    }

    client.claim_history_reward().await
}
