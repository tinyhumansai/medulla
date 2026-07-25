//! Owner→wrapper control-frame targeting.
//!
//! The wrapper polls its mailbox for [`HarnessControlFrame`]s and injects their
//! `text` into the child. A frame may address a specific session by id (either
//! the wrapper session id or the harness's own session id); an absent id targets
//! the single session this terminal runs. Since this is a single-terminal wrapper
//! (the machine-bus multi-terminal router is a scope cut), matching is a direct
//! id comparison rather than a spool lookup.

use crate::tinyplace::HarnessControlFrame;

/// Whether `frame` addresses this wrapper's session. A frame with no `session_id`
/// always matches (there is only one session here); a frame naming an id matches
/// only its wrapper or harness id.
pub fn frame_targets_session(
    frame: &HarnessControlFrame,
    wrapper_session_id: &str,
    harness_session_id: &str,
) -> bool {
    match frame.session_id.as_deref() {
        None => true,
        Some(id) => id == wrapper_session_id || id == harness_session_id,
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
