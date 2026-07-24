//! Binary entry point for the `medulla` TUI: load `.env`, parse the top-level
//! command, and dispatch. The heavy lifting lives in sibling modules:
//! [`terminal`] owns the crossterm terminal lifecycle, [`commands`] the non-TUI
//! subcommand runners and the pre-app login screen, [`app_loop`] TUI startup and
//! runtime selection, and [`event_loop`] the interactive event loop.

use std::io::{self, IsTerminal};

use medulla_tui::cli::{parse_command, sessions_json, Command};

use crate::app_loop::run_tui;
use crate::commands::{run_hub, run_init, run_login, run_logout, run_memory};
use crate::run::run_core;

mod app_loop;
mod commands;
mod event_loop;
mod hub_relay;
mod run;
mod terminal;
mod worker_loop;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load a cwd `.env` into the process env before anything reads it (this is
    // how local dev opts into `MEDULLA_DEV=1`). Never overrides existing vars.
    medulla::home::load_dotenv_from_cwd();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    match parse_command(&raw) {
        Command::Run => run_core(&raw[1..]).await,
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
        Command::Login => run_login(&raw[1..]).await,
        Command::Logout => run_logout(),
        Command::Memory => run_memory(&raw[1..]).await,
        Command::Init => run_init(&raw[1..]).await,
        Command::Hub => run_hub(&raw[1..]).await,
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
    let loaded = medulla::config::load_config(None, &env, std::path::Path::new(&cwd))?;

    // The contact queue and this daemon's address both come from the tiny.place
    // service. Without a `[tinyplace]` section there is no identity, so the
    // Contacts and Requests tabs say so rather than showing empty lists.
    let mut startup_status = None;
    // A worker always has an identity: the address is minted on first run and
    // reused from `<medulla_home>/tinyplace/config.json` thereafter. Absent a
    // `[tinyplace]` section we synthesize one rather than running without it, so
    // `medulla daemon --tui` bootstraps exactly like `medulla daemon` does —
    // same wallet, same location, same relay. Requiring config for the TUI and
    // not for the daemon would mean adding `--tui` silently cost you your peers.
    // It must be `default_tinyplace_config`, not `TinyplaceConfig::default()`:
    // only the former reads `MEDULLA_STAGING`, and a worker on the prod relay
    // never sees a contact request sent on staging.
    let mut tinyplace_config = loaded
        .config
        .tinyplace
        .clone()
        .unwrap_or_else(|| medulla::config::default_tinyplace_config(&env));

    // Claim the identity before anything binds it. `medulla daemon --tui` is a
    // daemon — it publishes pre-keys, drains one inbox and drives one Signal
    // ratchet — so two of them sharing a wallet would split a peer's messages
    // between them and corrupt the ratchet for everyone. The guard is bound in
    // this function, which lives as long as the process.
    let _identity_lock = claim_identity(&env, &mut tinyplace_config)?;

    let service = match medulla::tinyplace::service::TinyplaceService::start(&tinyplace_config) {
        Ok(service) => Some(service),
        Err(err) => {
            startup_status = Some(format!("tiny.place service failed to start ({err})"));
            None
        }
    };
    let contacts = service.as_ref().map(|s| s.contacts());
    // The encrypted transport is what lets peers reach this worker at all, and
    // it comes from the same service as the contact queue — one transport per
    // wallet, because two would be two writers to one Signal session store.
    // Without a `[tinyplace]` section there is no identity, so the TUI runs as a
    // local-sessions screen and simply never receives peer work.
    let transport = service.as_ref().map(|s| s.transport());
    // A failed pre-key publish makes this worker undeliverable, which otherwise
    // looks from both ends like peer messages simply vanish. Surface it.
    if let Some(notice) = service
        .as_ref()
        .and_then(|s| s.observation().lock().ok().and_then(|o| o.notice.clone()))
    {
        startup_status = Some(notice);
    }
    let agent_id = service.as_ref().and_then(|s| {
        s.observation()
            .lock()
            .ok()
            .and_then(|o| o.identity.as_ref().map(|i| i.agent_id.clone()))
    });

    let result = worker_loop::run_worker_tui(
        env,
        workspace,
        contacts,
        agent_id,
        startup_status,
        transport,
        service.as_ref().map(|s| s.endpoint().to_string()),
        // Claude gates a fresh directory behind a modal trust dialog that only
        // appears on a TTY, so the worker clears it up front — naming the
        // workspace at launch is the decision to run peer work there. This
        // declines that on the operator's behalf instead.
        !args.iter().any(|a| a == "--no-trust-workspace"),
        // Peer sessions run unattended, so they run with the harness's
        // permission bypass — nobody is in the pane to answer a prompt, and a
        // task that stops on one has hung until it times out.
        !args.iter().any(|a| a == "--no-skip-permissions"),
    )
    .await;
    drop(service); // aborts the background polls
    result
}

/// Acquire this worker's tiny.place identity exclusively, rewriting
/// `config.identity_dir` to the slot that was actually claimed.
///
/// Two paths, matching the daemon's:
///
/// - the config left `identityDir` at its home-derived default, so this worker
///   takes the first free slot of the identity pool — slot 1 (the address it has
///   always had) when it is the only worker, `workers/<N>` when it is not; or
/// - the operator named an identity, in `[tinyplace].identityDir` or in the
///   environment, so that exact one is taken and a collision is an error rather
///   than a silent move to an address the operator did not choose.
fn claim_identity(
    env: &std::collections::HashMap<String, String>,
    config: &mut medulla::config::TinyplaceConfig,
) -> anyhow::Result<medulla::tinyplace::IdentityLock> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    // Component-wise, not string or raw-`Path` equality: a hand-written
    // `identityDir = ".../tinyplace/"` keeps its trailing separator, which a
    // `Path`-value comparison treats as different and would route a default
    // worker down the fail-loud named path — so a second one would see "already
    // in use" instead of fanning out to `workers/2`.
    let default_pool_dir = medulla::home::medulla_home(env).join("tinyplace");
    let pooled = std::path::Path::new(&config.identity_dir)
        .components()
        .eq(default_pool_dir.components());
    let acquired = if pooled {
        medulla::tinyplace::acquire_identity(env, &home)
    } else {
        let named =
            std::path::Path::new(&config.identity_dir).join(medulla::tinyplace::IDENTITY_FILE);
        medulla::tinyplace::acquire_identity_at(&named, env)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    config.identity_dir = acquired.identity_dir.to_string_lossy().into_owned();
    Ok(acquired.lock)
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
