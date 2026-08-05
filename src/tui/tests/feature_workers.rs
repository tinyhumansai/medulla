//! Feature-level tests for the Routing fleet view and header stream-health
//! indicator. These need a `Runtime` that actually surfaces workers and a stream
//! state, so a thin `FleetRuntime` wraps `MockRuntime`, delegating everything but
//! `workers()` / `worker_op()` / `stream_state()`.
//!
//! This is the test-binary root. Shared setup lives in `helpers`; the behaviour
//! groups are split across submodules pulled in via `#[path]` so no single file
//! exceeds the repo's 500-line ceiling. `#[test]` fns inside these included
//! modules are collected and run as part of this binary.

#[path = "feature_workers/helpers.rs"]
mod helpers;

#[path = "feature_workers/list.rs"]
mod list;

#[path = "feature_workers/roles.rs"]
mod roles;

#[path = "feature_workers/routing.rs"]
mod routing;

#[path = "feature_workers/header.rs"]
mod header;

#[path = "feature_workers/fleet.rs"]
mod fleet;

#[path = "feature_workers/workspaces.rs"]
mod workspaces;

#[path = "feature_workers/hooks.rs"]
mod hooks;
