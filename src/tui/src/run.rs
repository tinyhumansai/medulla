//! Process wiring for `medulla run`: the non-interactive core driver.
//!
//! This is the headless counterpart to the TUI — a scriptable entry point a
//! docker container or CI job can drive without a TTY or tmux. It boots the
//! embedded OpenHuman core, submits one instruction, and streams the folded
//! cycle events to stdout as JSON lines via
//! [`medulla::runtime::headless::drive_once`] (see that module for the line
//! contract).
//!
//! It used to attach to an external `medulla-serve` unix socket. The core now
//! runs in this process, so there is no socket to resolve, no attach handshake
//! to fail, and no unix-only restriction — the same command works wherever the
//! binary does.

use medulla_tui::cli::parse_run_args;

/// Run the `medulla run` subcommand over `args` (everything after `run`).
///
/// Boots the embedded core against the operator's Medulla home and drives one
/// instruction to its cycle result. Errors: an unparseable command line, a core
/// that cannot boot (a broken workspace or config), or a cycle that never
/// completes within the headless timeout.
pub(crate) async fn run_core(args: &[String]) -> anyhow::Result<()> {
    use std::sync::Arc;

    use medulla::config::load_config;
    use medulla::runtime::headless::{drive_once, HeadlessOptions};
    use medulla::runtime::openhuman::OpenHumanRuntime;
    use medulla::runtime::Runtime;

    let parsed = parse_run_args(args).map_err(|e| anyhow::anyhow!(e))?;

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;

    // Bind the core's state directory before anything can construct it, exactly
    // as the interactive path does. Without this a scripted run reads and writes
    // the developer's real `~/.openhuman` instead of the `MEDULLA_HOME` it was
    // pointed at — which is precisely what CI and container runs rely on.
    let home = medulla::home::medulla_home(&env);
    medulla::core_host::bind_workspace(&env, &home);
    let first_workspace = loaded
        .config
        .workflow
        .workspaces
        .first()
        .map(std::path::PathBuf::from);
    medulla::core_host::bind_action_dir(&env, first_workspace.as_deref());
    // And the endpoints, for the same reason: a scripted run on staging that let
    // the core resolve its own backend would use the session stored by
    // `medulla login` against a deployment that never issued it.
    medulla::core_host::bind_medulla_base_url(&env, &loaded.config.backend.base_url);
    medulla::core_host::bind_backend_api_url(&env, &loaded.config.backend.base_url);

    let core = medulla::core_host::boot()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start the embedded OpenHuman core: {e}"))?;
    let runtime = Arc::new(OpenHumanRuntime::new(Arc::new(core)));
    // Replies arrive by polled replay, so the driver would see nothing at all
    // unless the loop runs — the same wiring the TUI does at boot.
    runtime.spawn_poll_loop();
    let runtime: Arc<dyn Runtime> = runtime;

    let mut stdout = std::io::stdout();
    let result = drive_once(
        runtime.clone(),
        parsed.instruction,
        &mut stdout,
        HeadlessOptions::default(),
    )
    .await;

    // Always shut down cleanly, whether the run passed or failed. The driver's
    // typed `HeadlessError` folds into `anyhow` here — the binary layer only
    // reports.
    runtime.shutdown().await.ok();
    result.map(|_| ()).map_err(anyhow::Error::from)
}
