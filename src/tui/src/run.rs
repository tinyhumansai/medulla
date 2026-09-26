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
/// instruction to its cycle result. Errors: an unparseable command line, a host
/// with no configured backend or no session, or a cycle that never completes
/// within the headless timeout.
pub(crate) async fn run_core(args: &[String]) -> anyhow::Result<()> {
    use std::sync::Arc;

    use medulla::config::load_config;
    use medulla::runtime::cloud::CloudRuntime;
    use medulla::runtime::headless::{drive_once, HeadlessOptions};
    use medulla::runtime::Runtime;

    let parsed = parse_run_args(args).map_err(|e| anyhow::anyhow!(e))?;

    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let loaded = load_config(parsed.config.as_deref(), &env, &cwd)?;

    // The backend client, built from the same precedence chain every other
    // backend-facing surface uses: an inline `backend.token`, then
    // `backend.tokenEnv`, then the stored session. A scripted run on staging
    // therefore presents the token `medulla login` stored for that deployment,
    // rather than one resolved from ambient state.
    let client = medulla::runtime::cloud::connect::client_from_config(&env, &loaded.config.backend)
        .ok_or_else(|| {
            match medulla::runtime::cloud::connect::readiness(&env, &loaded.config.backend) {
                medulla::runtime::cloud::connect::Readiness::Unusable(why) => {
                    anyhow::anyhow!("{why}")
                }
                _ => anyhow::anyhow!("not signed in — run `medulla login`, or set MEDULLA_TOKEN"),
            }
        })?;
    // The configured workspace roots ride the session mint here for the same
    // reason they do in the TUI: a scripted run against a repository the backend
    // has never seen is exactly the case `MEDULLA.md` profiles exist for, and
    // wiring one path and not the other would make `medulla run` and the app
    // brief a delegated turn differently.
    let workspaces: Vec<std::path::PathBuf> = loaded
        .config
        .workflow
        .workspaces
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let runtime = Arc::new(
        CloudRuntime::new(Arc::new(client))
            .with_backend(loaded.config.backend.clone())
            .with_workspaces(workspaces),
    );
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
