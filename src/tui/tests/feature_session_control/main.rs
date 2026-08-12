//! Starting a session the orchestrator will not touch, and moving control of one
//! between the operator and the orchestrator.
//!
//! The load-bearing assertion in most of these is not what is on screen but what
//! `claim_idle` will hand out: the badge is only a report, and a badge that said
//! "unmanaged" over a session dispatch would still reuse is the exact failure
//! worth testing for.
//!
//! This is the test binary's root, in the canonical directory layout cargo
//! offers for a multi-file integration test: `tests/feature_session_control/main.rs`
//! with ordinary sibling modules. No `feature_session_control.rs` sits beside the
//! directory and no `#[path]` is needed. Shared setup lives in `helpers`; the
//! behaviour groups are split by responsibility so none approaches the repo's
//! 500-line ceiling.

mod helpers;

mod control;
mod picker;
