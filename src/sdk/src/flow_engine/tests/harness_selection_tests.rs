//! Tests for which harness and model an `agent` node's dispatch carries.
//!
//! Every case drives a real graph through the real engine into the real agent
//! adapter and asserts on the task frame that came out, because the frame is
//! what a worker actually acts on — a unit test of the resolver alone would pass
//! even if the adapter forgot to put the answer on the wire.
//!
//! Split out of the `tests` module (which owns the shared fixtures) rather
//! than added to it: that file was already at the repository's 500-line
//! ceiling.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::super::caps::{build_capabilities, HostServices};
use super::super::settings::CapabilitySettings;
use super::{agent_graph, empty_resolver, settings, RecordingDispatch};
use crate::hub::TaskRequest;
use crate::protocol::HarnessProvider;

/// Run a one-agent-node graph with `config` under `settings` and return the task
/// frame it dispatched.
async fn frame_for(
    settings: Arc<CapabilitySettings>,
    root: &std::path::Path,
    config: serde_json::Value,
) -> TaskRequest {
    let dispatch = RecordingDispatch::replying("ok");
    let caps = build_capabilities(
        settings,
        HostServices {
            node_progress: None,
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-harness",
    );
    let compiled = tinyflows::compiler::compile(&agent_graph(config)).expect("compiles");
    tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");
    dispatch.requests().remove(0)
}

/// Host settings pinning Claude and a Claude model, which is what a node has to
/// override to prove anything.
fn claude_host(root: &std::path::Path) -> Arc<CapabilitySettings> {
    let mut settings = CapabilitySettings::clone(&settings(root));
    settings.default_provider = Some(HarnessProvider::Claude);
    settings.default_model = Some("claude-opus-4".to_string());
    Arc::new(settings)
}

#[tokio::test]
async fn a_node_that_names_no_harness_inherits_the_host_default() {
    let root = tempfile::tempdir().unwrap();
    let frame = frame_for(
        claude_host(root.path()),
        root.path(),
        json!({ "prompt": "hi" }),
    )
    .await;

    assert_eq!(frame.provider, Some(HarnessProvider::Claude));
    assert_eq!(frame.model.as_deref(), Some("claude-opus-4"));
    assert_eq!(frame.custom_harness, None);
}

#[tokio::test]
async fn a_node_may_choose_its_own_harness() {
    let root = tempfile::tempdir().unwrap();
    let frame = frame_for(
        claude_host(root.path()),
        root.path(),
        json!({ "prompt": "hi", "harness": "codex" }),
    )
    .await;

    assert_eq!(frame.provider, Some(HarnessProvider::Codex));
    // The host's Claude model must not ride along to Codex.
    assert_eq!(frame.model, None);
}

#[tokio::test]
async fn a_node_may_choose_its_own_harness_and_model_together() {
    let root = tempfile::tempdir().unwrap();
    let frame = frame_for(
        claude_host(root.path()),
        root.path(),
        json!({ "prompt": "hi", "harness": "codex", "model": "gpt-5-codex" }),
    )
    .await;

    assert_eq!(frame.provider, Some(HarnessProvider::Codex));
    assert_eq!(frame.model.as_deref(), Some("gpt-5-codex"));
}

#[tokio::test]
async fn a_node_may_pin_only_the_model_and_stay_on_the_inherited_harness() {
    let root = tempfile::tempdir().unwrap();
    let frame = frame_for(
        claude_host(root.path()),
        root.path(),
        json!({ "prompt": "triage this", "model": "claude-haiku-4-5" }),
    )
    .await;

    assert_eq!(frame.provider, Some(HarnessProvider::Claude));
    assert_eq!(frame.model.as_deref(), Some("claude-haiku-4-5"));
}

#[tokio::test]
async fn a_node_may_name_a_custom_harness_preset() {
    let root = tempfile::tempdir().unwrap();
    let frame = frame_for(
        claude_host(root.path()),
        root.path(),
        json!({ "prompt": "hi", "harness": "deepseek-claude" }),
    )
    .await;

    assert_eq!(frame.custom_harness.as_deref(), Some("deepseek-claude"));
    assert_eq!(
        frame.provider, None,
        "a preset replaces the built-in choice"
    );
}

#[tokio::test]
async fn the_workflow_default_reaches_a_node_through_the_run_settings() {
    // What `run_workflow` writes into the settings when a document carries a
    // `defaults` block: from a node's point of view it is simply the layer
    // underneath it.
    let root = tempfile::tempdir().unwrap();
    let mut settings = CapabilitySettings::clone(&settings(root.path()));
    settings.default_custom_harness = Some("team-preset".to_string());
    let frame = frame_for(Arc::new(settings), root.path(), json!({ "prompt": "hi" })).await;

    assert_eq!(frame.custom_harness.as_deref(), Some("team-preset"));
}

#[tokio::test]
async fn a_workflow_agent_node_inherits_the_runs_fleet_depth() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = CapabilitySettings::clone(&settings(root.path()));
    settings.fleet_depth = 2;

    let frame = frame_for(Arc::new(settings), root.path(), json!({ "prompt": "hi" })).await;

    assert_eq!(frame.fleet_depth, 2);
}

#[tokio::test]
async fn an_unusable_harness_fails_the_node_rather_than_running_elsewhere() {
    let root = tempfile::tempdir().unwrap();
    let caps = build_capabilities(
        claude_host(root.path()),
        HostServices {
            node_progress: None,
            dispatch: RecordingDispatch::replying("unused"),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-bad-harness",
    );
    let compiled = tinyflows::compiler::compile(&agent_graph(
        json!({ "prompt": "hi", "harness": "claude code" }),
    ))
    .expect("compiles");

    let err = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect_err("an unreadable harness must fail the node");
    let message = err.to_string();
    assert!(message.contains("agent node"), "{message}");
    assert!(message.contains("custom harness id"), "{message}");
}

/// OpenHuman is a harness a node may name, and it resolves to the built-in
/// provider rather than being taken for a custom preset id — which is what a
/// bare "unknown name" fallback would have made of it, silently, leaving the
/// node to fail later on a worker that has no such preset.
#[test]
fn openhuman_is_a_dispatchable_builtin_harness() {
    let selector = super::super::harness_choice::HarnessSelector::parse("openhuman")
        .expect("openhuman is dispatchable");

    assert_eq!(
        selector,
        super::super::harness_choice::HarnessSelector::Builtin {
            provider: crate::protocol::HarnessProvider::Openhuman,
            transport: crate::protocol::HarnessTransport::Cli,
        }
    );
}
