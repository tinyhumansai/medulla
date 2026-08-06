//! Binary entry point for the `medulla` TUI: load `.env`, parse the top-level
//! command, and dispatch. The heavy lifting lives in sibling modules:
//! [`terminal`] owns the crossterm terminal lifecycle, [`commands`] the non-TUI
//! subcommand runners and the pre-app login screen, [`app_loop`] TUI startup and
//! runtime selection, and [`event_loop`] the interactive event loop.

use std::io::{self, IsTerminal};

use medulla_tui::cli::{parse_command, sessions_json, Command};

use crate::app_loop::run_tui;
use crate::commands::run_hook_cmd;
use crate::commands::{run_hub, run_init, run_login, run_logout, run_workspace};
#[cfg(feature = "workflows")]
use crate::commands::{run_mcp_cmd, run_skills_cmd, run_workflow_cmd};
use crate::run::run_core;

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
    // The hook shim runs inside an operator's live turn under a 3-5 second
    // harness deadline (see `commands::hook`'s module docs), so it gets a
    // single-thread runtime rather than paying to spin up the multi-thread,
    // 16 MiB-per-worker-stack runtime every other command needs to host an
    // agent turn.
    if matches!(parse_command(&raw), Command::Hook) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async_main(raw))
    } else {
        medulla::tokio_tuning::build_runtime()?.block_on(async_main(raw))
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
    let command = parse_command(&raw);

    // Load a cwd `.env` into the process env before anything reads it (this is
    // how local dev opts into `MEDULLA_DEV=1`). Never overrides existing vars.
    //
    // The hook shim is the exception: it runs inside the operator's live turn,
    // with the harness waiting on this process under a hard deadline, and the
    // workspace `.env` can be a FIFO/device (or just enormous). An unbounded
    // read there would burn the shim's whole budget before `run_hook_cmd`'s
    // own deadline even starts, so the harness would kill it as a hung hook.
    if !matches!(&command, Command::Hook) {
        medulla::home::load_dotenv_from_cwd();
    }

    match command {
        Command::Run => run_core(&raw[1..]).await,
        Command::Daemon if daemon_uses_tui(io::stdout().is_terminal(), &raw) => {
            run_worker_tui_command(&raw[1..]).await
        }
        Command::Daemon => medulla::daemon::run_daemon(&raw[1..], onboarding_ui()).await,
        Command::DaemonTui => run_worker_tui_command(&raw[1..]).await,
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
/// One process: the tiny.place identity, the contact queue, the harness PTYs,
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
    // former reads `MEDULLA_STAGING`, and a host on the prod forwarder never
    // hears from an orchestrator on staging.
    let link_config = loaded
        .config
        .link
        .clone()
        .unwrap_or_else(|| medulla::config::default_link_config(&env));
    let masters = link_config.peers.clone();

    // The bridge the worker loop serves peer work over. One endpoint holds the
    // link identity for the life of the process: a second `Link` on the same
    // node would draw sequences from a second counter under one AEAD key, which
    // reuses nonces (protocol §3.1).
    let enrolled_node_id = medulla_link::keys::read_node_state(&medulla_link::keys::node_path(
        std::path::Path::new(&link_config.state_dir),
    ))
    .ok()
    .map(|state| state.node_id.to_string());
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
        // The link has no contact graph — enrollment is the only handshake
        // (protocol §7) — so there is no admission queue to render.
        contacts: None,
        agent_id,
        startup_status,
        transport,
        endpoint: Some(link_config.forwarder_url.clone()),
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

#[cfg(test)]
#[path = "main_tests.rs"]
mod daemon_entry_tests;
