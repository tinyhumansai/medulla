//! Data types for the `run` module.
#[allow(unused_imports)]
use super::*;
/// A minimal raw-mode + alt-screen guard for the pre-run onboarding loop. Unlike
/// the main TUI it needs no mouse capture or kitty flags — it is keyboard-only.
pub(super) struct OnboardTermGuard {
    pub(super) active: bool,
}
