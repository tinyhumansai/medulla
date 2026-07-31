//! `medulla workflow` — authoring, inspecting, and running workflows.
//!
//! This is the surface a coding harness reaches for. Medulla drives Claude Code
//! and Codex over ACP as a *client*, so it cannot hand them tools directly; what
//! it can do is be a well-behaved command they already know how to call. Every
//! verb therefore prints JSON on success, reads bulk input from stdin, and puts
//! errors on stderr with a non-zero exit — the shape a model can act on without
//! being taught anything.
//!
//! The operations themselves live in the SDK
//! ([`medulla::workflows::ops`]), so this command and the MCP server expose the
//! same behaviour rather than two implementations that drift.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use medulla::daemon::embedded::EmbeddedDaemonOptions;
use medulla::flow_engine::{folding_sink, CapabilitySettings, HostServices};
use medulla::workflows::authoring::GraphHandle;
use medulla::workflows::{
    ops, resume_workflow, run_workflow, LocalWorkflowHost, RunContext, StoreWorkflowResolver,
    WorkflowStore, LOCAL_WORKER_ADDRESS,
};
use medulla_tui::cli::{parse_workflow_args, WorkflowAction, WorkflowArgs};
use serde_json::{json, Value};

/// Run `medulla workflow <action>`.
///
/// # Errors
///
/// Returns an error when config cannot be loaded, when an operation fails, or
/// when a verb that reads stdin is given nothing.
pub(crate) async fn run_workflow_cmd(args: &[String]) -> anyhow::Result<()> {
    let parsed = parse_workflow_args(args);
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = ops::discover_store(&env, &cwd);

    // The MCP server owns stdout for the whole session, so it returns straight
    // from here rather than falling through to the JSON print below.
    if matches!(parsed.action, WorkflowAction::Mcp) {
        return medulla::workflows::mcp::serve_stdio(&env, &cwd)
            .await
            .map_err(anyhow::Error::from);
    }

    let output = match &parsed.action {
        WorkflowAction::List => ops::list(&store)?,
        WorkflowAction::Get(id) => ops::get(&store, id)?,
        WorkflowAction::Create(id) => ops::create(&store, &read_stdin("a workflow document")?, id)?,
        WorkflowAction::Delete(id) => ops::delete(&store, id)?,
        WorkflowAction::ApplyOps(id) => ops::apply_ops(&store, id, &read_stdin_json("graph ops")?)?,
        WorkflowAction::PreviewOps(id) => {
            ops::preview_ops(&store, id, &read_stdin_json("graph ops")?)?
        }
        WorkflowAction::Validate(id) => match id {
            Some(id) => ops::validate(&store, &GraphHandle::Saved(id)),
            None => {
                let document = read_stdin("a workflow document")?;
                ops::validate(&store, &GraphHandle::Inline(&document))
            }
        },
        WorkflowAction::DryRun(id) => ops::dry_run(&store, id, trigger_input(&parsed)?).await?,
        WorkflowAction::ListRuns(id) => ops::list_runs(&store, id)?,
        WorkflowAction::GetRun(run_id) => ops::get_run(&store, run_id)?,
        WorkflowAction::Cancel(run_id) => ops::cancel_run(run_id),
        WorkflowAction::Catalog(kind) => ops::catalog(kind.as_deref())?,
        WorkflowAction::Run(id) => execute(&parsed, &store, &env, &cwd, id).await?,
        WorkflowAction::Resume(run_id) => resume(&parsed, &store, &env, &cwd, run_id).await?,
        // Reading and writing share a verb because "what is this pinned to"
        // and "pin it to that" are the same question asked twice, and a
        // separate `show-defaults` would be a verb nobody remembers.
        WorkflowAction::Defaults(id) => match (&parsed.harness, &parsed.model) {
            (None, None) => json!({ "defaults": ops::get(&store, id)?["defaults"] }),
            (harness, model) => {
                ops::set_defaults(&store, id, harness.as_deref(), model.as_deref())?
            }
        },
        WorkflowAction::Notes(id) => ops::notes(&store, id)?,
        WorkflowAction::AddNote(id) => ops::add_note(
            &store,
            id,
            parsed.kind.as_deref().unwrap_or("observation"),
            parsed.text.as_deref().unwrap_or_default(),
            parsed.run_id.clone().into_iter().collect(),
            // Typed by a person at a terminal. Pinned as a result, so
            // automation writing observations cannot evict it.
            medulla::workflows::NoteSource::Operator,
            // An operator superseding a note does it by id, and a key press
            // cannot carry one; `--supersedes` is the CLI's way to say it.
            parsed.supersedes.clone(),
        )?,
        WorkflowAction::Proposals(id) => ops::proposals(&store, id)?,
        WorkflowAction::Accept(proposal_id) => ops::accept_proposal(&store, proposal_id)?,
        WorkflowAction::Reject(proposal_id) => ops::reject_proposal(
            &store,
            proposal_id,
            parsed.reason.as_deref().unwrap_or_default(),
        )?,
        WorkflowAction::Evolve(id) => {
            if let Some(path) = parsed.config.as_deref() {
                std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
            }
            let config = load_workflows_config(&parsed, &env, &cwd)?;
            ops::evolve(&store, &config, &cwd, id, parsed.run_id.as_deref()).await?
        }
        WorkflowAction::Mcp => unreachable!("handled above, before stdout is claimed"),
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Run a workflow for real, against the coding CLIs installed on this machine.
///
/// The loopback host is started per invocation and torn down with it: a CLI run
/// is a one-shot, and leaving a daemon behind would surprise the next command.
async fn execute(
    parsed: &WorkflowArgs,
    store: &Arc<dyn WorkflowStore>,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
    id: &str,
) -> anyhow::Result<Value> {
    let (context, run_id) = local_context(parsed, store, env, cwd)?;
    let record = run_workflow(context, id, &run_id, trigger_input(parsed)?).await?;
    Ok(serde_json::to_value(record)?)
}

/// Release approval gates and continue a paused run.
async fn resume(
    parsed: &WorkflowArgs,
    store: &Arc<dyn WorkflowStore>,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
    run_id: &str,
) -> anyhow::Result<Value> {
    if parsed.approve.is_empty() && parsed.reject.is_empty() {
        anyhow::bail!(
            "resume: name at least one gate with --approve <node-id> or --reject <node-id> \
             (medulla workflow get-run {run_id} lists what it is waiting on)"
        );
    }
    let (context, _) = local_context(parsed, store, env, cwd)?;
    let record = resume_workflow(
        context,
        run_id,
        parsed.approve.clone(),
        parsed.reject.clone(),
    )
    .await?;
    Ok(serde_json::to_value(record)?)
}

/// Build the run context for a local execution, and the id the run will carry.
fn local_context(
    parsed: &WorkflowArgs,
    store: &Arc<dyn WorkflowStore>,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
) -> anyhow::Result<(RunContext, String)> {
    // Recorded before `LocalWorkflowHost::start` below, which spawns an
    // embedded daemon that may in turn spawn ACP harness subprocesses — those
    // read this env var (see `medulla::config::CONFIG_PATH_ENV`) to resolve
    // the same `--config` this command was given, rather than rediscovering a
    // possibly different one from their own `cwd`.
    if let Some(path) = parsed.config.as_deref() {
        std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
    }
    let loaded = medulla::config::load_config(parsed.config.as_deref(), env, cwd)?;
    let home = medulla::home::medulla_home(env);
    let mut settings = CapabilitySettings::from_config(&loaded.config.workflows, &home);
    // A `medulla:shell` step runs where the command was invoked, matching what
    // an operator running it by hand would expect.
    settings.workspace = cwd.to_string_lossy().to_string();
    // Nodes that name no worker go to the loopback host this command starts,
    // unless the operator pinned a different default.
    if settings.default_worker_address.trim().is_empty() {
        settings.default_worker_address = LOCAL_WORKER_ADDRESS.to_string();
    }

    // A workflow's `agent` step may name a custom harness preset (see
    // `flow_engine::harness_choice`); the embedded daemon this command starts
    // must know that preset exists, or it rejects the step as not configured
    // on this host even though the operator configured it right here. Filtered
    // to this device's own `[host]` id, matching how the interactive TUI
    // advertises presets to its primary host in `app_loop.rs`.
    let custom_harnesses = local_custom_harnesses(&loaded);
    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        model: (!loaded.config.workflows.default_model.is_empty())
            .then(|| loaded.config.workflows.default_model.clone()),
        default_provider: loaded.config.workflows.default_provider,
        custom_harnesses,
        ..Default::default()
    })
    .map_err(anyhow::Error::msg)?;
    let dispatch = host.dispatch();
    // The host owns the embedded daemon's drain loop; leaking it keeps the
    // worker alive for the whole run, and the process exits straight after.
    std::mem::forget(host);

    let run_id = parsed
        .run_id
        .clone()
        .unwrap_or_else(|| format!("run-{}", uuid::Uuid::new_v4()));

    // A folding sink rather than a null one: the fold is cheap, and it keeps
    // the door open for this command to print progress without re-wiring.
    let (sink, _fold) = folding_sink();

    Ok((
        RunContext {
            store: store.clone(),
            settings: Arc::new(settings),
            services: HostServices {
                dispatch,
                resolver: Arc::new(StoreWorkflowResolver::new(store.clone())),
                http_credentials: HashMap::new(),
            },
            sink,
        },
        run_id,
    ))
}

/// The trigger payload for a run, from `--input` or the empty object.
fn trigger_input(parsed: &WorkflowArgs) -> anyhow::Result<Value> {
    match &parsed.input {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|err| anyhow::anyhow!("--input is not valid JSON: {err}")),
        None => Ok(json!({})),
    }
}

/// Read stdin whole, failing with a message naming what was expected.
fn read_stdin(what: &str) -> anyhow::Result<String> {
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    if buffer.trim().is_empty() {
        anyhow::bail!("expected {what} on stdin");
    }
    Ok(buffer)
}

/// Read stdin whole and parse it as JSON.
fn read_stdin_json(what: &str) -> anyhow::Result<Value> {
    let body = read_stdin(what)?;
    serde_json::from_str(&body).map_err(|err| anyhow::anyhow!("{what}: invalid JSON: {err}"))
}

/// This machine's own custom-harness presets, for the one-shot embedded daemon
/// this command starts.
///
/// Filtered to this device's `[host]` id, the same filter
/// `local_host::options_from_config_with_custom` applies for the interactive
/// TUI's primary host — a preset for another fleet machine is not this
/// machine's to advertise or run. A load failure (a malformed presets file)
/// is not fatal here: the run proceeds without custom harnesses rather than
/// refusing to run a workflow that may not need one at all.
fn local_custom_harnesses(
    loaded: &medulla::config::LoadedConfig,
) -> Vec<medulla::config::CustomHarnessConfig> {
    let host_id = crate::local_host::host_address(&loaded.config.host);
    medulla::config::load_layered_custom_harnesses(&loaded.sources)
        .unwrap_or_default()
        .into_iter()
        .filter(|harness| harness.host_id == host_id)
        .collect()
}

/// This machine's workflow settings.
///
/// An explicitly selected config is part of the command contract, so a read or
/// parse failure is returned instead of silently launching a review under
/// defaults the operator did not request.
fn load_workflows_config(
    parsed: &WorkflowArgs,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
) -> anyhow::Result<medulla::config::WorkflowsConfig> {
    Ok(
        medulla::config::load_config(parsed.config.as_deref(), env, cwd)?
            .config
            .workflows,
    )
}
