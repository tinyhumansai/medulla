//! TUI startup: config load, runtime selection, terminal setup, the optional
//! pre-app login screen, and background-service wiring before handing off to the
//! [`crate::event_loop::run`] loop.
//!
//! [`run_tui`] implements the runtime-selection order (core socket → backend
//! token → login screen → mock), installs the panic-safe terminal guard, starts
//! the optional tiny.place presence service, runs the event loop, and tears
//! everything down on exit.

use std::io::{self, IsTerminal};
use std::sync::Arc;

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::auth::{missing_token_note, resolve_backend_token, CredentialStore};
use medulla::client::MedullaClient;
use medulla::config::load_config;
use medulla::runtime::backend::BackendRuntime;
#[cfg(unix)]
use medulla::runtime::core::CoreRuntime;
#[cfg(unix)]
use medulla::runtime::core_client::CoreClient;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
#[cfg(unix)]
use medulla_tui::cli::core_socket_plan;
use medulla_tui::cli::{parse_tui_args, CorePlan};
use medulla_tui::ui::login::LoginOutcome;
use medulla_tui::ui::welcome::{format_usd, run_welcome_ui, WelcomeOutcome};

use crate::commands::{run_login_screen, save_credentials};
use crate::event_loop::run;
use crate::terminal::{restore, TermGuard};

/// Parse TUI args, select a runtime, set up the terminal, optionally run the
/// login screen, start background services, and drive the event loop to exit.
pub(crate) async fn run_tui(raw: &[String]) -> anyhow::Result<()> {
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
    // Kept alongside the runtime so the first-run welcome flow can talk to the
    // backend directly. `None` whenever we end up on core or the mock, which is
    // exactly when the welcome flow must not run.
    let mut backend_client: Option<MedullaClient> = None;

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
                match BackendRuntime::connect(client.clone()).await {
                    Ok(rt) => {
                        backend_client = Some(client);
                        (Some(Arc::new(rt)), None)
                    }
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
                    Ok(_) => match BackendRuntime::connect(client.clone()).await {
                        Ok(rt) => {
                            backend_client = Some(client);
                            (Some(Arc::new(rt)), None)
                        }
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
                match BackendRuntime::connect(client.clone()).await {
                    Ok(rt) => {
                        runtime = Some(Arc::new(rt));
                        backend_client = Some(client);
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

    // First-run welcome: offer promotional credit for sharing coding-agent
    // history. Gated locally by `[onboarding] welcomeCompleted` so a returning
    // user is never re-prompted; the backend independently refuses a second
    // grant. Only runs against a real authenticated backend — never on the mock.
    let config_path = home.join("config.toml");
    // Onboarding state must be written back to the file it is *read* from. With
    // an explicit --config, discovery is bypassed entirely, so persisting to the
    // home config would leave the flag unread and the flow would reappear every
    // launch.
    let onboarding_path = args
        .config
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config_path.clone());
    if !loaded.config.onboarding.welcome_completed {
        if let Some(client) = &backend_client {
            match run_welcome_ui(&mut terminal, client, env.clone()).await {
                // Completing *or* declining settles onboarding: a user who said
                // no should not be asked again on every launch.
                Ok(outcome @ (WelcomeOutcome::Completed { .. } | WelcomeOutcome::Skipped)) => {
                    if let Err(e) =
                        medulla::config::persist_welcome_completed(&onboarding_path, true)
                    {
                        startup_status = Some(format!("could not save onboarding state ({e})"));
                    }
                    if let WelcomeOutcome::Completed { awarded_usd, .. } = outcome {
                        if awarded_usd > 0.0 {
                            startup_status = Some(format!(
                                "{} in free credits added to your balance",
                                format_usd(awarded_usd)
                            ));
                        }
                    }
                }
                // Nothing was found to share, or the flow never settled (status
                // check or claim failed). Leaving the flag unset keeps the offer
                // available rather than burning it on an empty scan or a
                // transient backend error.
                Ok(WelcomeOutcome::NothingToShare | WelcomeOutcome::Unavailable) => {}
                Err(_) => {}
            }
        }
    }

    // Optional background tiny.place presence service (observational only): keep
    // the identity online, auto-accept peer contacts, and poll peer presence,
    // surfacing all of it into the Overview panel and Agents lanes.
    let mut tinyplace_status: Option<String> = None;
    let tinyplace_service = match &loaded.config.tinyplace {
        Some(tp) => match medulla::tinyplace::service::TinyplaceService::start(tp) {
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
        config_path,
    )
    .await;

    // Explicit teardown (the guard also runs on drop / panic).
    drop(guard);
    drop(tinyplace_service); // aborts the background loops.
    runtime.shutdown().await.ok();
    result
}
