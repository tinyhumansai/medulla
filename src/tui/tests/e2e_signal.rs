//! End-to-end Signal transport suite: the REAL vendored `tinyplace` SDK (live
//! X3DH + double-ratchet crypto) talking to a MOCK tiny.place Signal server
//! ([`mock_signal_server`]). Only the transport server is mocked — every byte of
//! encryption runs in the SDK on both ends.
//!
//! See `tests/support/mock_signal_server.rs` for the endpoint spec, state model,
//! fault-injection controls, and the scenario matrix these tests realize.
//!
//! This file is the test-binary root. Cargo compiles files directly in `tests/`
//! as separate binaries, so the behavior groups live in the `e2e_signal/`
//! subdirectory (which cargo does NOT auto-compile) and are pulled back into this
//! binary via the `#[path]` module declarations below. `#[test]` functions in the
//! included modules are collected and run as part of this binary. Shared fixtures
//! live in `e2e_signal/helpers.rs` and are referenced by the groups via
//! `use crate::helpers::*;`.

// Vendored test support (shared with the SDK crate's suites).
#[path = "../../sdk/tests/support/mock_harness.rs"]
mod mock_harness;
#[path = "../../sdk/tests/support/mock_signal_server.rs"]
mod mock_signal_server;
#[path = "../../sdk/tests/support/mod.rs"]
mod support;

// Shared fixtures + behavior groups for this suite.
#[path = "e2e_signal/fault_matrix.rs"]
mod fault_matrix;
#[path = "e2e_signal/fold.rs"]
mod fold;
#[path = "e2e_signal/helpers.rs"]
mod helpers;
#[path = "e2e_signal/registration.rs"]
mod registration;
#[path = "e2e_signal/task_chain.rs"]
mod task_chain;
#[path = "e2e_signal/wrapper.rs"]
mod wrapper;
