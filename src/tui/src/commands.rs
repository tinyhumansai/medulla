//! Non-TUI subcommand runners and the pre-app login screen driver.
//!
//! Holds the CLI verbs that do not enter the ratatui app — `medulla login`,
//! `logout`, and `memory` — plus the credential persistence helper and the
//! interactive login-screen loop the TUI runs before selecting a runtime. Each
//! runner parses its own args, loads config, performs its work, and returns an
//! `anyhow::Result`.

use std::io::Stdout;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::auth::{
    describe_me, is_one_time_login_token, open_browser, run_login_flow, start_loopback,
    CredentialStore, Credentials, LoopbackConfig, DEFAULT_LOGIN_TIMEOUT,
};
use medulla::client::MedullaClient;
use medulla::config::load_config;
use medulla_tui::cli::{
    parse_commit_args, parse_init_args, parse_login_args, parse_memory_args, LoginArgs,
    MemoryAction,
};
use medulla_tui::ui::login::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};

/// `medulla login`: obtain a JWT (loopback OAuth or a one-time token), verify it
/// with `/auth/me`, and persist it to the credential store.
pub(crate) async fn run_login(args: &[String]) -> anyhow::Result<()> {
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
pub(crate) fn run_logout() -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let store = CredentialStore::at_home(&medulla::home::medulla_home(&env));
    store.clear()?;
    println!("Logged out ({} cleared).", store.path().display());
    Ok(())
}

/// `medulla hub`: run the orchestrator hub — bridge the hosted backend brain to
/// tiny.place worker daemons. Reads the backend JWT from saved credentials and
/// the worker roster from `MEDULLA_TINYPLACE_PEER` / `MEDULLA_HUB_WORKERS`.
pub(crate) async fn run_hub(_args: &[String]) -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let home = medulla::home::medulla_home(&env);
    // The standalone `medulla hub` owns its terminal, so stderr is right there.
    match crate::hub_relay::build_hub_config_with_log(&env, &home, medulla::hub::stderr_log()) {
        Some(config) => medulla::hub::run_hub(config).await,
        None => anyhow::bail!(
            "hub: nothing to run — set MEDULLA_TINYPLACE_PEER (or MEDULLA_HUB_WORKERS) and run \
             `medulla login` first"
        ),
    }
}

/// `medulla memory <status|ingest|backfill|compile|search <query>>`: manage the
/// persona-memory layer from the command line.
pub(crate) async fn run_memory(args: &[String]) -> anyhow::Result<()> {
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

/// `medulla init [dir]` — author a `MEDULLA.md` workspace profile.
///
/// Reads the directory's `AGENTS.md` / `CLAUDE.md` / `README.md` and asks the
/// configured model to distil them into a short, routing-oriented profile, then
/// writes it for the operator to review. Falls back to an editable stub when
/// `--offline` is set or no model is reachable, so `init` always leaves a valid
/// file behind.
pub(crate) async fn run_init(args: &[String]) -> anyhow::Result<()> {
    let parsed = parse_init_args(args);
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dir = parsed
        .dir
        .as_ref()
        .map_or_else(|| cwd.clone(), |d| cwd.join(d));

    // Resolve the same backend/model settings memory ingest uses, so one login
    // (or one OPENROUTER_API_KEY) serves both surfaces.
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;
    let settings = medulla::memory::env::resolve_with_backend(
        loaded.config.memory.as_ref(),
        &loaded.config.backend,
        &env,
        &medulla::home::medulla_home(&env),
    );

    if !parsed.offline && !medulla::init::model_available(&settings) {
        eprintln!(
            "medulla init: no model available (run `medulla login` or set OPENROUTER_API_KEY) — writing an editable stub"
        );
    }

    let outcome =
        medulla::init::init_workspace_with_settings(&dir, &settings, parsed.offline, parsed.force)
            .await?;

    if outcome.drafted {
        println!(
            "Wrote {} (drafted from {})",
            outcome.path.display(),
            outcome.sources.join(", ")
        );
        println!("Review it — the summary is what the orchestrator reads.");
    } else {
        println!("Wrote {} (stub)", outcome.path.display());
        println!("Fill in the summary and routing hints, then it is ready to use.");
    }
    Ok(())
}

/// `medulla commit` — create a conventional commit from exactly named paths.
pub(crate) fn run_commit(args: &[String]) -> anyhow::Result<()> {
    let parsed = parse_commit_args(args).map_err(anyhow::Error::msg)?;
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let workspace = cwd.join(&parsed.workspace);
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;
    let paths = parsed
        .paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect::<Vec<_>>();
    let outcome = medulla::workspace::commit(
        &workspace,
        &paths,
        &parsed.subject,
        &medulla::workspace::CommitOptions {
            body: parsed.body,
            shared_path_denylist: loaded.config.workflow.shared_path_denylist,
            allow_shared: parsed.allow_shared,
        },
    )?;
    println!(
        "{} {} ({} path{})",
        outcome.short_id,
        outcome.subject,
        outcome.paths.len(),
        if outcome.paths.len() == 1 { "" } else { "s" }
    );
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

/// Persist a freshly-obtained JWT under the Medulla home. Returns `None` on
/// success or a non-fatal notice string on failure (the app still proceeds).
pub(crate) fn save_credentials(home: &Path, base_url: &str, jwt: &str) -> Option<String> {
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
pub(crate) async fn run_login_screen(
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
        // Best-effort, like the login-URL opener: a browser that refuses to
        // launch must not interrupt a sign-in the user is part-way through.
        LoginCmd::OpenUrl(url) => open_browser(&url),
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
