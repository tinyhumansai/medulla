//! Binary entry point for the `medulla` TUI: load `.env`, parse the top-level
//! command, and dispatch. The heavy lifting lives in sibling modules:
//! [`terminal`] owns the crossterm terminal lifecycle, [`commands`] the non-TUI
//! subcommand runners and the pre-app login screen, [`app_loop`] TUI startup and
//! runtime selection, and [`event_loop`] the interactive event loop.

use std::io::{self, IsTerminal};

use medulla_tui::cli::{parse_command, sessions_json, Command};

use crate::app_loop::run_tui;
use crate::commands::run_hook_cmd;
use crate::commands::{
    run_analytics_test, run_hub, run_init, run_login, run_logout, run_sentry_test, run_workspace,
};
#[cfg(feature = "workflows")]
use crate::commands::{run_mcp_cmd, run_skills_cmd, run_workflow_cmd};
use crate::run::run_core;

mod access_gate;
mod app_loop;
#[cfg(test)]
mod app_loop_tests;
mod commands;
#[cfg(feature = "workflows")]
mod control_plane;
mod event_loop;
mod hub_relay;
mod local_host;
mod run;
mod sign_in;
#[cfg(test)]
mod sign_in_tests;
#[cfg(feature = "workflows")]
mod startup_skills;
#[cfg(all(test, feature = "workflows"))]
mod startup_skills_tests;
mod terminal;
mod worker_loop;

/// Build the runtime explicitly rather than via `#[tokio::main]`, which offers
/// no way to set the worker stack size.
///
/// The tuning lives in [`medulla::tokio_tuning`] because the embedded OpenHuman
/// core is a dependency of the SDK, not of this crate — so that is the only
/// place able to source the values from it rather than restating them.
fn main() -> anyhow::Result<()> {
    install_crypto_provider();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let is_hook = matches!(parse_command(&raw), Command::Hook);
    // `--mock` is the offline demo runtime: no backend, no login, and per
    // README no network — starting crash reporting here would let a
    // configured (or `.env`-overridden) DSN reach out during what is supposed
    // to be an entirely local demo.
    let is_mock =
        matches!(parse_command(&raw), Command::Tui) && medulla_tui::cli::parse_tui_args(&raw).mock;

    // Load a cwd `.env` into the process env before anything reads it. Never
    // overrides existing vars, and never sets the variables that choose the
    // home or the account in it (`home::dotenv::HOME_SELECTORS`): a `.env`
    // belongs to the repository being opened, so `MEDULLA_DEV=1` and the
    // like must come from the invoking shell. Done ahead of crash reporting so
    // a `.env` can carry the opt-out.
    //
    // The hook shim is the exception: it runs inside the operator's live turn,
    // with the harness waiting on this process under a hard deadline, and the
    // workspace `.env` can be a FIFO/device (or just enormous). An unbounded
    // read there would burn the shim's whole budget before `run_hook_cmd`'s
    // own deadline even starts, so the harness would kill it as a hung hook.
    // For the same reason it never starts crash reporting — no transport
    // thread, and no flush on exit.
    //
    // The guard is bound here, outside the runtime, so it outlives every task
    // and its drop flushes queued reports on the way out. Initialized after the
    // TLS provider (the transport opens connections) and before the runtime, so
    // Sentry's panic hook is installed first; the TUI's terminal-restoring hook
    // is chained on top of it later, which means a panic restores the screen
    // before the report is captured and flushed.
    let _crash_reporting = if is_hook {
        None
    } else {
        // A cwd `.env` is controlled by the repository being opened. Preserve
        // only a DSN supplied by the invoking environment so an untrusted
        // checkout cannot redirect crash reports to its own collector.
        let trusted_sentry_dsn = std::env::var_os(medulla::observability::DSN_ENV);
        let trusted_analytics_url = std::env::var_os(medulla::analytics::API_URL_ENV);
        // For the same reason the stored account reports are attributed to is
        // resolved from the invoking environment, as it was before the `.env`
        // loaded: a checkout must not be able to plant a home or config whose
        // session names an account of its choosing.
        let invoking_env = decoded_env();
        let refused = medulla::home::load_dotenv_from_cwd();
        if !refused.is_empty() {
            // stderr: stdout can be a protocol stream (`medulla mcp`).
            eprintln!(
                "medulla: ignored {} from ./.env — a repository's .env cannot choose \
                 Medulla's home or account; set {} in your shell instead",
                refused.join(", "),
                if refused.len() == 1 { "it" } else { "them" },
            );
        }
        match trusted_sentry_dsn {
            Some(dsn) => std::env::set_var(medulla::observability::DSN_ENV, dsn),
            None => std::env::remove_var(medulla::observability::DSN_ENV),
        }
        match trusted_analytics_url {
            Some(url) => std::env::set_var(medulla::analytics::API_URL_ENV, url),
            None => std::env::remove_var(medulla::analytics::API_URL_ENV),
        }
        if is_mock {
            // The demo makes no network connections at all, so it opts out
            // of analytics too; set before anything resolves the tracker,
            // and before the runtime starts any thread.
            std::env::set_var(medulla::observability::DISABLED_ENV, "1");
            None
        } else {
            set_stored_telemetry_user(&raw, &invoking_env, &decoded_env());
            Some(medulla::observability::init())
        }
    };

    // The hook shim runs inside an operator's live turn under a 3-5 second
    // harness deadline (see `commands::hook`'s module docs), so it gets a
    // single-thread runtime rather than paying to spin up the multi-thread,
    // 16 MiB-per-worker-stack runtime every other command needs to host an
    // agent turn.
    if is_hook {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async_main(raw))
    } else {
        medulla::tokio_tuning::build_runtime()?.block_on(async_main(raw))
    }
}

/// Where the selected command will load its configuration from: the explicit
/// `--config` path, if any, and the directory layered discovery starts in.
/// Each is read through that command's own parser wherever one exists.
///
/// A blind scan of the argv misreads arguments that are not Medulla flags:
/// harness wrapper flags belong to the child CLI (Codex's `--config key=value`
/// is a model override), and `medulla run` accepts only `--config <path>`, so a
/// `--config=...` token there is instruction text, not a config path. Which
/// occurrence wins differs too, and is followed here. The
/// daemon has two parsers: its TUI takes either spelling (first wins) and
/// discovers from the process directory, while the headless daemon takes only
/// `--config <path>` (last wins) and discovers from its `--workspace`. Of the
/// remaining commands only `mcp`, `remote`, and `daemon --direct` accept the
/// `--config=<path>` spelling.
fn config_source(
    raw: &[String],
    env: &std::collections::HashMap<String, String>,
    stdout_is_terminal: bool,
) -> ConfigSource {
    let cwd = || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let at_cwd = |config: Option<String>| ConfigSource { config, dir: cwd() };
    match parse_command(raw) {
        Command::Wrapper(_) => wrapper_config_source(env),
        Command::Tui => at_cwd(medulla_tui::cli::parse_tui_args(raw).config),
        Command::Run => at_cwd(
            medulla_tui::cli::parse_run_args(&raw[1..])
                .ok()
                .and_then(|args| args.config),
        ),
        Command::DaemonTui => at_cwd(flag_value(&raw[1..], "--config")),
        Command::Hub => hub_config_source(env),
        Command::Daemon if daemon_uses_tui(stdout_is_terminal, raw) => {
            at_cwd(flag_value(&raw[1..], "--config"))
        }
        Command::Daemon => ConfigSource {
            config: last_separate_flag(&raw[1..], "--config"),
            dir: last_separate_flag(&raw[1..], "--workspace")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(cwd),
        },
        // These read the first `--config`, in either spelling.
        // `mcp` is usually spawned by a parent that passes its selection down
        // as `MEDULLA_CONFIG_PATH`; an argv `--config` overrides it, and
        // `serve_stdio` loads whichever results.
        Command::Mcp => at_cwd(
            flag_value(&raw[1..], "--config")
                .or_else(|| medulla::config::explicit_config_from_env(env).map(str::to_owned)),
        ),
        Command::Remote | Command::DaemonDirect => at_cwd(flag_value(&raw[1..], "--config")),
        // The rest parse `--config <path>` alone, each occurrence overwriting
        // the last, so the final one is what the command loads.
        _ => at_cwd(last_separate_flag(&raw[1..], "--config")),
    }
}

/// A wrapper's own flags belong to the child harness, so its Medulla config
/// comes only from an inherited `MEDULLA_CONFIG_PATH` — what `run_wrapper` and
/// `build_bridge` load — discovered from the directory it was launched in.
fn wrapper_config_source(env: &std::collections::HashMap<String, String>) -> ConfigSource {
    ConfigSource {
        config: medulla::config::explicit_config_from_env(env).map(str::to_owned),
        dir: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    }
}

/// `run_hub` loads its configuration from the account home alone, never from
/// a checkout it happens to be launched in.
fn hub_config_source(env: &std::collections::HashMap<String, String>) -> ConfigSource {
    ConfigSource {
        config: None,
        dir: medulla::home::medulla_home(env),
    }
}

/// See [`config_source`].
#[derive(Debug, PartialEq, Eq)]
struct ConfigSource {
    config: Option<String>,
    dir: std::path::PathBuf,
}

/// The last value of a `--name <value>` flag, read the way the headless
/// daemon's tokenizer and the overwrite-as-you-go command parsers do:
/// separate-token form only, later wins.
fn last_separate_flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .rev()
        .find_map(|pair| (pair[0] == name).then(|| pair[1].clone()))
}

/// Set at startup when the cwd `.env` moved this process onto a different
/// stored session than the one it was invoked with.
///
/// The TUI rebuilds its account from the effective environment, so without
/// this it would undo the startup decision and attribute telemetry to whatever
/// session the checkout pointed it at. It reads this and names the stored
/// account only when this process signed it in itself.
pub(crate) static DOTENV_RESELECTED_SESSION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether the `.env` re-selected the stored session: a different account home,
/// or a different account in it.
fn session_reselected(
    invoking: &std::collections::HashMap<String, String>,
    effective: &std::collections::HashMap<String, String>,
) -> bool {
    medulla::home::medulla_home(invoking) != medulla::home::medulla_home(effective)
        || medulla::auth::state(invoking).user_id != medulla::auth::state(effective).user_id
}

/// The process environment as a map, dropping any entry that is not valid
/// UTF-8 rather than panicking on it as `std::env::vars()` would.
fn decoded_env() -> std::collections::HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// Attribute non-TUI harness and daemon analytics to a stored account only
/// when config/environment credentials do not override that session.
///
/// `invoking` is the environment captured before a cwd `.env` loaded, so the
/// checkout being opened cannot choose the home, config, or session the
/// account id is read from. The credential check also consults `effective`,
/// the environment after the `.env`: a credential the checkout supplies (a
/// `MEDULLA_TOKEN`, a config with an inline token) is what the command will
/// authenticate with, so the stored account must not be named then either;
/// nor when the `.env` re-selects a different stored session.
fn set_stored_telemetry_user(
    raw: &[String],
    invoking: &std::collections::HashMap<String, String>,
    effective: &std::collections::HashMap<String, String>,
) {
    DOTENV_RESELECTED_SESSION.store(
        session_reselected(invoking, effective),
        std::sync::atomic::Ordering::Release,
    );
    let user_id = stored_telemetry_user(raw, invoking, effective, io::stdout().is_terminal());
    medulla::observability::set_user(user_id.as_deref());
}

/// The decision behind [`set_stored_telemetry_user`], without setting it.
fn stored_telemetry_user(
    raw: &[String],
    invoking: &std::collections::HashMap<String, String>,
    effective: &std::collections::HashMap<String, String>,
    terminal: bool,
) -> Option<String> {
    let external_wins = |env: &std::collections::HashMap<String, String>| {
        let source = config_source(raw, env, terminal);
        medulla::config::load_config(source.config.as_deref(), env, &source.dir)
            .map(|loaded| medulla::auth::external_token_wins(env, &loaded.config.backend))
    };
    // Unreadable config in either view attributes nothing, as before. And the
    // `.env` may also re-select the stored session itself (`MEDULLA_HOME`,
    // `MEDULLA_USER`) without supplying a token: the command then runs as that
    // session's account, so the id is named only when both views agree on it.
    match (external_wins(invoking), external_wins(effective)) {
        // Same id is not enough: another root can hold a session that repeats
        // the id with a different token. The session must come from the same
        // account home in both views.
        (Ok(false), Ok(false)) if !session_reselected(invoking, effective) => {
            medulla::auth::state(invoking).user_id
        }
        _ => None,
    }
}

/// Pick the TLS backend before anything opens a connection.
///
/// rustls 0.23 refuses to guess when more than one provider is compiled in, and
/// this binary has two: `ring` arrives with reqwest's `rustls-tls`, `aws-lc-rs`
/// with the vendored OpenHuman core. Neither wins by default, so the first TLS
/// handshake panicked — on a tokio worker thread, which meant the process
/// survived and the TUI kept drawing while every relay call died silently.
///
/// Done here rather than at each call site because the choice is process-wide
/// and must be made before the first handshake, whichever subcommand runs.
/// A failure means a provider is already installed, which is equally fine.
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

async fn async_main(raw: Vec<String>) -> anyhow::Result<()> {
    let result = dispatch(raw).await;
    // The runtime drops as soon as this returns, aborting any analytics event
    // still in flight (a TUI's last screen view, a `daemon --once` task's
    // token usage); give them a bounded chance to land first. Returns at once
    // when nothing is pending, so the deadline-bound hook pays nothing.
    medulla::analytics::flush_pending().await;
    result
}

/// Run the selected command.
async fn dispatch(raw: Vec<String>) -> anyhow::Result<()> {
    // `.env` was already loaded by `main`, ahead of crash reporting.
    match parse_command(&raw) {
        Command::Run => run_core(&raw[1..]).await,
        Command::Daemon if daemon_uses_tui(io::stdout().is_terminal(), &raw) => {
            run_worker_tui_command(&raw[1..]).await
        }
        Command::Daemon => medulla::daemon::run_daemon(&raw[1..], onboarding_ui()).await,
        Command::DaemonTui => run_worker_tui_command(&raw[1..]).await,
        Command::DaemonDirect => medulla_tui::remote::serve::entry::run(&raw[1..]).await,
        Command::Remote => medulla_tui::remote::client::exec::run(&raw[1..]).await,
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
        // Deliberately ahead of everything that loads config or touches the
        // terminal: the shim runs inside an operator's live turn, and its whole
        // job is to be cheap and silent. `run_hook_cmd` reads only the hook
        // grant's own two variables, so only those are decoded here —
        // `std::env::vars()` panics on any non-UTF-8 entry in the inherited
        // environment, and that must never turn "always exit zero" into a
        // crash before the shim gets a chance to swallow anything.
        Command::Hook => {
            let env: std::collections::HashMap<String, String> = [
                medulla::control_socket::HOOK_SOCKET_ENV,
                medulla::control_socket::HOOK_GRANT_ENV,
            ]
            .into_iter()
            .filter_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|value| (key.to_string(), value))
            })
            .collect();
            run_hook_cmd(&raw[1..], &env).await;
            Ok(())
        }
        Command::SentryTest => run_sentry_test().await,
        Command::AnalyticsTest => run_analytics_test().await,
        Command::Login => run_login(&raw[1..]).await,
        Command::Logout => run_logout().await,
        Command::Init => run_init(&raw[1..]).await,
        Command::Workspace => run_workspace(&raw[1..]).await,
        Command::Hub => run_hub(&raw[1..]).await,
        #[cfg(feature = "workflows")]
        Command::Workflow => run_workflow_cmd(&raw[1..]).await,
        #[cfg(feature = "workflows")]
        Command::Skills => run_skills_cmd(&raw[1..]),
        #[cfg(feature = "workflows")]
        Command::Mcp => run_mcp_cmd(&raw[1..]).await,
        #[cfg(not(feature = "workflows"))]
        Command::Mcp => {
            anyhow::bail!("this build has no MCP server (built without the `workflows` feature)")
        }
        // Built without the workflow engine: say so rather than starting the
        // TUI, which is what an unhandled subcommand would otherwise do.
        #[cfg(not(feature = "workflows"))]
        Command::Workflow => {
            anyhow::bail!(
                "this build has no workflow support (built without the `workflows` feature)"
            )
        }
        // Same reasoning as `Workflow` above: there are no workflows to
        // generate skills for, so say so instead of opening the TUI.
        #[cfg(not(feature = "workflows"))]
        Command::Skills => {
            anyhow::bail!(
                "this build has no workflow support (built without the `workflows` feature)"
            )
        }
        Command::Update => {
            let args = medulla_tui::cli::parse_update_args(&raw[1..]);
            medulla::update::run_update(args.check).await
        }
        Command::Wrapper(provider) => {
            let code = medulla::wrapper::run_wrapper(
                provider,
                &raw[1..],
                onboarding_ui(),
                Some(medulla_tui::harness_pty::spawner()),
            )
            .await?;
            medulla::analytics::flush_pending().await;
            medulla::observability::flush(std::time::Duration::from_secs(2));
            std::process::exit(code);
        }
        // Bare invocation, or the TUI's own --config/--no-alt-screen flags.
        Command::Tui => run_tui(&raw).await,
    }
}

/// Whether `medulla daemon` should open its operator UI.
///
/// A real terminal gets the simplified TUI by default. Pipes and service
/// managers remain headless, and `--headless` is the explicit opt-out for a
/// person launching from a terminal.
fn daemon_uses_tui(stdout_is_terminal: bool, args: &[String]) -> bool {
    stdout_is_terminal && !args.iter().any(|arg| arg == "--headless")
}

/// Build the interactive onboarding callback when stdout is a TTY, else `None`
/// so the daemon/wrapper first-run flow auto-registers headlessly. This is the
/// app-side seam that keeps the SDK free of any terminal dependency.
fn onboarding_ui() -> Option<medulla::onboarding::OnboardingUi> {
    if io::stdout().is_terminal() {
        Some(Box::new(|ctx| {
            Box::pin(medulla_tui::ui::onboarding::run_onboarding_ui(ctx))
        }))
    } else {
        None
    }
}

/// Start the worker-daemon TUI (`medulla daemon --tui`).
///
/// One process: the host-link identity, the contact queue, the harness PTYs,
/// and the screen all live in it. Harness sessions run in the current working
/// directory with this process's environment, so the operator sees the repo they
/// launched from.
async fn run_worker_tui_command(args: &[String]) -> anyhow::Result<()> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    // Where peer tasks run. This is the single most consequential argument the
    // worker takes: a harness serving a peer edits files here, so defaulting to
    // the current directory means the shell you launched from decides what a
    // remote peer can touch.
    let workspace = flag_value(args, "--workspace")
        .map(|dir| {
            std::fs::canonicalize(&dir)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(dir)
        })
        .unwrap_or_else(|| cwd.clone());
    let explicit_config = flag_value(args, "--config");
    // Recorded before anything spawns off this process — see
    // `medulla::config::CONFIG_PATH_ENV` and the matching comment in
    // `app_loop::run_tui`. This worker TUI runs the same daemon that spawns
    // ACP harness subprocesses, so it needs the same propagation.
    if let Some(path) = explicit_config.as_deref() {
        if !std::path::Path::new(path).is_file() {
            anyhow::bail!("explicit daemon configuration does not exist: {path}");
        }
        std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
    }
    let loaded =
        medulla::config::load_config(explicit_config.as_deref(), &env, std::path::Path::new(&cwd))?;
    let config_path = explicit_config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| medulla::home::medulla_home(&env).join("config.toml"));

    // The link identity is what lets an orchestrator reach this worker at all.
    // Without an enrolled one the TUI still runs — as a local-sessions screen
    // that simply never receives peer work — rather than refusing to start on a
    // machine the operator has not finished setting up.
    let mut startup_status = None;
    // The `[link]` section, synthesized when the config file has none so
    // `medulla daemon --tui` bootstraps exactly like `medulla daemon` does —
    // same identity directory, same forwarder. Requiring config for the TUI and
    // not for the daemon would mean adding `--tui` silently cost you your hosts.
    // It must be `default_link_config`, not `LinkConfig::default()`: only the
    // former follows the resolved backend, and a host on the prod forwarder
    // never hears from an orchestrator on staging.
    let link_config = loaded.config.link.clone().unwrap_or_else(|| {
        medulla::config::default_link_config(&env, &loaded.config.backend.base_url)
    });
    let masters = link_config.peers.clone();

    // The bridge the worker loop serves peer work over. One endpoint holds the
    // link identity for the life of the process: a second `Link` on the same
    // node would draw sequences from a second counter under one AEAD key, which
    // reuses nonces (protocol §3.1).
    let enrolled = medulla_link::keys::read_node_state(&medulla_link::keys::node_path(
        std::path::Path::new(&link_config.state_dir),
    ))
    .ok();
    let enrolled_node_id = enrolled.as_ref().map(|state| state.node_id.to_string());
    // What the datagrams actually go to. The forwarder key is issued *with* the
    // endpoint at enrollment, so a host cannot be re-pointed at another
    // forwarder by editing config — only by enrolling again — and the transport
    // is right to keep using `node.json`. What was wrong is reporting
    // `link.forwarderUrl` as though it were the live forwarder: an operator who
    // moved `link.forwarderUrl` then read the new deployment back off a screen
    // whose datagrams were still going to the old one. (The backend endpoint
    // itself is pinned now and cannot drift, but the forwarder key can still be
    // configured off it.) Reporting the enrolled endpoint makes that drift
    // visible as the mismatch it is. Only an unenrolled host — which has no
    // endpoint to report — falls back to the configured URL.
    let reported_endpoint = enrolled
        .as_ref()
        .map(|state| state.forwarder_endpoint.clone())
        .filter(|endpoint| !endpoint.trim().is_empty())
        .unwrap_or_else(|| link_config.forwarder_url.clone());
    let transport =
        match medulla_link::Link::connect(medulla_link::LinkConfig::new(&link_config.state_dir))
            .await
        {
            Ok(link) => {
                // The owner is the link's single peer: a host enrolls against
                // exactly one orchestrator (protocol §7.3).
                let owner = link_config
                    .peers
                    .first()
                    .map(|peer| peer.id.clone())
                    .unwrap_or_else(|| "orchestrator".to_string());
                let node_name = link_config
                    .node_name
                    .clone()
                    .or(enrolled_node_id)
                    .unwrap_or_else(|| owner.clone());
                match medulla::bridge::LinkBridge::single_peer(
                    std::sync::Arc::new(link),
                    node_name,
                    owner,
                ) {
                    Ok(bridge) => Some(bridge),
                    Err(err) => {
                        startup_status = Some(format!("host link is misconfigured ({err})"));
                        None
                    }
                }
            }
            Err(err) => {
                startup_status = Some(format!("host link unavailable ({err})"));
                None
            }
        };
    let agent_id = transport
        .as_ref()
        .map(|bridge| bridge.address().to_string())
        .filter(|name| !name.is_empty());

    let workspaces = loaded
        .config
        .workflow
        .workspaces
        .iter()
        .map(|path| {
            std::fs::canonicalize(path)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.clone())
        })
        .collect();
    let theme = medulla_tui::ui::theme::Theme::from_config(&loaded.config.theme);
    let result = worker_loop::run_worker_tui(worker_loop::WorkerTuiConfig {
        env,
        workspace,
        workspaces,
        masters,
        config_path,
        credential_dir: std::path::PathBuf::from(&link_config.state_dir),
        agent_id,
        // Same flag the headless daemon reads, parsed the same way, so the two
        // launch modes cannot disagree about which agents this worker may run.
        only_providers: parse_providers_flag(args),
        startup_status,
        transport,
        endpoint: Some(reported_endpoint),
        theme,
        // Claude gates a fresh directory behind a modal trust dialog that only
        // appears on a TTY, so the worker clears it up front — naming the
        // workspace at launch is the decision to run peer work there. This
        // declines that on the operator's behalf instead.
        trust_workspace: !args.iter().any(|a| a == "--no-trust-workspace"),
        // Peer sessions run unattended, so they run with the harness's
        // permission bypass — nobody is in the pane to answer a prompt, and a
        // task that stops on one has hung until it times out.
        skip_permissions: !args.iter().any(|a| a == "--no-skip-permissions"),
        // The custom OpenAI-compatible router from the layered config. Absent
        // `[router]` leaves this `None` and every harness spawns unrouted.
        router: loaded.config.router.clone(),
        attribution: loaded.config.attribution.commit,
        // Standardized lifecycle hooks from the layered `[[hooks]]` config,
        // installed into whichever harness this worker launches.
        hooks: loaded.config.hooks.clone(),
        // Operator-declared per-provider budgets from the layered `[budget]`
        // config. Absent leaves every harness advertising an estimate.
        budget: loaded.config.budget.clone(),
    })
    .await;
    result
}

/// Read `--name <value>` out of an argument list.
///
/// A local parse rather than the daemon's: its flag types are private to the
/// SDK, and the worker screen needs exactly one of them.
fn flag_value(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == name {
            return it.next().cloned().filter(|v| !v.is_empty());
        }
        if let Some(rest) = arg.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return (!rest.is_empty()).then(|| rest.to_string());
        }
    }
    None
}

/// The `--providers a,b` restriction, or `None` when the flag is absent.
///
/// Unknown names are dropped rather than fatal: the headless daemon rejects them
/// with a message, and this path has already printed a screen by the time it
/// could. A restriction that named nothing usable would leave the worker with no
/// agents at all, which the screen reports for itself — so an empty result is
/// returned as `None`, meaning "unrestricted", only when the flag was never
/// given.
fn parse_providers_flag(args: &[String]) -> Option<Vec<medulla::protocol::HarnessProvider>> {
    let raw = flag_value(args, "--providers")?;
    Some(
        raw.split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .filter_map(medulla::protocol::HarnessProvider::from_wire)
            .collect(),
    )
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod daemon_entry_tests;
