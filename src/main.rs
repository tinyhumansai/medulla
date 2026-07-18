//! Entry point: arg parsing, terminal setup/teardown (alt screen, raw mode,
//! kitty keyboard enhancement, mouse capture, panic-safe restore), and the
//! tokio event loop over crossterm events, runtime pings, and a 90ms tick.

use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, EventStream, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::cli::{
    core_socket_plan, missing_token_note, parse_command, parse_tui_args, resolve_backend_token,
    sessions_json, Command, CorePlan,
};
use medulla::client::MedullaClient;
use medulla::config::load_config;
use medulla::runtime::backend::BackendRuntime;
use medulla::runtime::core::CoreRuntime;
use medulla::runtime::core_client::CoreClient;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{ContextItem, Runtime};
use medulla::ui::app::{App, Cmd, TABS};

/// Messages sent from spawned async tasks back to the event loop.
enum AppMsg {
    Status(String),
    Contexts(Vec<ContextItem>),
    OpenResume(Vec<medulla::ui::chat_store::MainChatSummary>),
    Resumed(String),
}

struct TermGuard {
    alt_screen: bool,
    kitty: bool,
}

impl TermGuard {
    fn setup(alt_screen: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        if alt_screen {
            execute!(out, EnterAlternateScreen)?;
        }
        execute!(out, EnableMouseCapture)?;
        let kitty = supports_keyboard_enhancement().unwrap_or(false);
        if kitty {
            queue!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        out.flush()?;
        Ok(TermGuard { alt_screen, kitty })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore(self.alt_screen, self.kitty);
    }
}

fn restore(alt_screen: bool, kitty: bool) {
    let mut out = io::stdout();
    if kitty {
        let _ = queue!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableMouseCapture);
    if alt_screen {
        let _ = execute!(out, LeaveAlternateScreen);
    }
    let _ = disable_raw_mode();
    let _ = out.flush();
}

fn set_mouse_capture(on: bool) {
    let mut out = io::stdout();
    if on {
        let _ = execute!(out, EnableMouseCapture);
    } else {
        let _ = execute!(out, DisableMouseCapture);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match parse_command(&raw) {
        Command::Daemon => medulla::daemon::run_daemon(&raw[1..]).await,
        Command::Version => {
            println!("medulla {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            print!("{}", medulla::cli::help_text());
            Ok(())
        }
        Command::Sessions => {
            let env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string());
            match sessions_json(&env, &cwd) {
                Ok(json) => println!("{json}"),
                Err(err) => eprintln!("failed to serialize sessions: {err}"),
            }
            Ok(())
        }
        // Bare invocation, or the TUI's own --config/--no-alt-screen flags.
        Command::Tui => run_tui(&raw).await,
    }
}

async fn run_tui(raw: &[String]) -> anyhow::Result<()> {
    let args = parse_tui_args(raw);

    if !io::stdout().is_terminal() {
        eprintln!("medulla-tui requires an interactive terminal (TTY).");
        std::process::exit(1);
    }

    let loaded = load_config(&args.config)?;

    // Runtime selection order (spec §5):
    //   1. `--core`, or a `[core]` config section, with a reachable core socket → CoreRuntime
    //   2. a backend token (inline or via `backend.tokenEnv`)             → BackendRuntime
    //   3. otherwise                                                       → MockRuntime
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let want_core = args.core || loaded.config.core.is_some();
    let plan = core_socket_plan(
        want_core,
        loaded
            .config
            .core
            .as_ref()
            .and_then(|c| c.socket_path.as_deref()),
        env.get("XDG_RUNTIME_DIR").map(String::as_str),
        env.get("MEDULLA_STATE_DIR").map(String::as_str),
        |p| p.exists(),
    );

    let mut runtime: Option<Arc<dyn Runtime>> = None;
    let mut startup_status: Option<String> = None;

    match plan {
        CorePlan::Skip => {}
        CorePlan::Fallback(note) => startup_status = Some(note),
        CorePlan::Connect(path) => {
            let version = env!("CARGO_PKG_VERSION");
            match CoreClient::connect(&path).await {
                Ok((client, events_rx)) => {
                    match CoreRuntime::connect(client, events_rx, version).await {
                        Ok(rt) => runtime = Some(Arc::new(rt)),
                        Err(e) => {
                            startup_status =
                                Some(format!("core handshake failed ({e}) — falling back"));
                        }
                    }
                }
                Err(e) => {
                    startup_status = Some(format!(
                        "core socket {} unreachable ({e}) — falling back",
                        path.display()
                    ));
                }
            }
        }
    }

    if runtime.is_none() {
        let backend = &loaded.config.backend;
        let core_note = startup_status.take();
        let (rt, note): (Arc<dyn Runtime>, Option<String>) =
            match resolve_backend_token(&env, backend) {
                Some(tok) => {
                    let client = MedullaClient::new(backend.base_url.clone(), tok);
                    match BackendRuntime::connect(client).await {
                        Ok(rt) => (Arc::new(rt), None),
                        Err(e) => (
                            Arc::new(MockRuntime::demo()),
                            Some(format!(
                                "backend connect failed ({e}) — running with mock runtime"
                            )),
                        ),
                    }
                }
                None => (
                    Arc::new(MockRuntime::demo()),
                    Some(missing_token_note(backend)),
                ),
            };
        runtime = Some(rt);
        // Prefer the more specific fallback note (core → backend → mock).
        startup_status = core_note.or(note);
    }

    let runtime = runtime.expect("a runtime is always selected");

    // Optional background tiny.place presence service (observational only): keep
    // the identity online, auto-accept peer contacts, and poll peer presence,
    // surfacing all of it into the Overview panel and Agents lanes.
    let mut tinyplace_status: Option<String> = None;
    let tinyplace_service = match &loaded.config.tinyplace {
        Some(tp) => match medulla::tinyplace_support::service::TinyplaceService::start(tp) {
            Ok(service) => Some(service),
            Err(e) => {
                tinyplace_status = Some(format!("tinyplace service failed to start ({e})"));
                None
            }
        },
        None => None,
    };
    let tinyplace_obs = tinyplace_service.as_ref().map(|s| s.observation());

    // Restore the terminal on panic before the default hook prints the message.
    let alt = args.alt_screen;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(alt, true);
        default_hook(info);
    }));

    let guard = TermGuard::setup(args.alt_screen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let result = run(
        &mut terminal,
        runtime.clone(),
        loaded,
        startup_status.or(tinyplace_status),
        tinyplace_obs,
    )
    .await;

    // Explicit teardown (the guard also runs on drop / panic).
    drop(guard);
    drop(tinyplace_service); // aborts the background loops.
    runtime.shutdown().await.ok();
    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    runtime: Arc<dyn Runtime>,
    loaded: medulla::config::LoadedConfig,
    startup_status: Option<String>,
    tinyplace_obs: Option<
        Arc<std::sync::Mutex<medulla::tinyplace_support::service::TinyplaceObservation>>,
    >,
) -> anyhow::Result<()> {
    let mut app = App::new(runtime.clone(), loaded);
    if let Some(obs) = tinyplace_obs {
        app.set_tinyplace_observation(obs);
    }
    if let Some(status) = startup_status {
        app.set_status(status);
    }
    let mut sub = runtime.subscribe();
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AppMsg>();
    let mut mouse_on = true;

    loop {
        terminal.draw(|f| app.draw(f))?;
        if app.should_quit {
            break;
        }
        if app.mouse_capture != mouse_on {
            mouse_on = app.mouse_capture;
            set_mouse_capture(mouse_on);
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(ev)) = maybe_event {
                    if let Some(cmd) = app.on_event(ev) {
                        run_cmd(cmd, &runtime, &msg_tx);
                    }
                }
            }
            recv = sub.recv() => {
                if recv.is_ok() {
                    app.refresh_snapshot();
                    if app.tab() == "Context" && app.events_changed() {
                        run_cmd(Cmd::InspectContext, &runtime, &msg_tx);
                    }
                }
            }
            Some(msg) = msg_rx.recv() => {
                match msg {
                    AppMsg::Status(s) => { app.set_status(s); app.refresh_snapshot(); }
                    AppMsg::Contexts(c) => app.set_contexts(c),
                    AppMsg::OpenResume(chats) => app.open_resume(chats),
                    AppMsg::Resumed(s) => {
                        app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap_or(1);
                        app.refresh_snapshot();
                        app.set_status(s);
                    }
                }
            }
            _ = tick.tick() => {
                if app.snapshot.running {
                    app.frame = app.frame.wrapping_add(1);
                }
            }
        }
    }
    Ok(())
}

fn run_cmd(
    cmd: Cmd,
    runtime: &Arc<dyn Runtime>,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    match cmd {
        Cmd::Quit => {}
        Cmd::Submit(input) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let status = match rt.submit(input).await {
                    Ok(()) => "Cycle complete".to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = tx.send(AppMsg::Status(status));
            });
        }
        Cmd::Resume(id) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.resume_chat(id).await {
                    Ok(()) => {
                        let _ = tx.send(AppMsg::Resumed("Resumed chat".into()));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::ListChats => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.list_main_chats().await {
                    Ok(chats) => {
                        let _ = tx.send(AppMsg::OpenResume(chats));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::InspectContext => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.inspect_context().await {
                    Ok(items) => {
                        let _ = tx.send(AppMsg::Contexts(items));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(e.to_string()));
                    }
                }
            });
        }
        Cmd::WorkerOp(op) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                let status = match rt.worker_op(op).await {
                    Ok(()) => "Worker registry updated".to_string(),
                    Err(e) => e.to_string(),
                };
                let _ = tx.send(AppMsg::Status(status));
            });
        }
    }
}
