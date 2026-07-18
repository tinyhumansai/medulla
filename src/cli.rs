//! Pure, testable CLI plumbing for `main`: subcommand dispatch, TUI flag
//! parsing, help text, the `sessions` JSON, and the runtime-selection decision
//! (core → backend → mock). I/O-bound work (connecting sockets, reading the
//! terminal) stays in `main`; everything here is a pure function over its
//! inputs so it can be unit-tested without a TTY or a live core/backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::BackendConfig;
use crate::runtime::core_client::resolve_socket_path;
use crate::session_history::list_recent_sessions;

/// The top-level subcommand selected from `argv[1..]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the interactive TUI (bare invocation, or with TUI flags).
    Tui,
    /// Run the headless daemon; carries the remaining args.
    Daemon,
    Version,
    Help,
    Sessions,
}

/// Dispatch on the first argument. Anything else (including TUI flags) is the TUI.
pub fn parse_command(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("daemon") => Command::Daemon,
        Some("version") | Some("--version") | Some("-v") => Command::Version,
        Some("help") | Some("--help") | Some("-h") => Command::Help,
        Some("sessions") => Command::Sessions,
        _ => Command::Tui,
    }
}

/// Parsed TUI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiArgs {
    pub config: String,
    pub alt_screen: bool,
    pub core: bool,
}

impl Default for TuiArgs {
    fn default() -> Self {
        TuiArgs {
            config: "medulla.tui.json".to_string(),
            alt_screen: true,
            core: false,
        }
    }
}

/// Parse the TUI's own flags out of `argv[1..]`.
pub fn parse_tui_args(args: &[String]) -> TuiArgs {
    let mut out = TuiArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = v.clone();
                }
            }
            "--no-alt-screen" => out.alt_screen = false,
            "--core" => out.core = true,
            _ => {}
        }
    }
    out
}

/// The `medulla help` / `--help` text.
pub fn help_text() -> String {
    format!(
        "medulla {version}\n\n\
Usage:\n  \
medulla                 Start the interactive chat TUI (default)\n  \
medulla daemon [flags]  Run the headless coding-agent daemon (serves tasks over tiny.place)\n  \
medulla sessions        List recent claude/codex sessions as JSON\n  \
medulla version         Print the version\n  \
medulla help            Show this help\n\n\
TUI flags:\n  \
--config <path>         Path to medulla.tui.json (default: medulla.tui.json)\n  \
--core                  Drive the core-js orchestration core over its Unix socket\n  \
--no-alt-screen         Do not switch to the alternate screen\n",
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// The `medulla sessions` payload: recent claude/codex sessions as pretty JSON.
pub fn sessions_json(env: &HashMap<String, String>, cwd: &str) -> anyhow::Result<String> {
    let sessions = list_recent_sessions(env, cwd, None, None);
    Ok(serde_json::to_string_pretty(&sessions)?)
}

/// The decision made about the core runtime before any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorePlan {
    /// Core was not requested — go straight to backend/mock.
    Skip,
    /// A present socket path worth a connection attempt.
    Connect(PathBuf),
    /// Core was requested but is unavailable; carry the fallback note.
    Fallback(String),
}

/// Decide whether to attempt the core runtime, resolving and probing its socket.
/// `exists` probes a path (injected so this stays pure and testable).
pub fn core_socket_plan(
    want_core: bool,
    config_socket: Option<&str>,
    runtime_dir: Option<&str>,
    state_dir: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> CorePlan {
    if !want_core {
        return CorePlan::Skip;
    }
    match resolve_socket_path(config_socket, runtime_dir, state_dir) {
        Some(path) if exists(&path) => CorePlan::Connect(path),
        Some(path) => {
            CorePlan::Fallback(format!("core socket {} not present — falling back", path.display()))
        }
        None => CorePlan::Fallback(
            "no core socket resolved (set XDG_RUNTIME_DIR / MEDULLA_STATE_DIR / [core].socketPath) — falling back"
                .into(),
        ),
    }
}

/// Resolve the backend token: an inline `backend.token` wins, else the
/// `backend.tokenEnv` variable (ignoring an empty value).
pub fn resolve_backend_token(
    env: &HashMap<String, String>,
    backend: &BackendConfig,
) -> Option<String> {
    backend.token.clone().or_else(|| {
        env.get(&backend.token_env)
            .cloned()
            .filter(|s| !s.is_empty())
    })
}

/// The status note shown when no backend token is available and the mock runs.
pub fn missing_token_note(backend: &BackendConfig) -> String {
    format!(
        "backend token missing (set ${}) — running with mock runtime",
        backend.token_env
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dispatches_subcommands() {
        assert_eq!(parse_command(&argv(&[])), Command::Tui);
        assert_eq!(parse_command(&argv(&["daemon", "--foo"])), Command::Daemon);
        assert_eq!(parse_command(&argv(&["version"])), Command::Version);
        assert_eq!(parse_command(&argv(&["-v"])), Command::Version);
        assert_eq!(parse_command(&argv(&["help"])), Command::Help);
        assert_eq!(parse_command(&argv(&["-h"])), Command::Help);
        assert_eq!(parse_command(&argv(&["sessions"])), Command::Sessions);
        assert_eq!(parse_command(&argv(&["--config", "x.json"])), Command::Tui);
    }

    #[test]
    fn parses_tui_flags() {
        assert_eq!(parse_tui_args(&argv(&[])), TuiArgs::default());
        let a = parse_tui_args(&argv(&["--config", "c.json", "--core", "--no-alt-screen"]));
        assert_eq!(
            a,
            TuiArgs {
                config: "c.json".into(),
                alt_screen: false,
                core: true
            }
        );
        // A dangling --config keeps the default.
        assert_eq!(
            parse_tui_args(&argv(&["--config"])).config,
            "medulla.tui.json"
        );
    }

    #[test]
    fn help_names_the_binary() {
        let text = help_text();
        assert!(text.starts_with("medulla "));
        assert!(text.contains("--no-alt-screen"));
    }

    #[test]
    fn core_plan_skips_when_not_wanted() {
        let plan = core_socket_plan(false, None, None, None, |_| true);
        assert_eq!(plan, CorePlan::Skip);
    }

    #[test]
    fn core_plan_connects_when_socket_present() {
        let plan = core_socket_plan(true, Some("/run/core.sock"), None, None, |_| true);
        assert_eq!(plan, CorePlan::Connect(PathBuf::from("/run/core.sock")));
    }

    #[test]
    fn core_plan_falls_back_when_absent() {
        let plan = core_socket_plan(true, Some("/run/core.sock"), None, None, |_| false);
        match plan {
            CorePlan::Fallback(note) => assert!(note.contains("not present")),
            other => panic!("expected fallback, got {other:?}"),
        }
        let plan = core_socket_plan(true, None, None, None, |_| false);
        match plan {
            CorePlan::Fallback(note) => assert!(note.contains("no core socket resolved")),
            other => panic!("expected fallback, got {other:?}"),
        }
    }

    #[test]
    fn backend_token_prefers_inline_then_env() {
        let mut env = HashMap::new();
        env.insert("MEDULLA_TOKEN".to_string(), "from-env".to_string());
        let mut backend = BackendConfig::default();
        assert_eq!(
            resolve_backend_token(&env, &backend).as_deref(),
            Some("from-env")
        );
        backend.token = Some("inline".into());
        assert_eq!(
            resolve_backend_token(&env, &backend).as_deref(),
            Some("inline")
        );

        let empty = HashMap::new();
        let backend = BackendConfig::default();
        assert_eq!(resolve_backend_token(&empty, &backend), None);
    }

    #[test]
    fn backend_token_ignores_empty_env_value() {
        let mut env = HashMap::new();
        env.insert("MEDULLA_TOKEN".to_string(), String::new());
        let backend = BackendConfig::default();
        // An empty env value is treated as absent.
        assert_eq!(resolve_backend_token(&env, &backend), None);
    }

    #[test]
    fn missing_token_note_names_the_env_var() {
        let backend = BackendConfig::default();
        let note = missing_token_note(&backend);
        assert!(note.contains("MEDULLA_TOKEN"));
        assert!(note.contains("mock runtime"));
    }

    #[test]
    fn help_text_carries_crate_version() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("medulla daemon"));
        assert!(text.contains("--core"));
    }

    #[test]
    fn sessions_json_is_valid_json_array() {
        // Point the scan dirs at an empty temp path so the result is deterministic
        // ([]), independent of the developer's real ~/.claude / ~/.codex history.
        let tmp = std::env::temp_dir().join(format!("medulla-cli-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let mut env = HashMap::new();
        env.insert(
            "TINYPLACE_CLAUDE_SESSIONS_DIR".to_string(),
            tmp.join("claude").to_string_lossy().into_owned(),
        );
        env.insert(
            "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
            tmp.join("codex").to_string_lossy().into_owned(),
        );
        let json = sessions_json(&env, tmp.to_str().unwrap()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tui_args_default_and_debug() {
        let d = TuiArgs::default();
        assert_eq!(d.config, "medulla.tui.json");
        assert!(d.alt_screen);
        assert!(!d.core);
        // Command derives Debug/Eq for assertions.
        assert!(format!("{:?}", Command::Tui).contains("Tui"));
        assert_ne!(Command::Tui, Command::Daemon);
    }

    #[test]
    fn core_plan_connects_via_state_dir_when_no_config_socket() {
        // resolve_socket_path falls back to <stateDir>/core.sock.
        let plan = core_socket_plan(true, None, None, Some("/var/lib/medulla"), |_| true);
        match plan {
            CorePlan::Connect(path) => {
                assert!(path.ends_with("core.sock"));
            }
            other => panic!("expected connect, got {other:?}"),
        }
    }
}
