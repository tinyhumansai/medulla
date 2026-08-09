//! The worker TUI's entry point and event loop — `medulla daemon --tui`.
//!
//! One process: the host-link identity, the harness PTYs, and
//! the UI all live here. That is why closing the TUI stops the daemon — there is
//! no daemon behind it to keep running.
//!
//! The loop is deliberately tick-driven at 40ms rather than woken by the PTY
//! readers. A harness repainting its screen produces a continuous byte stream,
//! so a wake-per-chunk would redraw far more often than a terminal can show;
//! a fixed cadence bounds the work and still looks live.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::EventStream;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::bridge::{Bridge as _, LinkBridge};
use medulla::daemon::{DaemonConfig, DaemonRuntime};
use medulla::protocol::{decode_task_frame, HarnessProvider};

use medulla_tui::log::LogBuffer;
use medulla_tui::worker::app::{ExecutionMode, WorkerApp, WorkerWiring};
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

use crate::terminal::{set_mouse_capture, TermGuard};

mod commands;

/// Redraw cadence. Fast enough that a harness's own cursor blink and spinner
/// look native, slow enough to bound the cost of a full repaint.
const TICK: Duration = Duration::from_millis(40);

/// How often the encrypted inbox is drained for new peer work.
const INBOX_POLL: Duration = Duration::from_millis(1_000);

/// Peer work admitted at once before the daemon sheds load.
const MAX_PENDING: usize = 16;

/// Run the worker TUI to exit.
///
/// `env` is the process environment harness sessions inherit; `workspace` is the
/// directory they run in.
pub async fn run_worker_tui(config: WorkerTuiConfig) -> anyhow::Result<()> {
    let WorkerTuiConfig {
        env,
        workspace,
        workspaces,
        masters,
        config_path,
        credential_dir,
        agent_id,
        only_providers,
        startup_status,
        transport,
        endpoint,
        theme,
        trust_workspace,
        skip_permissions,
        router,
        budget,
        attribution,
        hooks,
    } = config;
    // Restricted exactly as the headless daemon restricts it: `--providers` is
    // the operator saying which coding agents this worker may run, and a screen
    // that ignored it would offer — and settle on — one they excluded.
    let providers =
        medulla::daemon::providers::detect_providers(&env, only_providers.as_deref(), None);
    let sessions = PtyManager::new();
    let logs = LogBuffer::new();
    // Persist the daemon's narration. The screen only helps while someone is
    // looking at it; a task that failed overnight has to be answerable for
    // afterwards.
    let log_path = logs.attach_file(&medulla_tui::log::default_log_dir(&env), "worker");

    // State the identity and the forwarder together, first thing. Two peers
    // pointed at different forwarders both start cleanly and report healthy —
    // the only symptom is that neither ever hears from the other. Side by side
    // with the orchestrator's own line, a mismatch is immediate.
    match &endpoint {
        Some(endpoint) => logs.push(format!(
            "host link: {} on {endpoint}",
            agent_id.as_deref().unwrap_or("(no identity)")
        )),
        // Unconditional, because the silent case is the one that needs saying.
        // A worker with no forwarder serves nobody, and if this line is skipped
        // the log's last entry is from whenever the machine last had an identity
        // — so a worker that has been up for hours doing nothing is
        // indistinguishable from one that was never started at all.
        None => logs.push(
            "host link: no forwarder configured — this worker serves local sessions only"
                .to_string(),
        ),
    }

    // Says, in the log, that the process is up but idle. `start_worker` is what
    // writes the next line, and it only runs once the launch step is answered —
    // so without this the gap between "booted" and "serving" leaves no trace,
    // and a worker parked on the launch step looks identical to a wedged one.
    // It also explains the keyboard: the launch step owns every key until it is
    // answered, which reads as dead navigation to anyone expecting the tabs.
    logs.push(
        "worker: up, waiting on the launch step — no peer work is served until it is answered"
            .to_string(),
    );

    // The inbox is not drained until the operator has answered the launch step.
    // A worker should not accept peer work before it has been told how to run
    // it — and the mode decides which executor the runtime is even built with.
    let mut inbox: Option<tokio::task::JoinHandle<()>> = None;
    let mut runtime: Option<DaemonRuntime> = None;

    let startup_status = match (startup_status, &log_path) {
        (Some(status), _) => Some(status),
        (None, Some(path)) => Some(format!("logging to {}", path.display())),
        (None, None) => None,
    };
    let mut app = WorkerApp::new(WorkerWiring {
        sessions: sessions.clone(),
        agent_id,
        providers: providers.clone(),
        startup_status,
        logs: logs.clone(),
        primary_workspace: workspace.clone(),
        workspaces: workspaces.clone(),
        masters,
        config_path,
        credential_dir,
        endpoint: endpoint.clone(),
        theme,
    });

    // The guard restores the terminal even on a panic — a worker TUI that dies
    // mid-frame must not leave the operator's shell in raw mode with the
    // alternate screen still up.
    let _guard = TermGuard::setup(true)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let start = StartWiring {
        env: env.clone(),
        workspace: workspace.clone(),
        workspaces,
        providers: providers.clone(),
        sessions: sessions.clone(),
        transport,
        logs,
        trust_workspace,
        skip_permissions,
        router,
        budget,
        attribution,
        hooks,
    };
    let result = drive(&mut terminal, &mut app, &start, &mut inbox, &mut runtime).await;

    // Every harness dies with the TUI. Leaving one attached to a PTY nobody
    // holds would strand a process the operator can no longer see or stop.
    sessions.shutdown();
    if let Some(inbox) = inbox {
        inbox.abort();
    }
    result
}

/// Build the daemon runtime that serves peer work.
///
/// The mode picks the executor and nothing else: admission control, duplicate
/// rejection, correlation and replies are the runtime's, identically either way.
pub(super) fn worker_runtime(
    start: &StartWiring,
    mode: ExecutionMode,
    provider: HarnessProvider,
    transport: &LinkBridge,
) -> DaemonRuntime {
    let StartWiring {
        env,
        workspace,
        workspaces,
        providers,
        sessions,
        logs,
        router,
        budget,
        attribution,
        hooks,
        ..
    } = start;
    let config = DaemonConfig {
        providers: providers.to_vec(),
        // The operator's choice is the fallback for a frame that names none.
        default_provider: provider,
        workspace: workspace.to_string(),
        accessible_dirs: workspaces.clone(),
        env: env.clone(),
        // The executor settles a turn when the harness says it is done, so this
        // is only the outer bound on a wedged one.
        task_timeout_ms: 30 * 60 * 1_000,
        capability_timeout_ms: None,
        concurrency: 4,
        status_throttle_ms: 4_000,
        max_pending: MAX_PENDING,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        // A worker serves peers while nobody is sitting in the pane. A harness
        // that stops to ask "allow this command?" has silently hung until the
        // task times out, so the bypass is the default and
        // `--no-skip-permissions` is how an operator who *is* watching declines
        // it. Narrated at startup: this is not a default to discover later.
        skip_permissions: start.skip_permissions,
        // The custom OpenAI-compatible router from the loaded `[router]` config.
        // Layered into every peer task's spawn env by the same executor the
        // headless daemon uses, so `--tui` and headless route identically.
        router: router.clone(),
        attribution: *attribution,
        hooks: hooks.clone(),
        // The standalone worker TUI does not yet expose a config editor; named
        // presets are loaded by the orchestrator's embedded host.
        custom_harnesses: Vec::new(),
        // Operator-declared budgets from the `[budget]` config, advertised on the
        // capability probe as `source: configured` for matching providers.
        budget: budget.clone(),
    };
    let executor = match mode {
        // The same executor `medulla daemon` uses, so headless-with-a-screen is
        // the existing daemon plus a view, not a second implementation of it.
        ExecutionMode::Headless => std::sync::Arc::new(|options| {
            Box::pin(medulla::daemon::providers::run_provider_task(options))
                as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
        }) as medulla::daemon::providers::RunTaskFn,
        ExecutionMode::Interactive => {
            PtySessionExecutor::new(sessions.clone(), env.clone(), workspace.to_string())
                .with_log(logs.sink())
                .into_run_task()
        }
    };
    let send = {
        let transport = transport.clone();
        let logs = logs.clone();
        Arc::new(move |to: String, body: String| {
            let transport = transport.clone();
            let logs = logs.clone();
            Box::pin(async move {
                loop {
                    match transport.send(&to, &body).await {
                        Ok(()) => break,
                        Err(err) if err.starts_with("link queue overflow:") => {
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                        Err(err) => {
                            logs.push(format!("worker: send to {to} failed ({err})"));
                            break;
                        }
                    }
                }
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    // The log the headless view renders is the daemon's own narration, captured
    // rather than reprinted — the same lines `medulla daemon` writes to stderr.
    DaemonRuntime::new(config, executor, send).with_log(logs.sink())
}

/// Drain the encrypted inbox into the runtime until aborted.
///
/// Screen messages are claimed **before** the runtime sees the body, and that
/// ordering is load-bearing rather than tidy: `DaemonRuntime::handle_message`
/// routes anything that is not a task frame to the plain-text path, which types
/// it into a harness as a prompt. An unclaimed screen message would not be
/// ignored — it would be executed.
pub(super) fn spawn_inbox_drain(
    transport: LinkBridge,
    runtime: DaemonRuntime,
    mut screens: medulla_tui::worker::stream::ScreenRouter,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            for message in transport.drain_inbox(50).await {
                if let Some(screen) = medulla::protocol::parse_screen_message(&message.text) {
                    screens.handle(&message.from, screen);
                    continue;
                }
                let frame = decode_task_frame(&message.text);
                // A worker-loop inbox only ever carries host-link traffic, i.e.
                // remote peers, so a forged `workflowNode` marker must not buy
                // workflow authority here. Ask the transport rather than
                // hard-coding `false` so the verdict tracks the link.
                let sender_device_local = transport.is_device_local(&message.from).await;
                runtime.handle_message_from(
                    message.from,
                    message.text,
                    frame,
                    sender_device_local,
                );
            }
            // Returns early when the link's pump delivers, so a subscribe is
            // acted on at about a round trip rather than up to a poll interval
            // later. The interval stays the correctness floor.
            transport.wait_for_inbox(INBOX_POLL).await;
        }
    })
}

/// Pre-trust the workspace so claude does not open on its trust dialog.
///
/// Narrated to the log and the status line: silently editing somebody's claude
/// config would be the wrong kind of convenient, even when it is the thing they
/// asked for by naming the workspace.
pub(super) fn claude_preflight(start: &StartWiring, app: &mut WorkerApp) {
    use medulla_tui::worker::trust;

    let mut said = Vec::new();
    if start.trust_workspace {
        let outcome = trust::ensure_workspace_trusted(&start.env, &start.workspace);
        said.extend(outcome.log_line(&format!("trusted {}", start.workspace)));
    }
    // The bypass disclaimer's default option is "No, exit", so meeting it
    // unattended does not mistype a prompt — it kills the session. Accept it up
    // front: that is the decision the operator already made by asking for the
    // mode.
    if start.skip_permissions {
        let outcome = trust::ensure_bypass_accepted(&start.env);
        said.extend(outcome.log_line("accepted the bypass-permissions disclaimer"));
    }
    for line in &said {
        start.logs.push(line.clone());
    }
    if let Some(last) = said.pop() {
        app.set_status(last);
    }
}

async fn drive(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut WorkerApp,
    start: &StartWiring,
    inbox: &mut Option<tokio::task::JoinHandle<()>>,
    runtime: &mut Option<DaemonRuntime>,
) -> anyhow::Result<()> {
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    let mut mouse_on = true;

    loop {
        terminal.draw(|f| app.draw(f))?;
        if app.should_quit {
            return Ok(());
        }
        if app.mouse_capture() != mouse_on {
            mouse_on = app.mouse_capture();
            set_mouse_capture(mouse_on);
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(event)) = maybe_event {
                    if let Some(cmd) = app.on_event(event) {
                        commands::run_cmd(app, cmd, start, inbox, runtime).await;
                    }
                }
            }
            _ = tick.tick() => {}
        }
    }
}

mod types;
pub(super) use types::StartWiring;
pub use types::WorkerTuiConfig;
