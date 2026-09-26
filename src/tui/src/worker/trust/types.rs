//! Data types for the `trust` module.
#[allow(unused_imports)]
use super::*;
/// What ensuring trust did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustOutcome {
    /// The workspace (or an ancestor) was already trusted; nothing was written.
    AlreadyTrusted,
    /// Trust was granted, in the config file at this path.
    Granted(PathBuf),
    /// Nothing was done, for this reason.
    Skipped(String),
}
