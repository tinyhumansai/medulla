//! Data types for the `harness_pty` module.
#[allow(unused_imports)]
use super::*;
/// Restores cooked mode on drop, so a panic between here and the wrapper's
/// teardown still leaves the operator with a usable terminal.
pub(super) struct RawGuard;
