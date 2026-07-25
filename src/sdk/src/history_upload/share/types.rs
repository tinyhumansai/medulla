//! Data types for the `share` module.
#[allow(unused_imports)]
use super::*;
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
