//! Workflow dispatch integration with daemon-wide resource limits.

use std::sync::Arc;

use crate::daemon::providers::{RunTaskFn, RunTaskResult};
use crate::daemon::task_loop::workflow::RuntimeDispatch;
use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::hub::TaskRequest;

use super::{base_config, blocking_runner, recording_send};

#[tokio::test]
async fn workflow_dispatch_waits_for_a_daemon_harness_slot() {
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut config = base_config();
    config.concurrency = 1;
    let (send, _) = recording_send();
    let runtime =
        crate::daemon::DaemonRuntime::new(config, blocking_runner(ready_tx, gate.clone()), send);
    let occupied = runtime
        .inner
        .slots
        .acquire()
        .await
        .expect("semaphore stays open");

    let dispatch = RuntimeDispatch::new(runtime.clone(), "peer".into());
    let task = tokio::spawn(async move {
        dispatch
            .dispatch(TaskRequest {
                transport: None,
                task_id: "review-1".into(),
                abort_id: "run-1".into(),
                cycle_id: None,
                instruction: "review the failed workflow".into(),
                worker_address: "claude".into(),
                provider: None,
                custom_harness: None,
                model: None,
                tool_mode: Some("propose:sweep".into()),
                workflow: None,
                workflow_fingerprint: None,
                workflow_inputs: Default::default(),
                conversation: None,
                fleet_depth: 0,
            })
            .await
    });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), ready_rx.recv())
            .await
            .is_err(),
        "the review must not start while the only slot is occupied"
    );
    drop(occupied);
    tokio::time::timeout(std::time::Duration::from_secs(1), ready_rx.recv())
        .await
        .expect("dispatch starts after the slot is released")
        .expect("runner reports readiness");
    gate.notify_waiters();
    task.await
        .expect("dispatch task joins")
        .expect("dispatch runs");
}

#[tokio::test]
async fn workflow_dispatch_preserves_the_callers_fleet_depth() {
    let (captured_tx, mut captured_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner: RunTaskFn = Arc::new(move |options| {
        let captured_tx = captured_tx.clone();
        Box::pin(async move {
            captured_tx.send(options.env.clone()).unwrap();
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(base_config(), runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    dispatch
        .dispatch(TaskRequest {
            transport: None,
            task_id: "child-1".into(),
            abort_id: "child-1".into(),
            cycle_id: None,
            instruction: "do the child work".into(),
            worker_address: "claude".into(),
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: Some("execute".into()),
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 2,
        })
        .await
        .unwrap();

    let env = captured_rx.recv().await.unwrap();
    assert_eq!(
        env.get(crate::control_socket::FLEET_DEPTH_ENV)
            .map(String::as_str),
        Some("2")
    );
}

/// A workflow node naming a preset must get the whole preset, not just its
/// model.
///
/// The regression this pins: a node resolved the preset's *model* and nothing
/// else, so a routed slug was handed to the base harness's own default account
/// with no endpoint and no credential. The harness answered with a provider-side
/// "model not supported" and exited, which reads as a workflow bug rather than
/// the provisioning gap it is. Mirrors
/// `custom_harness_tests::named_custom_harness_scopes_model_router_and_claude_tiers_to_one_run`,
/// which pins the same guarantee for an ordinary task frame — the two dispatch
/// paths reach the same spawn seam and must provision it identically.
#[tokio::test]
async fn workflow_dispatch_gives_a_named_preset_its_router_model_and_env() {
    let (captured_tx, mut captured_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner: RunTaskFn = Arc::new(move |options: crate::daemon::providers::RunTaskOptions| {
        let captured_tx = captured_tx.clone();
        Box::pin(async move {
            captured_tx
                .send((
                    options.provider,
                    options.model.clone(),
                    options
                        .router
                        .as_ref()
                        .and_then(|router| router.base_url_for("claude"))
                        .map(str::to_string),
                    options.env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").cloned(),
                ))
                .unwrap();
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let mut config = base_config();
    config
        .env
        .insert("OPENROUTER_API_KEY".into(), "secret".into());
    config.custom_harnesses = vec![crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Claude | claude | deepseek/pro | deepseek/fast | this-device",
    )
    .unwrap()];
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(config, runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    dispatch
        .dispatch(TaskRequest {
            custom_harness: Some("deepseek".into()),
            // Deliberately a different provider than the preset's base harness:
            // the preset describes one whole harness, so it must outrank an
            // address hint rather than pairing its model with another binary.
            worker_address: "codex".into(),
            transport: None,
            task_id: "node-1".into(),
            abort_id: "node-1".into(),
            cycle_id: None,
            instruction: "run the node".into(),
            provider: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        })
        .await
        .unwrap();

    let (provider, model, endpoint, opus_tier) = captured_rx.recv().await.unwrap();
    assert_eq!(provider, crate::protocol::HarnessProvider::Claude);
    assert_eq!(model.as_deref(), Some("deepseek/pro"));
    assert_eq!(
        endpoint.as_deref(),
        Some(crate::config::OPENROUTER_ANTHROPIC_URL),
        "the preset's endpoint must reach the spawn seam, not just its model"
    );
    assert_eq!(opus_tier.as_deref(), Some("deepseek/pro"));
}

/// A `codexOverrides` preset must publish its Codex knobs into the node's
/// environment.
///
/// The environment is where `crate::codex_overrides` reads them back at the
/// spawn seam, so a node whose env lacks them spawns a bare `codex -m <slug>`:
/// no provider block, no API-key auth preference, no catalog entry. That is the
/// exact shape of the failure this test exists to prevent.
#[tokio::test]
async fn workflow_dispatch_publishes_codex_override_knobs_for_a_preset() {
    let (captured_tx, mut captured_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner: RunTaskFn = Arc::new(move |options: crate::daemon::providers::RunTaskOptions| {
        let captured_tx = captured_tx.clone();
        Box::pin(async move {
            captured_tx.send(options.env.clone()).unwrap();
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let mut config = base_config();
    config
        .providers
        .push(crate::protocol::HarnessProvider::Codex);
    config
        .env
        .insert("OPENROUTER_API_KEY".into(), "secret".into());
    let mut preset = crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek-codex | DeepSeek via Codex | codex | deepseek/flash | | this-device",
    )
    .unwrap();
    preset.codex_overrides = true;
    preset.reasoning_effort = Some("high".into());
    config.custom_harnesses = vec![preset];
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(config, runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    dispatch
        .dispatch(TaskRequest {
            custom_harness: Some("deepseek-codex".into()),
            worker_address: "codex".into(),
            transport: None,
            task_id: "node-1".into(),
            abort_id: "node-1".into(),
            cycle_id: None,
            instruction: "run the node".into(),
            provider: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        })
        .await
        .unwrap();

    let env = captured_rx.recv().await.unwrap();
    assert_eq!(
        env.get(crate::codex_overrides::OVERRIDES_ENV)
            .map(String::as_str),
        Some("1"),
        "without this the spawn seam emits no Codex provider block at all"
    );
    assert_eq!(
        env.get(crate::codex_overrides::EFFORT_ENV)
            .map(String::as_str),
        Some("high")
    );
}

/// A node naming a preset this host has not configured is refused.
///
/// Refused rather than quietly downgraded to the default harness: the node asked
/// for specific credentials and a specific model, and running it on whatever
/// this machine happens to default to spends the wrong account without saying
/// so. `handle_task` refuses the same case for an ordinary task frame.
#[tokio::test]
async fn workflow_dispatch_refuses_an_unconfigured_preset() {
    let runner: RunTaskFn = Arc::new(move |options: crate::daemon::providers::RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(base_config(), runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    let error = dispatch
        .dispatch(TaskRequest {
            custom_harness: Some("not-here".into()),
            worker_address: "claude".into(),
            transport: None,
            task_id: "node-1".into(),
            abort_id: "node-1".into(),
            cycle_id: None,
            instruction: "run the node".into(),
            provider: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        })
        .await
        .expect_err("an unconfigured preset must not silently run elsewhere");
    assert!(
        format!("{error}").contains("not-here"),
        "the error must name the preset that is missing, got: {error}"
    );
}

/// A preset cannot safely run when its base harness is unavailable.
///
/// Unlike an address hint, a preset supplies harness-specific endpoint and
/// credential settings. Falling back to the daemon default would run the
/// preset's model and router on a different binary, so this must fail with the
/// same unavailable-provider error as an ordinary task frame.
#[tokio::test]
async fn workflow_dispatch_refuses_a_preset_with_an_unavailable_base_provider() {
    let runner: RunTaskFn = Arc::new(move |options: crate::daemon::providers::RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let mut config = base_config();
    config.providers = vec![crate::protocol::HarnessProvider::Codex];
    config.default_provider = crate::protocol::HarnessProvider::Codex;
    config.custom_harnesses = vec![crate::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek via Claude | claude | deepseek/pro | deepseek/fast | this-device",
    )
    .unwrap()];
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(config, runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    let error = dispatch
        .dispatch(TaskRequest {
            custom_harness: Some("deepseek".into()),
            worker_address: "codex".into(),
            transport: None,
            task_id: "node-1".into(),
            abort_id: "node-1".into(),
            cycle_id: None,
            instruction: "run the node".into(),
            provider: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        })
        .await
        .expect_err("a preset must not fall back to a different provider");

    assert_eq!(
        format!("{error}"),
        "worker error: no available provider for requested \"claude\"; daemon offers: codex"
    );
}

/// Every harness a workflow node opens is served no Medulla tools.
///
/// The environment is the carrier the four launch seams share, so this asserts
/// on what actually reaches them: the withholding marker set, and — the half
/// that matters for a run started *by* a harness session — the inherited grant
/// cleared rather than merely unused. A node that kept its parent's capability
/// could exchange its way back to the `fleet_*` verbs and dispatch into the
/// very worker pool its own run is competing for.
#[tokio::test]
async fn workflow_dispatch_withholds_medulla_tools_from_every_node() {
    let (captured_tx, mut captured_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner: RunTaskFn = Arc::new(move |options| {
        let captured_tx = captured_tx.clone();
        Box::pin(async move {
            captured_tx.send(options.env.clone()).unwrap();
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let (send, _) = recording_send();
    let mut config = base_config();
    // What a run started from a harness session inherits.
    config.env.insert(
        crate::control_socket::MCP_GRANT_ENV.to_string(),
        "inherited-token".to_string(),
    );
    let runtime = crate::daemon::DaemonRuntime::new(config, runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    dispatch
        .dispatch(TaskRequest {
            transport: None,
            task_id: "child-tools".into(),
            abort_id: "child-tools".into(),
            cycle_id: None,
            instruction: "do the child work".into(),
            worker_address: "claude".into(),
            provider: None,
            custom_harness: None,
            model: None,
            // Even asked for explicitly, a node gets none.
            tool_mode: Some("full".into()),
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        })
        .await
        .unwrap();

    let env = captured_rx.recv().await.unwrap();
    assert!(
        crate::harness_tools::withheld(&env),
        "a workflow node's harness must be marked tool-less: {env:?}"
    );
    assert!(
        !env.contains_key(crate::control_socket::MCP_GRANT_ENV),
        "the inherited grant must be cleared, not merely unused: {env:?}"
    );
    assert!(
        !env.contains_key(crate::mcp::TOOL_MODE_ENV),
        "a requested tool mode must not survive the withholding: {env:?}"
    );
}

/// A node that names the embedded core reaches it even though no host ever
/// lists OpenHuman among its providers.
///
/// `config.providers` is the set of coding CLIs found on PATH, and OpenHuman
/// has no binary to find — so the ordinary "anything this worker does not
/// offer falls back to the default" rule would silently send a node that asked
/// for the operator's own core to a coding CLI instead.
#[tokio::test]
async fn a_node_naming_the_embedded_core_is_not_fallen_back_to_a_cli() {
    let (captured_tx, mut captured_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner: RunTaskFn = Arc::new(move |options| {
        let captured_tx = captured_tx.clone();
        Box::pin(async move {
            captured_tx.send(options.provider).unwrap();
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(base_config(), runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    dispatch
        .dispatch(TaskRequest {
            transport: None,
            task_id: "child-openhuman".into(),
            abort_id: "child-openhuman".into(),
            cycle_id: None,
            instruction: "ask my own core".into(),
            worker_address: "openhuman".into(),
            provider: Some(crate::protocol::HarnessProvider::Openhuman),
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        })
        .await
        .unwrap();

    assert_eq!(
        captured_rx.recv().await.unwrap(),
        crate::protocol::HarnessProvider::Openhuman,
        "a node that named the embedded core must not be redirected to a CLI"
    );
}
