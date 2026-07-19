//! Entry point: arg parsing, terminal setup/teardown (alt screen, raw mode,
//! kitty keyboard enhancement, mouse capture, panic-safe restore), and the
//! tokio event loop over crossterm events, runtime pings, and a 90ms tick.

use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::auth::{
    describe_me, is_one_time_login_token, missing_token_note, open_browser, resolve_backend_token,
    run_login_flow, start_loopback, CredentialStore, Credentials, LoopbackConfig,
    DEFAULT_LOGIN_TIMEOUT,
};
use medulla::client::MedullaClient;
use medulla::config::load_config;
use medulla::runtime::backend::BackendRuntime;
#[cfg(unix)]
use medulla::runtime::core::CoreRuntime;
#[cfg(unix)]
use medulla::runtime::core_client::CoreClient;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::{ContextItem, Runtime};
#[cfg(unix)]
use medulla_tui::cli::core_socket_plan;
use medulla_tui::cli::{
    parse_command, parse_login_args, parse_memory_args, parse_tui_args, sessions_json, Command,
    CorePlan, LoginArgs, MemoryAction,
};
use medulla_tui::ui::app::{App, Cmd, TABS};
use medulla_tui::ui::login::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

/// Messages sent from spawned async tasks back to the event loop.
enum AppMsg {
    Status(String),
    Contexts(Vec<ContextItem>),
    OpenResume(Vec<medulla::ui::chat_store::MainChatSummary>),
    Resumed(String),
    MemoryLoaded {
        status: Option<medulla::memory::MemoryStatus>,
        directives: Vec<String>,
    },
    UsageLoaded(Option<serde_json::Value>),
    MemoryResults {
        hits: Vec<medulla::memory::MemoryHit>,
        query: String,
    },
    /// A newer release was detected by the background update checker.
    UpdateAvailable(String),
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
    // Load a cwd `.env` into the process env before anything reads it (this is
    // how local dev opts into `MEDULLA_DEV=1`). Never overrides existing vars.
    medulla::home::load_dotenv_from_cwd();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    match parse_command(&raw) {
        Command::Daemon => medulla::daemon::run_daemon(&raw[1..]).await,
        Command::Version => {
            println!("medulla {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            print!("{}", medulla_tui::cli::help_text());
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
        Command::Login => run_login(&raw[1..]).await,
        Command::Logout => run_logout(),
        Command::Memory => run_memory(&raw[1..]).await,
        Command::Update => {
            let args = medulla_tui::cli::parse_update_args(&raw[1..]);
            medulla::update::run_update(args.check).await
        }
        Command::Wrapper(provider) => {
            let code = medulla::wrapper::run_wrapper(provider, &raw[1..]).await?;
            std::process::exit(code);
        }
        // Bare invocation, or the TUI's own --config/--no-alt-screen flags.
        Command::Tui => run_tui(&raw).await,
    }
}

/// `medulla login`: obtain a JWT (loopback OAuth or a one-time token), verify it
/// with `/auth/me`, and persist it to the credential store.
async fn run_login(args: &[String]) -> anyhow::Result<()> {
    let parsed: LoginArgs = match parse_login_args(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("medulla login: {msg}");
            std::process::exit(2);
        }
    };
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;
    let base_url = loaded.config.backend.base_url.clone();

    let jwt = match parsed.token {
        Some(token) => {
            // Headless fallback: redeem a one-time token, no listener.
            let client = MedullaClient::new(base_url.clone(), String::new());
            client
                .consume_login_token(token)
                .await
                .map_err(|e| anyhow::anyhow!("failed to redeem login token: {e}"))?
        }
        None => {
            let cfg = LoopbackConfig {
                no_browser: parsed.no_browser,
                ..Default::default()
            };
            run_login_flow(&base_url, parsed.provider, cfg, open_browser)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
        }
    };

    // Verify the token and greet the user.
    let client = MedullaClient::new(base_url.clone(), jwt.clone());
    match client.me().await {
        Ok(me) => println!("{}", medulla::auth::describe_me(&me)),
        Err(e) => return Err(anyhow::anyhow!("token verification failed: {e}")),
    }

    let store = CredentialStore::at_home(&medulla::home::medulla_home(&env));
    store.save(&Credentials { base_url, jwt })?;
    println!("Credentials saved to {}", store.path().display());
    Ok(())
}

/// `medulla logout`: clear stored credentials.
fn run_logout() -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let store = CredentialStore::at_home(&medulla::home::medulla_home(&env));
    store.clear()?;
    println!("Logged out ({} cleared).", store.path().display());
    Ok(())
}

/// `medulla memory <status|ingest|backfill|compile|search <query>>`: manage the
/// persona-memory layer from the command line.
async fn run_memory(args: &[String]) -> anyhow::Result<()> {
    let parsed = match parse_memory_args(args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("medulla memory: {msg}");
            std::process::exit(2);
        }
    };
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;
    // Summarization syncs through the backend when a token is available (an
    // explicit OPENROUTER_API_KEY still wins inside the service).
    let settings = medulla::memory::env::resolve_with_backend(
        loaded.config.memory.as_ref(),
        &loaded.config.backend,
        &env,
        &medulla::home::medulla_home(&env),
    );
    let service = medulla::memory::MemoryService::open(settings)?;

    match parsed.action {
        MemoryAction::Status => {
            let status = service.status();
            if parsed.json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print!("{}", service.overview());
            }
        }
        MemoryAction::Search(query) => {
            let hits = service.search(&query, parsed.facet.as_deref(), parsed.k);
            if parsed.json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                println!("(no matches)");
            } else {
                for hit in &hits {
                    println!("[{}] ({:.3}) {}", hit.facet, hit.score, hit.text);
                }
            }
        }
        MemoryAction::Compile => {
            let report = service.compile()?;
            print_ingest_report(&report, parsed.json)?;
        }
        MemoryAction::Ingest | MemoryAction::Backfill => {
            let mode = if matches!(parsed.action, MemoryAction::Backfill) {
                medulla::memory::IngestMode::Backfill
            } else {
                medulla::memory::IngestMode::Incremental
            };
            let report = service.ingest(mode).await?;
            print_ingest_report(&report, parsed.json)?;
        }
    }
    Ok(())
}

/// Print an ingest/compile report as JSON or a short human summary.
fn print_ingest_report(report: &medulla::memory::IngestReport, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!(
            "{}: {} files, {} sessions, {} observations{}",
            report.mode,
            report.files_seen,
            report.sessions_processed,
            report.observations,
            if report.budget_hit {
                " (budget hit)"
            } else {
                ""
            },
        );
        if let Some(path) = &report.pack_path {
            println!("pack: {path}");
        }
    }
    Ok(())
}

async fn run_tui(raw: &[String]) -> anyhow::Result<()> {
    let args = parse_tui_args(raw);

    if !io::stdout().is_terminal() {
        eprintln!("medulla-tui requires an interactive terminal (TTY).");
        std::process::exit(1);
    }

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(args.config.as_deref(), &env, &cwd)?;
    let home = medulla::home::medulla_home(&env);

    // Runtime selection order (spec §5):
    //   1. `--core`, or a `[core]` config section, with a reachable core socket → CoreRuntime
    //   2. a backend token (inline or via `backend.tokenEnv`)             → BackendRuntime
    //   3. otherwise                                                       → MockRuntime
    let want_core = args.core || loaded.config.core.is_some();
    // The core runtime rides a Unix domain socket, so on Windows a core request
    // resolves to a clear note and falls through to the backend→mock chain.
    #[cfg(unix)]
    let plan = core_socket_plan(
        want_core,
        loaded
            .config
            .core
            .as_ref()
            .and_then(|c| c.socket_path.as_deref()),
        env.get("XDG_RUNTIME_DIR").map(String::as_str),
        // The resolved state dir already reflects MEDULLA_STATE_DIR / <home>/state.
        Some(loaded.config.state_dir.as_str()),
        |p| p.exists(),
    );
    #[cfg(not(unix))]
    let plan = if want_core {
        CorePlan::Fallback(
            "core runtime requires unix sockets — unavailable on Windows; falling back".into(),
        )
    } else {
        CorePlan::Skip
    };

    let mut runtime: Option<Arc<dyn Runtime>> = None;
    let mut startup_status: Option<String> = None;

    // Optional persona-memory service (tinycortex). Built once here and attached
    // to the core runtime so it can advertise + serve the memory toolset; also
    // available to a later TUI surface via the runtime seam.
    let memory_settings = medulla::memory::env::resolve_with_backend(
        loaded.config.memory.as_ref(),
        &loaded.config.backend,
        &env,
        &medulla::home::medulla_home(&env),
    );
    let memory_service: Option<Arc<medulla::memory::MemoryService>> = if memory_settings.enabled {
        match medulla::memory::MemoryService::open(memory_settings) {
            Ok(svc) => Some(Arc::new(svc)),
            Err(e) => {
                startup_status = Some(format!("memory service failed to open ({e})"));
                None
            }
        }
    } else {
        None
    };
    // The memory service is only consumed by the unix-only core runtime; keep it
    // built (for its side effects / startup note) without an unused warning on
    // platforms where core is unavailable.
    #[cfg(not(unix))]
    let _ = &memory_service;

    match plan {
        CorePlan::Skip => {}
        CorePlan::Fallback(note) => startup_status = Some(note),
        CorePlan::Connect(_path) => {
            #[cfg(unix)]
            {
                let path = _path;
                let version = env!("CARGO_PKG_VERSION");
                match CoreClient::connect(&path).await {
                    Ok((client, events_rx)) => {
                        match CoreRuntime::connect(
                            client,
                            events_rx,
                            version,
                            memory_service.clone(),
                        )
                        .await
                        {
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
            #[cfg(not(unix))]
            {
                startup_status = Some(
                    "core runtime requires unix sockets — unavailable on Windows; falling back"
                        .into(),
                );
            }
        }
    }

    // When core is not the runtime, decide between a backend runtime, the
    // interactive login screen, or the mock. `--core` (or a `[core]` config) is
    // left on its existing backend→mock fallback so an explicit core request is
    // never hijacked by the login UX.
    let mut need_login: Option<String> = None;
    if runtime.is_none() {
        let backend = &loaded.config.backend;
        let core_note = startup_status.take();
        let stored = CredentialStore::at_home(&home).load_or_legacy();
        let token = resolve_backend_token(&env, backend, stored.as_ref());

        let (rt, note): (Option<Arc<dyn Runtime>>, Option<String>) = match (want_core, token) {
            // Explicit core that fell back: preserve the old backend→mock path.
            (true, Some(tok)) => {
                let client = MedullaClient::new(backend.base_url.clone(), tok);
                match BackendRuntime::connect(client).await {
                    Ok(rt) => (Some(Arc::new(rt)), None),
                    Err(e) => (
                        Some(Arc::new(MockRuntime::demo())),
                        Some(format!(
                            "backend connect failed ({e}) — running with mock runtime"
                        )),
                    ),
                }
            }
            (true, None) => (
                Some(Arc::new(MockRuntime::demo())),
                Some(missing_token_note(backend)),
            ),
            // Default path: no token → login screen.
            (false, None) => {
                need_login = Some(backend.base_url.clone());
                (None, None)
            }
            // Default path with a token: preflight `me()` so an expired/rejected
            // token routes to the login screen instead of silently dropping to
            // mock; a network failure keeps the old mock fallback.
            (false, Some(tok)) => {
                let client = MedullaClient::new(backend.base_url.clone(), tok);
                match client.me().await {
                    Ok(_) => match BackendRuntime::connect(client).await {
                        Ok(rt) => (Some(Arc::new(rt)), None),
                        Err(e) => (
                            Some(Arc::new(MockRuntime::demo())),
                            Some(format!(
                                "backend connect failed ({e}) — running with mock runtime"
                            )),
                        ),
                    },
                    Err(e) if e.is_auth_error() => {
                        need_login = Some(backend.base_url.clone());
                        (None, None)
                    }
                    Err(e) => (
                        Some(Arc::new(MockRuntime::demo())),
                        Some(format!(
                            "backend unreachable ({e}) — running with mock runtime"
                        )),
                    ),
                }
            }
        };
        runtime = rt;
        // Prefer the more specific fallback note (core → backend → mock).
        startup_status = core_note.or(note);
    }

    // Restore the terminal on panic before the default hook prints the message.
    let alt = args.alt_screen;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(alt, true);
        default_hook(info);
    }));

    let guard = TermGuard::setup(args.alt_screen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Pre-app login screen: runs inside the same alt-screen session and resolves
    // to a token (→ backend runtime), the mock, or a clean quit.
    if let Some(base_url) = need_login {
        match run_login_screen(&mut terminal, base_url.clone()).await? {
            LoginOutcome::Quit => {
                drop(guard);
                return Ok(());
            }
            LoginOutcome::Mock => {
                runtime = Some(Arc::new(MockRuntime::demo()));
                startup_status = Some("continuing offline with the mock runtime".to_string());
            }
            LoginOutcome::Token(jwt) => {
                let client = MedullaClient::new(base_url.clone(), jwt.clone());
                match BackendRuntime::connect(client).await {
                    Ok(rt) => {
                        runtime = Some(Arc::new(rt));
                        startup_status = save_credentials(&home, &base_url, &jwt);
                    }
                    Err(e) => {
                        runtime = Some(Arc::new(MockRuntime::demo()));
                        startup_status = Some(format!(
                            "backend connect failed ({e}) — running with mock runtime"
                        ));
                    }
                }
            }
        }
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

    let result = run(
        &mut terminal,
        runtime.clone(),
        loaded,
        startup_status.or(tinyplace_status),
        tinyplace_obs,
        home.join("config.toml"),
    )
    .await;

    // Explicit teardown (the guard also runs on drop / panic).
    drop(guard);
    drop(tinyplace_service); // aborts the background loops.
    runtime.shutdown().await.ok();
    result
}

/// Persist a freshly-obtained JWT under the Medulla home. Returns `None` on
/// success or a non-fatal notice string on failure (the app still proceeds).
fn save_credentials(home: &std::path::Path, base_url: &str, jwt: &str) -> Option<String> {
    let store = CredentialStore::at_home(home);
    match store.save(&Credentials {
        base_url: base_url.to_string(),
        jwt: jwt.to_string(),
    }) {
        Ok(()) => None,
        Err(e) => Some(format!("logged in, but saving credentials failed ({e})")),
    }
}

/// The pre-app login loop: draw the [`LoginScreen`], route keys to async tasks,
/// and fold their events back in until the screen reaches an outcome.
async fn run_login_screen(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    base_url: String,
) -> anyhow::Result<LoginOutcome> {
    let mut screen = LoginScreen::new(base_url.clone());
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LoginEvent>();
    let mut loopback_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        terminal.draw(|f| screen.draw(f))?;
        if let Some(outcome) = screen.outcome() {
            if let Some(h) = loopback_task.take() {
                h.abort();
            }
            return Ok(outcome);
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind != KeyEventKind::Release {
                        if let Some(cmd) = screen.handle_key(key) {
                            dispatch_login_cmd(cmd, &base_url, &tx, &mut loopback_task);
                        }
                    }
                }
            }
            Some(ev) = rx.recv() => screen.apply(ev),
            _ = tick.tick() => screen.tick(),
        }
    }
}

/// Spawn the async work a [`LoginCmd`] requires and stream results back as
/// [`LoginEvent`]s.
fn dispatch_login_cmd(
    cmd: LoginCmd,
    base_url: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<LoginEvent>,
    loopback_task: &mut Option<tokio::task::JoinHandle<()>>,
) {
    match cmd {
        LoginCmd::StartLoopback { base_url, provider } => {
            let tx = tx.clone();
            let handle = tokio::spawn(async move {
                match start_loopback(&base_url, provider).await {
                    Ok(lb) => {
                        let _ = tx.send(LoginEvent::LoopbackStarted {
                            url: lb.login_url().to_string(),
                            port: lb.port(),
                        });
                        open_browser(lb.login_url());
                        match lb.await_callback(DEFAULT_LOGIN_TIMEOUT).await {
                            Ok(jwt) => {
                                let _ = tx.send(LoginEvent::CallbackToken(jwt.clone()));
                                verify_and_emit(&base_url, jwt, &tx).await;
                            }
                            Err(e) => {
                                let _ = tx.send(LoginEvent::CallbackError(e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(LoginEvent::CallbackError(e.to_string()));
                    }
                }
            });
            if let Some(old) = loopback_task.replace(handle) {
                old.abort();
            }
        }
        LoginCmd::CancelLoopback => {
            if let Some(h) = loopback_task.take() {
                h.abort();
            }
        }
        LoginCmd::SubmitToken(token) => {
            let base = base_url.to_string();
            let tx = tx.clone();
            tokio::spawn(async move {
                let jwt = if is_one_time_login_token(&token) {
                    let client = MedullaClient::new(base.clone(), String::new());
                    match client.consume_login_token(token).await {
                        Ok(j) => j,
                        Err(e) => {
                            let _ = tx.send(LoginEvent::VerifyFailed(format!(
                                "login token redemption failed: {e}"
                            )));
                            return;
                        }
                    }
                } else {
                    token
                };
                verify_and_emit(&base, jwt, &tx).await;
            });
        }
    }
}

/// Verify a JWT via `me()` and emit the matching [`LoginEvent`].
async fn verify_and_emit(
    base_url: &str,
    jwt: String,
    tx: &tokio::sync::mpsc::UnboundedSender<LoginEvent>,
) {
    let client = MedullaClient::new(base_url.to_string(), jwt.clone());
    match client.me().await {
        Ok(me) => {
            let _ = tx.send(LoginEvent::Verified {
                who: describe_me(&me),
                jwt,
            });
        }
        Err(e) => {
            let _ = tx.send(LoginEvent::VerifyFailed(format!(
                "verification failed: {e}"
            )));
        }
    }
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    runtime: Arc<dyn Runtime>,
    loaded: medulla::config::LoadedConfig,
    startup_status: Option<String>,
    tinyplace_obs: Option<
        Arc<std::sync::Mutex<medulla::tinyplace_support::service::TinyplaceObservation>>,
    >,
    config_path: std::path::PathBuf,
) -> anyhow::Result<()> {
    let mut app = App::new(runtime.clone(), loaded);
    app.set_config_path(config_path);
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

    // Background release-update checker ("automated cron"): first probe ~10s
    // after startup, then every 6h. A newer version surfaces as a persistent
    // header banner. Disabled via `[update] check = false` or
    // `MEDULLA_NO_UPDATE_CHECK`.
    spawn_update_checker(&app.loaded, &msg_tx);

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
                    AppMsg::UsageLoaded(data) => app.set_account_usage(data),
                    AppMsg::OpenResume(chats) => app.open_resume(chats),
                    AppMsg::Resumed(s) => {
                        app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap_or(1);
                        app.refresh_snapshot();
                        app.set_status(s);
                    }
                    AppMsg::MemoryLoaded { status, directives } => {
                        app.set_memory_loaded(status, directives);
                    }
                    AppMsg::MemoryResults { hits, query } => {
                        let n = hits.len();
                        app.set_memory_results(hits, query);
                        app.set_status(format!("Memory · {n} hit(s)"));
                    }
                    AppMsg::UpdateAvailable(notice) => {
                        app.set_update_notice(notice.clone());
                        app.set_status(notice);
                        app.refresh_snapshot();
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

/// Spawn the periodic release-update checker unless disabled by config/env. It
/// waits ~10s, checks once, then rechecks every 6h, sending [`AppMsg::UpdateAvailable`]
/// on a newer release.
fn spawn_update_checker(
    loaded: &medulla::config::LoadedConfig,
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AppMsg>,
) {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    if !loaded.config.update.enabled(&env) {
        return;
    }
    let tx = msg_tx.clone();
    tokio::spawn(async move {
        let url = medulla::update::update_url();
        let current = env!("CARGO_PKG_VERSION");
        let mut first = true;
        loop {
            let delay = if first {
                Duration::from_secs(10)
            } else {
                Duration::from_secs(6 * 60 * 60)
            };
            first = false;
            tokio::time::sleep(delay).await;
            if let Ok(Some(info)) = medulla::update::check_for_update(&url, current).await {
                let notice = format!("update v{} available — run `medulla update`", info.version);
                if tx.send(AppMsg::UpdateAvailable(notice)).is_err() {
                    break; // app exited
                }
            }
        }
    });
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
        Cmd::LoadUsage => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::spawn(async move {
                match rt.team_usage().await {
                    Ok(data) => {
                        let _ = tx.send(AppMsg::UsageLoaded(data));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMsg::Status(format!("usage fetch failed: {e}")));
                    }
                }
            });
        }
        // Memory queries are synchronous but touch SQLite, so run them off the UI
        // thread via `spawn_blocking` and report back over `AppMsg`.
        Cmd::LoadMemory => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let status = rt.memory_status();
                let directives = rt.memory_directives();
                let _ = tx.send(AppMsg::MemoryLoaded { status, directives });
            });
        }
        Cmd::SearchMemory(query) => {
            let rt = runtime.clone();
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let status = rt.memory_status();
                let directives = rt.memory_directives();
                let hits = rt.memory_search(query.clone(), None, 20);
                let _ = tx.send(AppMsg::MemoryLoaded { status, directives });
                let _ = tx.send(AppMsg::MemoryResults { hits, query });
            });
        }
    }
}
