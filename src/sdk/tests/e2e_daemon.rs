//! (Unix-only: exercises Unix-domain-socket cores and/or spawned `/bin/sh` mock scripts.)
#![cfg(unix)]

//! End-to-end tests for the daemon [`DaemonRuntime`] with fake providers and an
//! in-memory transport (an injected recording `send` closure).
//!
//! Two execution modes are exercised:
//! - injected `run_task` (deterministic, for capacity/duplicate/drain), and
//! - the REAL spawn path ([`run_provider_task`]) driving fake provider CLIs
//!   (shell scripts emitting realistic JSONL) via `TINYPLACE_*_BIN` overrides.
//!
//! Nothing touches the network or tiny.place, and no real claude/codex/opencode
//! binary is required.
//!
//! The suite is split across a sibling `e2e_daemon/` directory so no file
//! exceeds the repo 500-line ceiling: shared fixtures in [`helpers`], the
//! real-spawn scenarios in [`spawn_path`], and the injected-runner scenarios in
//! [`injected`]. `#[test]` fns in the included modules are collected and run as
//! part of this binary.

mod support;

#[path = "e2e_daemon/helpers.rs"]
mod helpers;

#[path = "e2e_daemon/spawn_path.rs"]
mod spawn_path;

#[path = "e2e_daemon/injected.rs"]
mod injected;
