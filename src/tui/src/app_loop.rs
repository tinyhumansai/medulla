//! TUI startup: config load, runtime selection, terminal setup, the optional
//! pre-app login screen, and background-service wiring before handing off to the
//! [`crate::event_loop::run`] loop.
//!
//! [`run_tui`] selects a runtime — the embedded OpenHuman core, signing the
//! operator in through the pre-app login screen when the core has no session —
//! installs the panic-safe terminal guard, starts
//! the optional tiny.place presence service, runs the event loop, and tears
//! everything down on exit.

use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex};

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::config::load_config;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::cli::parse_tui_args;

use crate::commands::run_login_screen;
use crate::event_loop::{run, SessionExit, SessionWiring};
use crate::terminal::{restore, TermGuard};
use medulla_tui::ui::login::LoginOutcome;

/// Wrap a signed-in core in its [`Runtime`] and start it producing.
///
/// Both the already-signed-in path and the just-logged-in path need the same
/// two priming steps, and getting either wrong is invisible until the UI sits
/// empty or inert.
async fn core_runtime(
    core: Arc<medulla::core_host::EmbeddedCore>,
    hub: crate::hub_relay::HubSlot,
) -> Arc<dyn Runtime> {
    let rt = medulla::runtime::openhuman::OpenHumanRuntime::with_hub(core, hub);
    // First fetch before the UI paints, so the initial frame shows real state
    // rather than an empty one that fills in a beat later.
    rt.refresh().await;
    let rt = Arc::new(rt);
    // Start replaying events before the UI paints. Without this a submitted turn
    // is accepted and nothing ever returns to the transcript, which reads as a
    // hang rather than a missing loop.
    rt.spawn_poll_loop();
    rt
}

/// Read the core's session once: the bearer for backend-facing services, and
/// who it belongs to for the Account subpage.
///
/// `base_url` comes from the loaded Medulla config rather than the core — they
/// address the same deployment by construction, and the core exposes no RPC for
/// the URL it resolved. A failed read degrades to signed out: the surfaces that
/// take this simply go without a backend, which is exactly what they do for a
/// signed-out host.
async fn session_of(
    core: &medulla::core_host::EmbeddedCore,
    base_url: &str,
) -> (
    Option<medulla::auth::Credentials>,
    Option<medulla::core_host::auth::AuthState>,
) {
    // A failed read is indistinguishable from signed out for every consumer, and
    // this runs before the terminal guard is up — printing here would land on
    // the screen the login flow is about to take over.
    let jwt = medulla::core_host::auth::session_token(core)
        .await
        .unwrap_or_default();
    let account = medulla::core_host::auth::state(core).await.ok();
    let session = jwt.map(|jwt| medulla::auth::Credentials {
        base_url: base_url.to_string(),
        jwt,
    });
    (session, account)
}

/// Run the login screen again for an already-booted core and store the result.
///
/// `Ok(true)` when a new session was stored and the app should start another
/// session; `Ok(false)` when the operator quit from the screen. A core that
/// rejects the freshly verified token is an error rather than a silent return to
/// the screen: the token passed `/auth/me`, so a refusal here means the core and
/// the login flow disagree about which deployment they are talking to, and
/// looping would just ask for the same token again.
async fn relogin(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    core: &medulla::core_host::EmbeddedCore,
    base_url: &str,
) -> anyhow::Result<bool> {
    match run_login_screen(terminal, base_url.to_string()).await? {
        LoginOutcome::Quit => Ok(false),
        LoginOutcome::Token(jwt) => {
            medulla::core_host::auth::store_session(core, &jwt)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("signed in, but the core rejected the session: {e}")
                })?;
            Ok(true)
        }
    }
}

/// Parse TUI args, select a runtime, set up the terminal, optionally run the
/// login screen, start background services, and drive the event loop to exit.
pub(crate) async fn run_tui(raw: &[String]) -> anyhow::Result<()> {
    let args = parse_tui_args(raw);

    if !io::stdout().is_terminal() {
        eprintln!("medulla-tui requires an interactive terminal (TTY).");
        std::process::exit(1);
    }

    // An explicit `--config` is recorded in this process's own environment
    // before anything spawns off it, so every subprocess that later re-reads
    // config (the `medulla workflow mcp` tool server, an ACP harness advertised
    // its MCP servers) inherits the same choice instead of silently
    // rediscovering a different one from its own `cwd`. See
    // `medulla::config::CONFIG_PATH_ENV`.
    if let Some(path) = args.config.as_deref() {
        std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
    }
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(args.config.as_deref(), &env, &cwd)?;
    let home = medulla::home::medulla_home(&env);

    // Bind the embedded core's state directory to this process's Medulla home
    // BEFORE anything can construct the core. Both resolve state independently
    // otherwise, which would silently route memory, flows, and credentials into
    // the developer's real `~/.openhuman` even on a `MEDULLA_HOME=$(mktemp -d)`
    // scratch run — the recipe that exists precisely to avoid that. Cheap, and
    // a no-op when the operator set `OPENHUMAN_WORKSPACE` themselves.
    medulla::core_host::bind_workspace(&env, &home);
    // Same binding discipline for the backend: the core must dial the endpoint
    // this host was configured for, or a staging/self-hosted install verifies a
    // token against one deployment and stores it against another.
    medulla::core_host::bind_medulla_base_url(&env, &loaded.config.backend.base_url);
    // The core's own backend client needs the same treatment: it resolves
    // `/auth/me` from `BACKEND_URL`, which defaults to production and knows
    // nothing about `MEDULLA_STAGING`, so the in-app login screen would verify a
    // staging token and then have the core hand it to production to validate.
    medulla::core_host::bind_backend_api_url(&env, &loaded.config.backend.base_url);

    // Runtime selection.
    //
    // The embedded core is THE runtime: no token
    // lookup, no fallback chain, because there is nothing to fall back from —
    // the core runs in this process. A core with no app session is not a reason
    // to run something else, it is a reason to sign in, so it routes to the
    // pre-app login screen below. `--mock` is still honoured ahead of all of
    // this: the demo runtime exists for tests and offline demos, and reaching it
    // requires asking for it.
    let mut runtime: Option<Arc<dyn Runtime>> = None;
    let mut startup_status: Option<String> = None;
    // A booted-but-signed-out core, held across terminal setup: the login screen
    // needs the alt screen, which is not up yet at selection time.
    let mut pending_core: Option<medulla::core_host::EmbeddedCore> = None;
    let mut need_login: Option<String> = None;
    // The signed-in session, resolved once from the core. Everything that needs
    // a backend bearer — the hub uplink, the Account subpage — takes it from
    // here rather than looking it up again, so
    // no two surfaces can disagree about whether this process is signed in.
    let mut session: Option<medulla::auth::Credentials> = None;
    let mut account: Option<medulla::core_host::auth::AuthState> = None;

    // Shared hub roster slot: filled after the hub connects, read by the
    // runtime's worker surface so the Workers tab manages the hub's tiny.place
    // peers live.
    let hub_slot: crate::hub_relay::HubSlot = Arc::new(Mutex::new(None));
    // Cloned before the core is consumed: a relogin rebuilds the runtime around
    // the same in-process core rather than booting a second one.
    let mut core_arc: Option<Arc<medulla::core_host::EmbeddedCore>> = None;
    // Active workspace roots whose `MEDULLA.md` profiles ride every backend
    // session mint (`workspaceProfiles`). Roots without a profile are skipped by
    // the collector, so passing every configured workspace is safe.
    let workspace_roots: Vec<std::path::PathBuf> = loaded
        .config
        .workflow
        .workspaces
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    // The agent's read/write root. OpenHuman defaults this to
    // `~/OpenHuman/projects`, which a Medulla operator has never used — their
    // repos are the configured workspace roots. Binding the first keeps the
    // agent writing where the operator actually works. Also non-overriding.
    medulla::core_host::bind_action_dir(&env, workspace_roots.first().map(|p| p.as_path()));
    // The hub narrates itself; those lines must not reach the terminal while the
    // TUI owns the screen, so they are captured here instead.
    let hub_logs = medulla_tui::log::LogBuffer::new();
    // Persist them too: the failures worth chasing are usually noticed after the
    // fact, and an in-memory ring dies with the process.
    let log_dir = medulla_tui::log::default_log_dir(&env);
    // Held apart rather than written into `startup_status`: this runs before
    // anything else could have set one, so `get_or_insert` always won here and
    // was then overwritten by every later assignment — the line never showed on
    // any path that reported anything at all. It is the least interesting thing
    // that could be said at startup, so it belongs at the end of the fallback
    // chain, not the front.
    let log_note = hub_logs
        .attach_file(&log_dir, "orchestrator")
        .map(|path| format!("logging to {}", path.display()));

    if args.mock {
        // Explicit offline demo: skip the token lookup and the login screen
        // entirely so the TUI is drivable with no backend at all.
        runtime = Some(Arc::new(MockRuntime::demo()));
        startup_status = Some("running the offline mock runtime (--mock)".to_string());
    }
    // The embedded core, whenever it is compiled in. Deliberately ahead of the
    // old token/login chain: a host that ships the core has no reason to dial a
    // remote backend, and keeping that chain as a general fallback would mean a
    // misconfiguration silently downgrades to a different runtime with
    // different behaviour instead of surfacing itself.
    //
    // The one exception is a core that booted but has no Medulla backend to
    // talk to — no configured URL, or nobody signed in. That is not a
    // misconfiguration to surface, it is the documented credential-free start,
    // and every drive method would otherwise return the same error behind a UI
    // that looks live. It takes the offline demo, exactly as `--mock` does.
    if runtime.is_none() {
        match medulla::core_host::boot().await {
            Ok(core) => match medulla::core_host::probe_medulla(&core).await {
                medulla::core_host::Readiness::Ready => {
                    (session, account) = session_of(&core, &loaded.config.backend.base_url).await;
                    let core = Arc::new(core);
                    core_arc = Some(Arc::clone(&core));
                    runtime = Some(core_runtime(core, hub_slot.clone()).await);
                }
                // Signed out is an expected state with an obvious remedy, so it
                // is the one thing that does not end here: the core is held and
                // the login screen runs once the terminal is up.
                medulla::core_host::Readiness::SignedOut => {
                    pending_core = Some(core);
                    need_login = Some(loaded.config.backend.base_url.clone());
                }
                // No backend to reach, or the surface compiled out. Neither is
                // fixable from inside the app, and neither is a reason to start
                // a different runtime that would quietly behave differently.
                medulla::core_host::Readiness::Unusable(why) => {
                    anyhow::bail!("the embedded OpenHuman core cannot reach Medulla: {why}");
                }
            },
            Err(e) => {
                // Boot failure is fatal rather than a downgrade. The core is
                // in-process, so a failure here means a broken workspace or
                // config — conditions the operator must see and fix, not have
                // papered over by a mock that then behaves differently.
                //
                // No terminal teardown needed: this runs before `TermGuard`
                // takes over the screen, so the error reaches a normal stdout.
                anyhow::bail!("failed to start the embedded OpenHuman core: {e}");
            }
        }
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

    // Pre-app login screen: the core booted but has no app session. Runs inside
    // the alt-screen session already set up, and resolves to a signed-in core or
    // a clean quit — there is no third option, because a TUI with no runtime has
    // nothing to show.
    if let Some(base_url) = need_login.take() {
        let core = pending_core.take().expect("a core is held for the login");
        match run_login_screen(&mut terminal, base_url).await? {
            LoginOutcome::Quit => {
                drop(guard);
                return Ok(());
            }
            LoginOutcome::Token(jwt) => {
                // The core validates the JWT against the backend before writing
                // it, so a rejected token fails here rather than after startup.
                if let Err(e) = medulla::core_host::auth::store_session(&core, &jwt).await {
                    drop(guard);
                    anyhow::bail!("signed in, but the core rejected the session: {e}");
                }
                match medulla::core_host::probe_medulla(&core).await {
                    medulla::core_host::Readiness::Ready => {
                        (session, account) =
                            session_of(&core, &loaded.config.backend.base_url).await;
                        let core = Arc::new(core);
                        core_arc = Some(Arc::clone(&core));
                        runtime = Some(core_runtime(core, hub_slot.clone()).await);
                    }
                    // Stored and still unusable: the token was for a different
                    // deployment than the one the core resolves, or the backend
                    // withdrew it between the two calls.
                    other => {
                        drop(guard);
                        anyhow::bail!("signed in, but Medulla is still unreachable: {other:?}");
                    }
                }
            }
        }
    }

    // `mut` because a relogin rebuilds it around the same core.
    let mut runtime = runtime.expect("a runtime is always selected");

    // First-run welcome: offer promotional credit for sharing coding-agent
    // history. Gated locally by `[onboarding] welcomeCompleted` so a returning
    // user is never re-prompted; the backend independently refuses a second
    // grant. Only runs against a real authenticated backend — never on the mock.
    let home_config_path = home.join("config.toml");
    // Every write-back (onboarding flag, routing strategy, …) must target the
    // file whose value *wins on the next launch*, or the change is silently lost —
    // the welcome flow reappears, the saved strategy reverts. That target is:
    //   1. the explicit --config file when one was passed (discovery is bypassed);
    //   2. otherwise the highest-precedence file that actually contributed to the
    //      layered load (`sources` is ordered low → high, so `.last()`), which is
    //      the project-local `.medulla/config.toml` / `medulla.toml` when present;
    //   3. otherwise the home config (nothing was discovered to layer).
    // The welcome/credit-sharing flow ran only against an authenticated cloud
    // backend, which the embedded core replaces; it returns with the auth
    // migration rather than being reconstructed against a core that has no
    // notion of a Medulla account.
    let mut sharing = None;
    let active_config_path = args
        .config
        .as_deref()
        .map(std::path::PathBuf::from)
        .or_else(|| loaded.sources.last().map(std::path::PathBuf::from))
        .unwrap_or_else(|| home_config_path.clone());

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

    // Backend runtime only: start the orchestrator hub so the hosted brain's
    // delegated tasks reach local tiny.place workers, and fill the roster slot so
    // the Workers tab manages it live. Opt-in via `MEDULLA_TINYPLACE_PEER` /
    // `MEDULLA_HUB_WORKERS`; the session is dropped (disconnected) on exit.
    //
    // The hub is scoped to the *authenticated* session: its Socket.IO uplink
    // carries the current account's JWT and its roster handle is that account's.
    // On a relogin (below) it is torn down and re-started for the new account so
    // no worker mutation or task relay ever targets a revoked/stale session.
    // This device also *runs* the work, unless `[host].enabled = false` /
    // `MEDULLA_HOST=0`. The host binds an address on a bus the hub dispatches
    // over, so a task for this machine is delivered in-process — no relay, no
    // second identity, no contact edge between two programs on one laptop.
    //
    // Started before the hub because the hub advertises it: the roster it
    // registers with the backend has to name this host from the first moment, or
    // the orchestrator's opening move has nowhere to send work.
    let local_network = medulla::bridge::LocalBridgeNetwork::new();
    // Every harness this device runs lives here, on a real pseudo-terminal with
    // its screen parsed by an emulator. Built out here rather than inside the
    // host because both halves need it: the host's executor opens sessions in
    // it, and the Agents tab reads their screens out of it and types into them.
    // It outlives each session (a relogin rebuilds those), so a harness the
    // operator is watching is not torn down by an unrelated re-auth.
    let harness_sessions = medulla_tui::worker::pty::PtyManager::new();
    // A bad `[host]` section is reported exactly like a failed start: this
    // machine does not host, and the operator is told why. `and_then` keeps the
    // two failure kinds — unparseable config, unstartable host — on one path.
    let local_host = match crate::local_host::options_from_config(
        &loaded.config.host,
        &env,
        loaded.config.router.clone(),
        loaded.config.budget.clone(),
        Some(hub_logs.sink()),
    )
    .and_then(|options| {
        crate::local_host::start(
            &loaded.config.host,
            &env,
            &local_network,
            options,
            harness_sessions.clone(),
        )
    }) {
        Ok(host) => host,
        Err(e) => {
            // Not fatal: the orchestrator still drives remote workers. But it is
            // the difference between "nothing happens" and "nothing happens
            // *here*", so it goes on the status line rather than only the log.
            hub_logs.push(format!("host: not hosting on this device ({e})"));
            startup_status.get_or_insert(format!("not hosting on this device ({e})"));
            None
        }
    };
    if let Some(host) = &local_host {
        hub_logs.push(format!(
            "host: serving [{}] as {} in {}",
            host.providers()
                .iter()
                .map(|provider| provider.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            host.address(),
            host.workspace()
        ));
    }
    // Only meaningful with a host: with hosting off nothing runs here, so there
    // is no local screen to resolve and the Agents tab keeps its remote/
    // transcript behaviour unchanged.
    let local_harnesses =
        local_host
            .as_ref()
            .map(|host| medulla_tui::ui::harness_pane::LocalHarnesses {
                sessions: harness_sessions.clone(),
                runtime: host.runtime(),
                hub_address: medulla::hub::DEFAULT_LOCAL_HUB_ADDRESS.to_string(),
            });
    let local_dispatch = crate::hub_relay::LocalDispatch {
        network: local_network,
        hub_address: medulla::hub::DEFAULT_LOCAL_HUB_ADDRESS.to_string(),
        // Always known, even with hosting off — it is what identifies a
        // remembered local roster entry that must not be inherited.
        host_address: crate::local_host::host_address(&loaded.config.host),
        host: local_host.as_ref().map(|host| host.spec().clone()),
    };

    // Start the hub unconditionally. It used to be gated on an authenticated
    // cloud client; it reads its own credentials from the Medulla home and
    // returns `None` when there are none, so the gate only duplicated a check it
    // already makes. The hub is tiny.place/harness wiring and stays TUI-side
    // regardless of which runtime backs the session.
    let _hub_session = crate::hub_relay::start(
        &env,
        &home,
        hub_slot.clone(),
        hub_logs.clone(),
        Some(local_dispatch.clone()),
        session.as_ref(),
    )
    .await;

    // A session, and another after every logout. `run` reports `Relogin` when
    // the Account page's logout landed, and the whole point of that logout is to
    // reach the login screen — dropping the operator back to the shell instead
    // would contradict both the message it shows and the no-session startup path
    // they would then have to trigger by relaunching.
    let mut status = startup_status.or(tinyplace_status).or(log_note);
    let result = loop {
        let exit = run(
            &mut terminal,
            runtime.clone(),
            SessionWiring {
                loaded: loaded.clone(),
                startup_status: status.take(),
                tinyplace_obs: tinyplace_obs.clone(),
                config_path: active_config_path.clone(),
                medulla_home: home.clone(),
                account: account.clone(),
                sharing: sharing.take(),
                onboarding_path: active_config_path.clone(),
                host: local_host.as_ref().map(|host| host.observation()),
                harnesses: local_harnesses.clone(),
            },
        )
        .await;

        match exit {
            Ok(SessionExit::Relogin) => {
                // Only the embedded core can be signed back in; every other
                // runtime reports that it holds no session, so its logout never
                // succeeds and this arm is unreachable for it.
                if let Some(core) = core_arc.clone() {
                    match relogin(&mut terminal, &core, &loaded.config.backend.base_url).await {
                        Ok(true) => {
                            (_, account) = session_of(&core, &loaded.config.backend.base_url).await;
                            runtime = core_runtime(core, hub_slot.clone()).await;
                            continue;
                        }
                        // Quit from the login screen: the operator asked to
                        // leave, having already logged out.
                        Ok(false) => break Ok(()),
                        Err(e) => break Err(e),
                    }
                }
                break Ok(());
            }
            Ok(SessionExit::Quit) => break Ok(()),
            Err(e) => break Err(e),
        }
    };

    runtime.shutdown().await.ok();
    // Explicit teardown (the guard also runs on drop / panic).
    drop(guard);
    drop(tinyplace_service); // aborts the background loops.
    result
}
