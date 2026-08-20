//! Starting and selecting local sessions.
//!
//! This is the test binary's root, in the canonical directory layout cargo
//! offers for a multi-file integration test: `tests/feature_session_control/main.rs`
//! with ordinary sibling modules. No `feature_session_control.rs` sits beside the
//! directory and no `#[path]` is needed. Shared setup lives in `helpers`; the
//! behaviour groups are split by responsibility so none approaches the repo's
//! 500-line ceiling.

mod helpers;

mod picker;
