//! Data types for the `envelope` module.
#[allow(unused_imports)]
use super::*;
/// The stable per-run facts an envelope carries, plus the advancing `seq`.
pub struct EnvelopeBuilder {
    pub(super) wrapper_session_id: String,
    pub(super) harness_session_id: String,
    pub(super) cwd: String,
    pub(super) provider: String,
    pub(super) command: String,
    pub(super) argv: Vec<String>,
    pub(super) source_path: String,
    pub(super) seq: i64,
}
