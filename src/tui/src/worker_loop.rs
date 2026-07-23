//! The worker TUI's entry point and event loop — `medulla daemon --tui`.
//!
//! One process: the tiny.place identity and contact poll, the harness PTYs, and
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

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::contacts::ContactDesk;
use medulla::daemon::transport::SignalTransport;
use medulla::daemon::{DaemonConfig, DaemonRuntime};
use medulla::tinyplace::{decode_task_frame, HarnessProvider};

use medulla_tui::log::LogBuffer;
use medulla_tui::worker::app::{ExecutionMode, WorkerApp, WorkerCmd, WorkerWiring};
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

use crate::terminal::TermGuard;

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
#[allow(clippy::too_many_arguments)]
pub async fn run_worker_tui(
    env: HashMap<String, String>,
    workspace: String,
    contacts: Option<ContactDesk>,
    agent_id: Option<String>,
    startup_status: Option<String>,
    transport: Option<SignalTransport>,
    endpoint: Option<String>,
    trust_workspace: bool,
    skip_permissions: bool,
) -> anyhow::Result<()> {
    let providers = medulla::daemon::providers::detect_providers(&env, None, None);
    let sessions = PtyManager::new();
    let logs = LogBuffer::new();
    // Persist the daemon's narration. The screen only helps while someone is
    // looking at it; a task that failed overnight has to be answerable for
    // afterwards.
    let log_path = logs.attach_file(&medulla_tui::log::default_log_dir(&env), "worker");

    // State the wallet and the relay together, first thing. Two peers pointed at
    // different relays both start cleanly, publish keys and report healthy — the
    // only symptom is that neither ever hears from the other. Side by side with
    // the orchestrator's own line, a mismatch is immediate.
    if let Some(endpoint) = &endpoint {
        logs.push(format!(
            "tiny.place: {} on {endpoint}",
            agent_id.as_deref().unwrap_or("(no identity)")
        ));
    }

    // The contact queue narrates into the same log as everything else, so
    // "nobody asked" and "the worker never saw it" stop looking alike.
    // No `spawn_poll` here: `TinyplaceService::start` already polls this same
    // desk, and a second loop would double the relay traffic and interleave the
    // snapshots — which shows up as duplicate "new request(s)" narration. The
    // sink is shared across handles, so attaching it here reaches the poll the
    // service owns.
    let contacts = contacts.map(|desk| {
        let logs = logs.clone();
        desk.with_log(Arc::new(move |line: &str| logs.push(line)))
    });

    // The inbox is not drained until the operator has answered the launch step.
    // A worker should not accept peer work before it has been told how to run
    // it — and the mode decides which executor the runtime is even built with.
    let mut inbox: Option<tokio::task::JoinHandle<()>> = None;

    let startup_status = match (startup_status, &log_path) {
        (Some(status), _) => Some(status),
        (None, Some(path)) => Some(format!("logging to {}", path.display())),
        (None, None) => None,
    };
    let mut app = WorkerApp::new(WorkerWiring {
        sessions: sessions.clone(),
        contacts,
        agent_id,
        providers: providers.clone(),
        startup_status,
        logs: logs.clone(),
    });

    // The guard restores the terminal even on a panic — a worker TUI that dies
    // mid-frame must not leave the operator's shell in raw mode with the
    // alternate screen still up.
    let _guard = TermGuard::setup(true)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let start = StartWiring {
        env: env.clone(),
        workspace: workspace.clone(),
        providers: providers.clone(),
        sessions: sessions.clone(),
        transport,
        logs,
        trust_workspace,
        skip_permissions,
    };
    let result = drive(&mut terminal, &mut app, &start, &mut inbox).await;

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
fn worker_runtime(
    start: &StartWiring,
    mode: ExecutionMode,
    provider: HarnessProvider,
    transport: &SignalTransport,
) -> DaemonRuntime {
    let StartWiring {
        env,
        workspace,
        providers,
        sessions,
        logs,
        ..
    } = start;
    let config = DaemonConfig {
        providers: providers.to_vec(),
        // The operator's choice is the fallback for a frame that names none.
        default_provider: provider,
        workspace: workspace.to_string(),
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
                .into_run_task()
        }
    };
    let send = {
        let transport = transport.clone();
        Arc::new(move |to: String, body: String| {
            let transport = transport.clone();
            Box::pin(async move {
                let _ = transport.send(&to, &body).await;
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    // The log the headless view renders is the daemon's own narration, captured
    // rather than reprinted — the same lines `medulla daemon` writes to stderr.
    DaemonRuntime::new(config, executor, send).with_log(logs.sink())
}

/// Drain the encrypted inbox into the runtime until aborted.
fn spawn_inbox_drain(
    transport: SignalTransport,
    runtime: DaemonRuntime,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            for message in transport.drain_inbox(50).await {
                let frame = decode_task_frame(&message.text);
                runtime.handle_message(message.from, message.text, frame);
            }
            tokio::time::sleep(INBOX_POLL).await;
        }
    })
}

/// The select loop.
/// What building the daemon runtime needs, once the launch step is answered.
struct StartWiring {
    env: HashMap<String, String>,
    workspace: String,
    providers: Vec<HarnessProvider>,
    sessions: PtyManager,
    transport: Option<SignalTransport>,
    logs: LogBuffer,
    /// Whether to pre-trust the workspace with claude. `--no-trust-workspace`
    /// clears it for an operator who would rather answer the dialog themselves.
    trust_workspace: bool,
    /// Whether peer sessions run with the harness's permission-bypass flag.
    /// `--no-skip-permissions` clears it.
    skip_permissions: bool,
}

/// Pre-trust the workspace so claude does not open on its trust dialog.
///
/// Narrated to the log and the status line: silently editing somebody's claude
/// config would be the wrong kind of convenient, even when it is the thing they
/// asked for by naming the workspace.
fn claude_preflight(start: &StartWiring, app: &mut WorkerApp) {
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
) -> anyhow::Result<()> {
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(TICK);

    loop {
        terminal.draw(|f| app.draw(f))?;
        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(event)) = maybe_event {
                    if let Some(cmd) = on_event(app, event) {
                        run_cmd(app, cmd, start, inbox).await;
                    }
                }
            }
            _ = tick.tick() => {}
        }
    }
}

/// Route one terminal event.
fn on_event(app: &mut WorkerApp, event: Event) -> Option<WorkerCmd> {
    match event {
        Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => app.on_key(key),
        // A resize is handled by the next draw, which re-measures the pane and
        // resizes the PTY to match.
        Event::Resize(_, _) => None,
        _ => None,
    }
}

/// Execute a command the app emitted.
async fn run_cmd(
    app: &mut WorkerApp,
    cmd: WorkerCmd,
    start: &StartWiring,
    inbox: &mut Option<tokio::task::JoinHandle<()>>,
) {
    match cmd {
        WorkerCmd::Quit => app.should_quit = true,
        WorkerCmd::Refresh => {
            let Some(desk) = app.contact_desk() else {
                app.set_status("No tiny.place identity — nothing to refresh");
                return;
            };
            let health = desk.refresh().await;
            app.set_status(match &health {
                medulla::contacts::PollHealth::Failed { error, .. } => {
                    format!("Relay unreachable: {error}")
                }
                _ => format!(
                    "Relay checked · {} request(s) known, {} pending",
                    desk.requests().len(),
                    desk.pending_count()
                ),
            });
        }
        WorkerCmd::Start { mode, provider } => {
            let Some(transport) = start.transport.clone() else {
                app.set_status("No tiny.place identity — this worker serves local sessions only");
                return;
            };
            if inbox.is_some() {
                return; // already serving
            }
            // Clear claude's workspace-trust dialog before any peer can dispatch
            // work, rather than letting the first task die against a modal.
            // Interactive-and-claude only: the dialog exists solely on a TTY,
            // and there is no reason to touch claude's config for a worker the
            // operator pointed at codex.
            if mode == ExecutionMode::Interactive && provider == HarnessProvider::Claude {
                claude_preflight(start, app);
            }
            // Say it out loud. Running peer work with permission checks off is
            // exactly the sort of default that must not be discovered from a
            // process listing three weeks later.
            start.logs.push(if start.skip_permissions {
                format!(
                    "{}: permission checks bypassed for peer tasks (--no-skip-permissions declines)",
                    provider.as_str()
                )
            } else {
                format!(
                    "{}: permission checks left on — a task that stops to ask will hang",
                    provider.as_str()
                )
            });
            let runtime = worker_runtime(start, mode, provider, &transport);
            *inbox = Some(spawn_inbox_drain(transport, runtime));
            app.set_status(format!(
                "Serving peers · {} on {}",
                mode.as_str(),
                provider.as_str()
            ));
        }
        WorkerCmd::ContactOp { agent_id, decision } => {
            let Some(desk) = app.contact_desk() else {
                app.set_status("No tiny.place identity — contact decisions are unavailable");
                return;
            };
            // Awaited inline: a contact decision is one small REST call, and
            // blocking the loop for it keeps the queue and the screen honest
            // about what has actually been settled.
            let status = match desk.decide(&agent_id, decision).await {
                Ok(status) => status,
                Err(message) => message,
            };
            app.set_status(status);
        }
    }
}
