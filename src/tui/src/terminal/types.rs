//! Data types for the `terminal` module.
#[allow(unused_imports)]
use super::*;
/// RAII guard that owns the terminal's raw-mode/alt-screen/kitty state and
/// restores it on drop. Construct with [`TermGuard::setup`].
pub(crate) struct TermGuard {
    pub(super) alt_screen: bool,
    pub(super) kitty: bool,
}
