//! (Unix-only: exercises Unix-domain-socket cores and/or spawned `/bin/sh` mock scripts.)
#![cfg(unix)]

//! Additional end-to-end tests for the core-js runtime path, complementing
//! `e2e_core.rs`. Uses the configurable [`mock_core`] stub to reach the branches
//! the base scenarios skip: existing-thread adoption, snapshot seeding, the full
//! steering / fleet RPC matrix, error surfacing, the `resync.required` snapshot
//! carry, the stall / connection-drop transitions, and malformed / oversize frames.
//!
//! The suite is split across a sibling `e2e_core_ext/` directory so no file
//! exceeds the repo 500-line ceiling: shared fixtures in [`helpers`], the
//! runtime lifecycle scenarios in [`lifecycle`], the resilience / guard
//! scenarios in [`resilience`], and the CoreClient transport branches in
//! [`transport`]. `#[test]` fns in the included modules are collected and run as
//! part of this binary.

#[path = "../../sdk/tests/support/mod.rs"]
mod support;

#[path = "../../sdk/tests/support/mock_core.rs"]
mod mock_core;

#[path = "e2e_core_ext/helpers.rs"]
mod helpers;

#[path = "e2e_core_ext/lifecycle.rs"]
mod lifecycle;

#[path = "e2e_core_ext/resilience.rs"]
mod resilience;

#[path = "e2e_core_ext/transport.rs"]
mod transport;
