//! Process wiring for `medulla run`: the non-interactive core-runtime driver.
//!
//! This is the headless counterpart to the TUI — a scriptable entry point a
//! docker container or CI job can drive without a TTY or tmux. It attaches the
//! core (`medulla-serve`) runtime to a local unix socket, submits one
//! instruction, and streams the folded cycle events to stdout as JSON lines via
//! [`medulla::runtime::headless::drive_once`] (see that module for the line
//! contract). The socket is never spawned here — attach-only, matching the
//! runtime itself.

use medulla_tui::cli::parse_run_args;

/// Run the `medulla run` subcommand over `args` (everything after `run`).
///
/// Resolves the config and the `medulla-serve` socket (`--core-socket`, then
/// `MEDULLA_CORE_SOCKET`, then the `[core]` config / default runtime dir),
/// attaches the core runtime, and drives one instruction to its cycle result.
/// Errors: an unparseable command line, an unreachable/rejected socket, or a
/// cycle that never completes. Unix-only — the core runtime speaks a unix
/// domain socket.
#[cfg(unix)]
pub(crate) async fn run_core(args: &[String]) -> anyhow::Result<()> {
    use std::sync::Arc;

    use medulla::config::load_config;
    use medulla::runtime::core::CoreRuntime;
    use medulla::runtime::headless::{drive_once, HeadlessOptions};
    use medulla::runtime::Runtime;

    let parsed = parse_run_args(args).map_err(|e| anyhow::anyhow!(e))?;

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;

    // `run` always drives the core runtime, so resolve a socket even when no
    // `[core]` section opted in: fall back to the default runtime-dir path.
    let socket = loaded
        .core_socket_request(&env, parsed.core_socket.as_deref())
        .unwrap_or_else(|| loaded.core_socket_path(&env));

    let runtime: Arc<dyn Runtime> = Arc::new(CoreRuntime::attach(socket));

    let mut stdout = std::io::stdout();
    let result = drive_once(
        runtime.clone(),
        parsed.instruction,
        &mut stdout,
        HeadlessOptions::default(),
    )
    .await;

    // Always tear the attachment down cleanly, whether the run passed or failed,
    // so serve is not left with a dangling connection. The driver's typed
    // `HeadlessError` folds into `anyhow` here — the binary layer only reports.
    runtime.shutdown().await.ok();
    result.map(|_| ()).map_err(anyhow::Error::from)
}

/// On non-unix platforms the core runtime is unavailable (it speaks a unix
/// domain socket), so `medulla run` cannot attach anything.
#[cfg(not(unix))]
pub(crate) async fn run_core(_args: &[String]) -> anyhow::Result<()> {
    // Validate the command line first so a scripted caller still gets the usage
    // error, then report that the runtime itself is unsupported here.
    parse_run_args(_args).map_err(|e| anyhow::anyhow!(e))?;
    anyhow::bail!("medulla run requires the core runtime, which is unix-only")
}
