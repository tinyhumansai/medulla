//! TUI startup: config load, runtime selection, terminal setup, the optional
//! pre-app login screen, and background-service wiring before handing off to the
//! [`crate::event_loop::run`] loop.
//!
//! [`run_tui`] selects a runtime — the embedded OpenHuman core, signing the
//! operator in through the pre-app login screen when the core has no session —
//! installs the panic-safe terminal guard, starts
//! the optional host-link presence service, runs the event loop, and tears
//! everything down on exit.

use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex};

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use medulla::config::load_config;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::cli::parse_tui_args;

use crate::event_loop::{run, SessionExit, SessionWiring};
use crate::sign_in::{
    account_is_active, relogin, sign_in_first_account, SignIn, SWITCHED_ACCOUNT_NOTICE,
};
use crate::terminal::{restore, TermGuard};

/// Wrap a signed-in core in its [`Runtime`] and start it producing.
///
/// Both the already-signed-in path and the just-logged-in path need the same
/// two priming steps, and getting either wrong is invisible until the UI sits
/// empty or inert.
async fn core_runtime(
    core: Arc<medulla::core_host::EmbeddedCore>,
    hub: crate::hub_relay::HubSlot,
    backend_base_url: &str,
) -> Arc<dyn Runtime> {
    // The backend URL rides along for the surfaces the core cannot serve — the
    // feedback board lives on the cloud deployment this host is configured for.
    let rt = medulla::runtime::openhuman::OpenHumanRuntime::with_hub(core, hub)
        .with_backend_base_url(backend_base_url);
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

/// Parse TUI args, select a runtime, set up the terminal, optionally run the
/// login screen, start background services, and drive the event loop to exit.
pub(crate) async fn run_tui(raw: &[String]) -> anyhow::Result<()> {
    let args = parse_tui_args(raw);

    validate_explicit_config(args.config.as_deref())?;

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
    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut loaded = load_config(args.config.as_deref(), &env, &cwd)?;
    prompt_for_update(&loaded.config.update, &env).await;

    // Sign in before resolving anything else, when this install has no account
    // yet. Every path below — the config that wins, the log directory, the
    // workflow store, the core's own workspace — is derived from the *account's*
    // home, so learning who the operator is has to come first or all of it lands
    // in the pre-login directory and stays there for the life of the process.
    //
    // Skipped on `--mock`, which never touches a backend, and skipped once an
    // account is recorded: from then on the ordinary boot below finds the
    // session where that account's home keeps it.
    let mut pending_jwt = None;
    if !args.mock && !account_is_active(&env) {
        match sign_in_first_account(&env, &loaded.config.backend.base_url, args.alt_screen).await? {
            // Quit from the login screen before any account exists: there is no
            // home to open and nothing to show, so this is a clean exit.
            None => return Ok(()),
            Some(jwt) => {
                // The account's own config file is a different file from the one
                // loaded a moment ago, and it is the one that wins from here on.
                loaded = load_config(args.config.as_deref(), &env, &cwd)?;
                pending_jwt = Some(jwt);
            }
        }
    }

    let home = medulla::home::medulla_home(&env);

    // Bind the embedded core's state directory to this process's Medulla home
    // BEFORE anything can construct the core. Both resolve state independently
    // otherwise, which would silently route memory, flows, and credentials into
    // the developer's real `~/.openhuman` even on a `MEDULLA_HOME=$(mktemp -d)`
    // scratch run — the recipe that exists precisely to avoid that. Cheap, and
    // a no-op when the operator set `OPENHUMAN_WORKSPACE` themselves.
    let core_workspace = medulla::core_host::bind_workspace(&env, &home);
    // Keep the subprocess snapshot in lockstep with the process environment.
    // OpenHuman's picker entry inherits this map, so its native TUI opens the
    // same persisted core and agents as Medulla instead of rediscovering the
    // developer's default ~/.openhuman in the child.
    env.insert(
        medulla::core_host::OPENHUMAN_WORKSPACE_ENV.to_string(),
        core_workspace.to_string_lossy().into_owned(),
    );
    // Same binding discipline for the backend: the core must dial the endpoint
    // this host was configured for, or a staging/self-hosted install verifies a
    // token against one deployment and stores it against another.
    let medulla_base_url =
        medulla::core_host::bind_medulla_base_url(&env, &loaded.config.backend.base_url);
    if !medulla_base_url.is_empty() {
        env.insert(
            medulla::core_host::OPENHUMAN_MEDULLA_BASE_URL_ENV.to_string(),
            medulla_base_url,
        );
    }
    // The core's own backend client needs the same treatment: it resolves
    // `/auth/me` from `BACKEND_URL`, which defaults to production and knows
    // nothing about `MEDULLA_STAGING`, so the in-app login screen would verify a
    // staging token and then have the core hand it to production to validate.
    let backend_api_url =
        medulla::core_host::bind_backend_api_url(&env, &loaded.config.backend.base_url);
    if !backend_api_url.is_empty() {
        env.insert(
            medulla::core_host::OPENHUMAN_BACKEND_URL_ENV.to_string(),
            backend_api_url,
        );
    }

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
    // runtime's worker surface so the Workers tab manages the hub's host-link
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
    if let Some(action_dir) =
        medulla::core_host::bind_action_dir(&env, workspace_roots.first().map(|p| p.as_path()))
    {
        env.insert(
            medulla::core_host::OPENHUMAN_ACTION_DIR_ENV.to_string(),
            action_dir.to_string_lossy().into_owned(),
        );
    }
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

    // User-launched Claude/Codex sessions read their fixed user skill roots,
    // unlike sessions spawned by Medulla (which receive scoped skills and MCP
    // directly). Keep the fixed roots and their MCP registrations in step on
    // every real app start. Mock runs stay hermetic and never touch the
    // operator's harness configuration.
    if !args.mock {
        let integration = crate::startup_skills::reconcile(&env, &cwd);
        for warning in integration.warnings {
            hub_logs.push(warning);
        }
        if let Some(notice) = integration.notice {
            startup_status.get_or_insert(notice);
        }
    }

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
        match medulla::core_host::boot_with_hooks(&loaded.config.hooks).await {
            Ok(core) => {
                // A token from the sign-in gate above: the core now exists, and
                // it was booted against the home the gate chose, so this is the
                // first moment the session can be stored where it belongs.
                if let Some(jwt) = pending_jwt.take() {
                    if let Err(e) = medulla::core_host::auth::store_session(&core, &jwt).await {
                        anyhow::bail!("signed in, but the core rejected the session: {e}");
                    }
                }
                match medulla::core_host::probe_medulla(&core).await {
                    medulla::core_host::Readiness::Ready => {
                        (session, account) =
                            session_of(&core, &loaded.config.backend.base_url).await;
                        let core = Arc::new(core);
                        // Donated before anything else can want one. A workflow
                        // `agent` node that names `harness: openhuman` runs
                        // deep inside the engine with no way to be handed a
                        // core down the call stack, so it takes the process's
                        // shared one — and without this it would boot a second
                        // core beside the one the operator is looking at, with
                        // its own scheduler writing the same memory database.
                        // Idempotent: the sign-in path below reaches here too,
                        // and the first core installed is the one that stays.
                        medulla::core_host::shared::install(Arc::clone(&core));
                        core_arc = Some(Arc::clone(&core));
                        runtime = Some(
                            core_runtime(core, hub_slot.clone(), &loaded.config.backend.base_url)
                                .await,
                        );
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
                }
            }
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
        // Same flow as a mid-session relogin, and deliberately the same code: an
        // install with an account whose session went missing is signing in
        // against an already-booted core, which is exactly what that is. Signing
        // in as somebody else stores nothing here — the account check runs
        // before the write, and the core validates the JWT before persisting it,
        // so a rejected token fails here rather than after startup.
        match relogin(&mut terminal, &core, &env, &base_url).await {
            Ok(SignIn::Quit) => {
                drop(guard);
                return Ok(());
            }
            Ok(SignIn::SwitchedAccount(notice)) => {
                drop(guard);
                if let Some(notice) = notice {
                    eprintln!("{notice}");
                }
                println!("{SWITCHED_ACCOUNT_NOTICE}");
                return Ok(());
            }
            Err(e) => {
                drop(guard);
                return Err(e);
            }
            Ok(SignIn::SameAccount) => {
                match medulla::core_host::probe_medulla(&core).await {
                    medulla::core_host::Readiness::Ready => {
                        (session, account) =
                            session_of(&core, &loaded.config.backend.base_url).await;
                        let core = Arc::new(core);
                        // Donated before anything else can want one. A workflow
                        // `agent` node that names `harness: openhuman` runs
                        // deep inside the engine with no way to be handed a
                        // core down the call stack, so it takes the process's
                        // shared one — and without this it would boot a second
                        // core beside the one the operator is looking at, with
                        // its own scheduler writing the same memory database.
                        // Idempotent: the sign-in path below reaches here too,
                        // and the first core installed is the one that stays.
                        medulla::core_host::shared::install(Arc::clone(&core));
                        core_arc = Some(Arc::clone(&core));
                        runtime = Some(
                            core_runtime(core, hub_slot.clone(), &loaded.config.backend.base_url)
                                .await,
                        );
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
    // Set when a relogin lands on a different account than this process runs as.
    // Reported after the terminal is restored, not while the app owns the screen.
    let mut account_switched: Option<Option<String>> = None;

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
    // Hooks cannot follow `active_config_path` to a project-local layer:
    // `load_config` strips `[[hooks]]` from every layer but an explicit
    // `--config` file and the user-global config, so a hook saved to the
    // project-local file would show "saved" and vanish on the next launch.
    // An explicit `--config` is fully trusted (discovery, and the strip, never
    // run), so it is honored here exactly as `active_config_path` honors it;
    // otherwise hooks always target the user-global file, whatever layer other
    // settings resolved to.
    let hooks_config_path = args
        .config
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home_config_path.clone());

    // Optional background host-link service (observational only): keep per-peer
    // liveness current and surface it into the Overview panel and Agents lanes.
    let mut link_status: Option<String> = None;
    let link_config = loaded.config.link.clone().unwrap_or_else(|| {
        medulla::config::default_link_config(&env, &loaded.config.backend.base_url)
    });
    let link_service = match medulla::protocol::service::LinkService::start(&link_config).await {
        Ok(service) => Some(service),
        Err(e) => {
            link_status = Some(format!("host link unavailable ({e})"));
            None
        }
    };
    let link_obs = link_service.as_ref().map(|s| s.observation());

    // Backend runtime only: start the orchestrator hub so the hosted brain's
    // delegated tasks reach linked hosts, and fill the roster slot so
    // the Workers tab manages it live. Opt-in via `MEDULLA_LINK_PEER` /
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
    let custom_harnesses = medulla::config::load_layered_custom_harnesses(&loaded.sources)
        .unwrap_or_else(|error| {
            hub_logs.push(format!("custom harnesses: cannot load ({error})"));
            Vec::new()
        });
    let local_hosts = match crate::local_host::options_from_config_with_custom_and_hooks(
        &loaded.config.host,
        &env,
        loaded.config.router.clone(),
        loaded.config.budget.clone(),
        Some(hub_logs.sink()),
        &custom_harnesses,
        &crate::local_host::LaunchPolicy {
            attribution: loaded.config.attribution.commit,
            hooks: loaded.config.hooks.clone(),
        },
    )
    .map(|options| {
        crate::local_host::start_all(
            &loaded.config.host,
            &loaded.config.hosts,
            &env,
            &local_network,
            options,
            harness_sessions.clone(),
            &loaded.config.fleet.agent_declarations,
        )
    }) {
        Ok((hosts, problems)) => {
            for problem in problems {
                // Reported one by one, in the words `start_all` used: one
                // mistyped directory costs that host, and one misplaced agent
                // declaration costs that declaration — neither costs hosting on
                // this device, and the operator needs to know *which* thing was
                // dropped rather than that something was. Wrapping every one of
                // them in "not hosting one of this device's directories" said
                // the wrong thing about all but the first kind.
                hub_logs.push(format!("host: {problem}"));
                startup_status.get_or_insert(problem);
            }
            hosts
        }
        Err(e) => {
            // Not fatal: the orchestrator still drives remote workers. But it is
            // the difference between "nothing happens" and "nothing happens
            // *here*", so it goes on the status line rather than only the log.
            hub_logs.push(format!("host: not hosting on this device ({e})"));
            startup_status.get_or_insert(format!("not hosting on this device ({e})"));
            Vec::new()
        }
    };
    for host in &local_hosts {
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
    let hosting = !local_hosts.is_empty();
    let host_runtimes = std::sync::Arc::new(std::sync::Mutex::new(
        local_hosts
            .iter()
            .map(|host| host.runtime())
            .collect::<Vec<_>>(),
    ));
    // The started hosts live here for the session: dropping a `LocalHost` stops
    // it, so this list is what keeps them running — and it is shared so a host
    // added later joins the same list rather than a copy of it.
    // `workspace` and `providers` stay singular: they are the default a
    // *manually started* harness inherits, and the primary is the right default
    // for that. Only task resolution needs every host, which is `runtimes`.
    // Read before the list moves into the shared handle below.
    let primary_defaults = local_hosts.first().map(|primary| {
        let providers = medulla::daemon::providers::detect_providers(&env, None, None);
        let presets = available_primary_presets(&custom_harnesses, primary.address(), &providers);
        (primary.workspace().to_string(), providers, presets)
    });
    let started_hosts = std::sync::Arc::new(std::sync::Mutex::new(local_hosts));
    let local_sessions = primary_defaults.map(|(workspace, providers, custom_harnesses)| {
        medulla_tui::ui::harness_pane::LocalSessions {
            sessions: harness_sessions.clone(),
            runtimes: host_runtimes.clone(),
            hub_address: medulla::hub::DEFAULT_LOCAL_HUB_ADDRESS.to_string(),
            env: env.clone(),
            workspace,
            providers,
            custom_harnesses,
            router: loaded.config.router.clone(),
            attribution: loaded.config.attribution.commit,
            hooks: loaded.config.hooks.clone(),
            log: Some(hub_logs.sink()),
        }
    });
    // Shared with the hub's roster filter and appended to by the spawner, so a
    // host added mid-session is recognised as device-local the next time the
    // roster is saved rather than being remembered as a remote peer.
    let declared_local_hosts = std::sync::Arc::new(std::sync::Mutex::new(
        crate::local_host::all_local_hosts(&loaded.config.host, &loaded.config.hosts),
    ));
    // Only meaningful while this device hosts: with hosting off there are no
    // host options to read declared harnesses from.
    let local_host_harnesses = hosting
        .then(|| {
            crate::local_host::options_from_config_with_custom_and_hooks(
                &loaded.config.host,
                &env,
                loaded.config.router.clone(),
                loaded.config.budget.clone(),
                Some(hub_logs.sink()),
                &custom_harnesses,
                &crate::local_host::LaunchPolicy {
                    attribution: loaded.config.attribution.commit,
                    hooks: loaded.config.hooks.clone(),
                },
            )
            .ok()
            .map(crate::local_host::LocalHostHarnesses::new)
        })
        .flatten();
    let local_dispatch = crate::hub_relay::LocalDispatch {
        network: local_network,
        hub_address: medulla::hub::DEFAULT_LOCAL_HUB_ADDRESS.to_string(),
        // Always known, even with hosting off — it is what identifies a
        // remembered local roster entry that must not be inherited.
        local_hosts: declared_local_hosts,
        // Flattened: a host contributes one entry per agent declared on it, not
        // one entry standing in for the machine.
        hosts: started_hosts
            .lock()
            .expect("started hosts")
            .iter()
            .flat_map(|host| host.specs().to_vec())
            .collect(),
    };

    // Start the hub unconditionally. It used to be gated on an authenticated
    // cloud client; it reads its own credentials from the Medulla home and
    // returns `None` when there are none, so the gate only duplicated a check it
    // already makes. The hub is host-link/harness wiring and stays TUI-side
    // regardless of which runtime backs the session.
    // Held (not read) for the rest of this scope: dropping it early would tear
    // the hub session down. Whether it started is no longer a gate on the
    // control plane below — see that call's doc comment.
    let _hub_session = crate::hub_relay::start(
        &env,
        &home,
        hub_slot.clone(),
        hub_logs.clone(),
        Some(local_dispatch.clone()),
        session.as_ref(),
        crate::hub_relay::StartupConfig {
            // The roles a worker can be toggled on for. Read from the same
            // layered config the Agent Templates page shows.
            agent_templates: loaded.config.fleet.agent_templates.clone(),
            link: link_service
                .as_ref()
                .map(|service| crate::hub_relay::ResolvedLink {
                    config: link_config.clone(),
                    handle: service.link().clone(),
                }),
        },
    )
    .await;

    // Every harness Medulla launches reports its lifecycle here, through the
    // hooks Medulla installs into it. Created before the control plane because
    // that is what writes into it, and shared with the app, which reads it.
    let hook_log = medulla::harness_hooks::HookEventLog::new();
    // The attention poller reads a `Notification` report off this same log to
    // blink a claude session that is waiting on the operator — the one cue the
    // screen scraper cannot always name. Set before any session opens, so the
    // poller (spawned per session) never reads an unset log.
    harness_sessions.set_hook_log(hook_log.clone());

    // Bound once, here, and held for the whole process: the socket belongs to
    // this process rather than to a login session, and rebinding inside the
    // relogin loop below would race this process's own live socket. The server
    // reads the hub slot per request, so a relogin that refills that slot is
    // picked up with no rebind. See `control_plane::startup` for what it
    // resolves and why the registry it hands back is shared with every session.
    let control_plane = crate::control_plane::startup::start(
        &env,
        &loaded.config,
        hub_slot.clone(),
        &local_dispatch,
        hook_log.clone(),
        &hub_logs,
    )
    .await;
    let harness_runs = control_plane.runs.clone();

    // A session, and another after every logout. `run` reports `Relogin` when
    // the Account page's logout landed, and the whole point of that logout is to
    // reach the login screen — dropping the operator back to the shell instead
    // would contradict both the message it shows and the no-session startup path
    // they would then have to trigger by relaunching.
    let mut status = startup_status.or(link_status).or(log_note);
    let result = loop {
        // Read before the wiring is built: holding the lock into an awaited call
        // would park every other host operation behind this session for as long
        // as it runs.
        let primary_observation = started_hosts
            .lock()
            .expect("started hosts")
            .first()
            .map(|host| host.observation());
        let exit = run(
            &mut terminal,
            runtime.clone(),
            SessionWiring {
                local_hosts: local_host_harnesses.clone(),
                loaded: loaded.clone(),
                startup_status: status.take(),
                link_obs: link_obs.clone(),
                config_path: active_config_path.clone(),
                hooks_config_path: hooks_config_path.clone(),
                medulla_home: home.clone(),
                account: account.clone(),
                sharing: sharing.take(),
                onboarding_path: active_config_path.clone(),
                // The primary's counters only. The Overview device panel shows
                // one host, so extras are served and dispatchable but not yet
                // reflected there — a UI gap, not a hosting one.
                host: primary_observation.clone(),
                local_sessions: local_sessions.clone(),
                harness_runs: harness_runs.clone(),
                hook_log: hook_log.clone(),
            },
        )
        .await;

        match exit {
            Ok(SessionExit::Relogin) => {
                // The copilot host cache is process-global and keyed by
                // workflow id, not by account (see
                // `event_loop::clear_copilot_hosts`'s doc). Left alive across
                // this boundary, a second account opening a workflow that
                // happens to share an id with one the first account had a
                // live conversation on would silently inherit that daemon's
                // harness session and context.
                crate::event_loop::clear_copilot_hosts();
                // Only the embedded core can be signed back in; every other
                // runtime reports that it holds no session, so its logout never
                // succeeds and this arm is unreachable for it.
                if let Some(core) = core_arc.clone() {
                    match relogin(&mut terminal, &core, &env, &loaded.config.backend.base_url).await
                    {
                        // Signing in as a different account re-homes the
                        // install, and this process cannot follow: its config,
                        // logs, workflow store, and the core's own workspace all
                        // belong to the account it started as. Their session was
                        // not stored in it — the next launch signs them in
                        // against their own home.
                        Ok(SignIn::SwitchedAccount(notice)) => {
                            account_switched = Some(notice);
                            break Ok(());
                        }
                        Ok(SignIn::SameAccount) => {
                            (_, account) = session_of(&core, &loaded.config.backend.base_url).await;
                            runtime = core_runtime(
                                core,
                                hub_slot.clone(),
                                &loaded.config.backend.base_url,
                            )
                            .await;
                            continue;
                        }
                        // Quit from the login screen: the operator asked to
                        // leave, having already logged out.
                        Ok(SignIn::Quit) => break Ok(()),
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
    drop(link_service); // aborts the background loops.
                        // Printed after the screen is back: the operator signed in as somebody else
                        // mid-session, the marker now names them, and the next launch opens their
                        // home. Nothing is lost — the previous account's directory stays put.
    if let Some(notice) = account_switched {
        // Both after the restore: the alt screen swallows anything written to
        // the terminal while the app still owns it.
        if let Some(notice) = notice {
            eprintln!("{notice}");
        }
        println!("{SWITCHED_ACCOUNT_NOTICE}");
    }
    result
}

/// Refuse an explicit configuration path that cannot contribute any config.
///
/// `load_config` intentionally permits missing paths for config creation flows,
/// but TUI startup must not silently fall back to the default link identity.
pub(crate) fn validate_explicit_config(path: Option<&str>) -> anyhow::Result<()> {
    if let Some(path) = path {
        if !std::path::Path::new(path).is_file() {
            anyhow::bail!("explicit TUI configuration does not exist: {path}");
        }
    }
    Ok(())
}

/// Keep only presets that belong to the primary host and can launch locally.
pub(super) fn available_primary_presets(
    presets: &[medulla::config::CustomHarnessConfig],
    host_id: &str,
    providers: &[medulla::protocol::HarnessProvider],
) -> Vec<medulla::config::CustomHarnessConfig> {
    presets
        .iter()
        .filter(|preset| preset.host_id == host_id && preset.runnable_on(providers))
        .cloned()
        .collect()
}

/// Offer an available release before the interactive session starts, so an
/// accepted update never interrupts an active turn or terminal session.
async fn prompt_for_update(
    config: &medulla::config::UpdateConfig,
    env: &std::collections::HashMap<String, String>,
) {
    if !config.enabled(env) {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let Ok(Some(info)) =
        medulla::update::check_for_update(&medulla::update::update_url(), current).await
    else {
        return;
    };
    println!(
        "medulla {} is available (current {}). Install now? [y/N]",
        info.version, current
    );
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_ok() && answer.trim().eq_ignore_ascii_case("y") {
        match medulla::update::install_update(&info).await {
            Ok(()) => println!("updated successfully; starting medulla {}.", info.version),
            Err(error) => eprintln!("update failed; continuing with medulla {current}: {error}"),
        }
    }
}
