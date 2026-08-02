//! `medulla mcp` — serve Medulla's tools over MCP on stdin/stdout.
//!
//! Not a command a person runs. Medulla spawns this itself when it starts a
//! harness over ACP, handing the child the control socket path and a grant
//! token in its environment; that grant is what decides which tool families the
//! session is served. Run by hand, with no grant in the environment, it serves
//! the workflow tools off the local store and advertises no fleet.

use std::collections::HashMap;
use std::path::PathBuf;

/// Run `medulla mcp`.
///
/// Takes over stdin and stdout for the life of the process — the MCP stdio
/// transport owns both — and returns only when the client closes the stream.
///
/// # Errors
///
/// Returns an error only when stdin or stdout fails. A malformed request is
/// answered with a JSON-RPC error and the loop continues.
pub(crate) async fn run_mcp_cmd(args: &[String]) -> anyhow::Result<()> {
    // Recorded into the environment before serving, for the same reason the TUI
    // does it: this process may itself resolve config, and a `--config` the
    // operator chose must not be silently replaced by whatever the harness's
    // working directory happens to discover.
    if let Some(path) = explicit_config(args) {
        std::env::set_var(medulla::config::CONFIG_PATH_ENV, path);
    }
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    medulla::mcp::serve_stdio(&env, &cwd)
        .await
        .map_err(anyhow::Error::from)
}

/// The `--config <path>` value, if one was passed.
fn explicit_config(args: &[String]) -> Option<String> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--config" {
            return rest.next().cloned();
        }
        if let Some(path) = arg.strip_prefix("--config=") {
            return Some(path.to_string());
        }
    }
    None
}
