//! Starting a session the orchestrator will not touch, and moving control of one
//! between the operator and the orchestrator.
//!
//! The load-bearing assertion in most of these is not what is on screen but what
//! `claim_idle` will hand out: the badge is only a report, and a badge that said
//! "unmanaged" over a session dispatch would still reuse is the exact failure
//! worth testing for.
//!
//! This is the test-binary root. Shared setup lives in `helpers`; the behaviour
//! groups are split across submodules pulled in via `#[path]` so no single file
//! exceeds the repo's 500-line ceiling. `#[test]` fns inside these included
//! modules are collected and run as part of this binary.

#[path = "feature_session_control/helpers.rs"]
mod helpers;

#[path = "feature_session_control/picker.rs"]
mod picker;

#[path = "feature_session_control/control.rs"]
mod control;

#[path = "feature_session_control/spawn.rs"]
mod spawn;
