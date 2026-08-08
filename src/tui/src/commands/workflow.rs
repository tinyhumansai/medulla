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
use serde_json::{json, Map, Value};

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
        return medulla::mcp::serve_stdio(&env, &cwd)
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
        WorkflowAction::DryRun(id) => {
            let inputs = declared_inputs(&parsed, &declarations(&store, id))?;
            ops::dry_run(&store, id, trigger_input(&parsed)?, inputs).await?
        }
        // The CLI prints whole records: its reader is an operator with a
        // terminal and `jq`, not a model with a context window.
        WorkflowAction::ListRuns(id) => ops::list_runs(&store, id, ops::StepDetail::Full)?,
        WorkflowAction::GetRun(run_id) => ops::get_run(&store, run_id, ops::StepDetail::Full)?,
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
            let (config, launch) = load_workflows_config(&parsed, &env, &cwd)?;
            ops::evolve(&store, &config, &launch, &cwd, id, parsed.run_id.as_deref()).await?
        }
        WorkflowAction::Author(id) => {
            if let Some(path) = parsed.config.as_deref() {
                std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
            }
            let (config, launch) = load_workflows_config(&parsed, &env, &cwd)?;
            let instruction = parsed.text.as_deref().unwrap_or_default();
            if instruction.trim().is_empty() {
                anyhow::bail!("author needs an instruction: pass it with --text \"<what to do>\"");
            }
            // Progress goes to stderr, never stdout: stdout is the JSON result
            // a caller parses, and interleaving a harness's chatter into it
            // would break every consumer of this command.
            let (status_tx, mut status_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let forward = tokio::spawn(async move {
                while let Some(line) = status_rx.recv().await {
                    // Classified rather than printed raw, for the same reason
                    // the pane classifies it: a frame carries the provider's
                    // call id after an invisible separator, so echoing it
                    // whole prints `workflow_listtoolu_01Vu…` and reads as a
                    // corrupted tool name.
                    eprintln!("{}", progress_line(&line));
                }
            });
            let result = ops::author(
                &store,
                &config,
                &launch,
                &cwd,
                id.as_deref(),
                instruction,
                Some(status_tx),
            )
            .await;
            // The sender is dropped with the turn, so this ends on its own;
            // awaiting it keeps a trailing progress line from landing after
            // the result.
            let _ = forward.await;
            result?
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
    let inputs = declared_inputs(parsed, &declarations(store, id))?;
    let record = run_workflow(context, id, &run_id, trigger_input(parsed)?, inputs).await?;
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
    // an operator running it by hand would expect — unless `--workspace` named
    // another checkout, which is how one command runs a workflow against a
    // repository the operator is not standing in.
    let workspace = medulla::workflows::workspace::resolve(parsed.workspace.as_deref(), cwd, env)?
        .to_string_lossy()
        .to_string();
    settings.workspace = workspace.clone();
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
    // The same policy every other Medulla spawn door applies. Without it this
    // command's harnesses ran with no lifecycle hooks at all — neither the
    // operator's nor Medulla's own reporting ones — and attributed their
    // commits regardless of `[attribution] commit`.
    let launch =
        medulla::harness_hooks::LaunchPolicy::from_config(&loaded.config).without_builtin_hooks();
    let host = LocalWorkflowHost::start(
        EmbeddedDaemonOptions {
            workspace: workspace.clone(),
            model: (!loaded.config.workflows.default_model.is_empty())
                .then(|| loaded.config.workflows.default_model.clone()),
            default_provider: loaded.config.workflows.default_provider,
            custom_harnesses,
            ..Default::default()
        }
        .with_launch_policy(&launch),
    )
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
    let max_loop_iterations = settings.max_loop_iterations;

    Ok((
        RunContext {
            // Runs inline, so claiming at the top of the run is early enough.
            claim: None,
            store: store.clone(),
            settings: Arc::new(settings),
            services: HostServices::new(
                dispatch,
                Arc::new(StoreWorkflowResolver::new(
                    store.clone(),
                    max_loop_iterations,
                )),
                HashMap::new(),
            ),
            sink,
            step_snapshot: None,
            // `medulla workflow run` on a terminal. The workspace is worth
            // recording even though nothing else about the caller is: a run
            // that touched the wrong checkout is a common enough mistake that
            // the record should be able to prove which one it used.
            origin: Some(
                medulla::workflows::RunOrigin::of_kind(medulla::workflows::RunOrigin::CLI)
                    .labelled("medulla workflow run")
                    .in_workspace(workspace),
            ),
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

/// The values supplied for a workflow's declared inputs, from `--inputs` and
/// `--set`.
///
/// `--set name=value` is merged over `--inputs`, so the two compose: a base
/// object plus a one-off override reads the way an operator expects.
///
/// A `--set` value arrives as a string, because a shell argument is one. It is
/// coerced to the input's *declared* type — the declaration is the only thing
/// that can say whether `3` means the number three or the string "3", and
/// guessing from the text would make `--set version=1.0` silently become a
/// float. An input the workflow does not declare is left as a string and
/// resolution rejects it by name, which is the more useful error than a
/// complaint about its type.
fn declared_inputs(
    parsed: &WorkflowArgs,
    declared: &[medulla::workflows::WorkflowInput],
) -> anyhow::Result<Map<String, Value>> {
    use medulla::workflows::InputType;

    let mut values = match &parsed.inputs {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(map)) => map,
            Ok(_) => anyhow::bail!("--inputs must be a JSON object keyed by declared input name"),
            Err(err) => anyhow::bail!("--inputs is not valid JSON: {err}"),
        },
        None => Map::new(),
    };

    for pair in &parsed.set {
        let Some((name, raw)) = pair.split_once('=') else {
            anyhow::bail!("--set expects <name>=<value>, got {pair:?}");
        };
        let ty = declared
            .iter()
            .find(|input| input.name == name)
            .map(|input| input.ty);
        let value = match ty {
            Some(InputType::Number) => Value::Number(
                raw.parse::<serde_json::Number>()
                    .map_err(|_| anyhow::anyhow!("--set {name}: {raw:?} is not a number"))?,
            ),
            Some(InputType::Boolean) => Value::Bool(
                raw.parse::<bool>()
                    .map_err(|_| anyhow::anyhow!("--set {name}: {raw:?} is not true or false"))?,
            ),
            Some(InputType::Json) => serde_json::from_str(raw)
                .map_err(|err| anyhow::anyhow!("--set {name}: not valid JSON: {err}"))?,
            // Declared `string`, or undeclared — see the doc above.
            Some(InputType::String) | None => Value::String(raw.to_string()),
        };
        values.insert(name.to_string(), value);
    }

    Ok(values)
}

/// The declared inputs of a saved workflow, or an empty list when it is unknown
/// — the operation itself reports a missing workflow far better than this would.
fn declarations(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
) -> Vec<medulla::workflows::WorkflowInput> {
    store
        .get(id)
        .ok()
        .flatten()
        .map(|record| record.graph.inputs)
        .unwrap_or_default()
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

/// This machine's workflow settings, and the policy its harnesses launch under.
///
/// An explicitly selected config is part of the command contract, so a read or
/// parse failure is returned instead of silently launching a review under
/// defaults the operator did not request. The launch policy travels with the
/// settings for the same reason: the embedded host these commands start spawns
/// real harnesses, and one started without it installs no lifecycle hooks and
/// attributes its commits to whatever the default happens to be.
fn load_workflows_config(
    parsed: &WorkflowArgs,
    env: &HashMap<String, String>,
    cwd: &std::path::Path,
) -> anyhow::Result<(
    medulla::config::WorkflowsConfig,
    medulla::harness_hooks::LaunchPolicy,
)> {
    let loaded = medulla::config::load_config(parsed.config.as_deref(), env, cwd)?;
    let launch =
        medulla::harness_hooks::LaunchPolicy::from_config(&loaded.config).without_builtin_hooks();
    Ok((loaded.config.workflows, launch))
}

/// One harness progress frame, as a line for a terminal.
///
/// The pane draws these as coloured rows with a glyph per kind; a terminal gets
/// the same distinctions as ASCII markers. Shares
/// [`classify_progress`](medulla::ui::workflows::classify_progress) with the
/// pane so the two cannot come to disagree about what counts as a tool call.
fn progress_line(frame: &str) -> String {
    use medulla::ui::workflows::{classify_progress, Progress};

    match classify_progress(frame) {
        Progress::Tool { text, .. } => format!("↻ {text}"),
        Progress::ToolResult { failed, detail, .. } => {
            let mark = if failed { "✗" } else { "✓" };
            if detail.is_empty() {
                mark.to_string()
            } else {
                format!("{mark} {detail}")
            }
        }
        // Reasoning streams in fragments and would otherwise scroll a terminal
        // off its own output; the pane keeps one updating row, which a stream
        // of lines cannot do, so it is summarised to the fact that it happened.
        Progress::Thinking(_) => "· thinking".to_string(),
        Progress::Status(text) => format!("· {text}"),
    }
}
