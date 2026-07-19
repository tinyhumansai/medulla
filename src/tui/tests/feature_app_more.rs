//! Feature-level tests (batch 2): more whole-flow `App` coverage — slash-command
//! variants, Agents steering (X/A), the inline prompt overlay, Context navigation,
//! mouse routing, and the remaining composer edits. Driven via synthetic crossterm
//! events against a `MockRuntime`, asserting on observable state and rendered
//! `TestBackend` buffers.
//!
//! This is the test-binary root. Shared setup lives in `helpers`; the behaviour
//! groups are split across submodules pulled in via `#[path]` so no single file
//! exceeds the repo's 500-line ceiling. `#[test]` fns inside these included
//! modules are collected and run as part of this binary.

#[path = "feature_app_more/helpers.rs"]
mod helpers;

#[path = "feature_app_more/commands.rs"]
mod commands;

#[path = "feature_app_more/agents.rs"]
mod agents;

#[path = "feature_app_more/views.rs"]
mod views;

#[path = "feature_app_more/chat_overview.rs"]
mod chat_overview;
