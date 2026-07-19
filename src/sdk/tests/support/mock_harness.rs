//! Mock coding-agent CLI harness for daemon e2e tests.
//!
//! This is a richer successor to `fake_provider`'s ad-hoc shell snippets: a
//! single [`MockCli`] builder renders a `/bin/sh` script that emits the *exact*
//! streaming-JSONL shapes the daemon mappers ([`medulla::daemon::mappers`])
//! parse for each provider — claude `-p --output-format stream-json`, codex
//! `exec --json`, and opencode `run --format json`. The daemon's real spawn path
//! ([`medulla::daemon::providers::run_provider_task`]) runs them through the
//! `TINYPLACE_*_BIN` env overrides.
//!
//! A mock is a sequence of high-level [`Step`]s (thinking, agent messages, tool
//! call/result pairs, provider errors, garbage lines) plus a terminal behavior
//! (clean exit, non-zero exit with a stderr tail, or hang-until-killed for the
//! idle watchdog). Each step is lowered to the provider-specific record shape, so
//! the same scenario can be replayed against any of the three providers.

//! # Module layout
//!
//! This entry file is a thin root that wires three sibling submodules (each
//! `#[path]`-included so integration tests can keep pointing `#[path =
//! "support/mock_harness.rs"]` at this file):
//! - `types`   — [`MockProvider`], [`Step`], [`Terminal`], [`SessionLogSpec`],
//!   and the [`MockCli`] builder surface.
//! - `script`  — the `impl MockCli` block that renders the `/bin/sh` script.
//! - `helpers` — canned scenarios, record builders, and [`MockDir`] install glue.

#![allow(dead_code, unused_imports)]

#[path = "mock_harness_helpers.rs"]
mod helpers;
#[path = "mock_harness_script.rs"]
mod script;
#[path = "mock_harness_types.rs"]
mod types;

pub use helpers::*;
pub use types::*;
