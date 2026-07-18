//! Pure, testable CLI plumbing for `main`: subcommand dispatch, TUI flag
//! parsing, help text, the `sessions` JSON, and the runtime-selection decision
//! (core → backend → mock). I/O-bound work (connecting sockets, reading the
//! terminal) stays in `main`; everything here is a pure function over its
//! inputs so it can be unit-tested without a TTY or a live core/backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::auth::{Credentials, Provider};
use crate::config::BackendConfig;
use crate::session_history::list_recent_sessions;
use crate::tinyplace_support::HarnessProvider;

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
    /// Log in to the backend (loopback OAuth or a one-time token).
    Login,
    /// Clear stored credentials.
    Logout,
    /// Persona-memory management (`status`/`ingest`/`backfill`/`compile`/`search`).
    Memory,
    /// Launch a coding-agent CLI as a transparent tiny.place-bridged wrapper.
    Wrapper(HarnessProvider),
    /// Check for / install a newer release (`update [--check]`).
    Update,
}

/// Dispatch on the first argument. Anything else (including TUI flags) is the TUI.
pub fn parse_command(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("daemon") => Command::Daemon,
        Some("version") | Some("--version") | Some("-v") => Command::Version,
        Some("help") | Some("--help") | Some("-h") => Command::Help,
        Some("sessions") => Command::Sessions,
        Some("login") => Command::Login,
        Some("logout") => Command::Logout,
        Some("memory") => Command::Memory,
        Some("update") => Command::Update,
        Some("codex") => Command::Wrapper(HarnessProvider::Codex),
        Some("claude") => Command::Wrapper(HarnessProvider::Claude),
        Some("opencode") => Command::Wrapper(HarnessProvider::Opencode),
        _ => Command::Tui,
    }
}

/// Parsed `medulla login` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoginArgs {
    /// Explicit `--config` path used only to resolve `backend.baseUrl`. `None`
    /// selects the layered config discovery.
    pub config: Option<String>,
    pub provider: Provider,
    pub no_browser: bool,
    /// A 64-hex one-time login token (headless fallback); skips the listener.
    pub token: Option<String>,
}

/// Parse `medulla login` flags out of the args following `login`. Returns the
/// offending flag name on an unknown `--provider` value.
pub fn parse_login_args(args: &[String]) -> Result<LoginArgs, String> {
    let mut out = LoginArgs::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                if let Some(v) = it.next() {
                    out.config = Some(v.clone());
                }
            }
            "--provider" => {
                if let Some(v) = it.next() {
                    out.provider =
                        Provider::parse(v).ok_or_else(|| format!("unknown provider '{v}'"))?;
                }
            }
            "--token" => {
                if let Some(v) = it.next() {
                    out.token = Some(v.clone());
                }
            }
            "--no-browser" => out.no_browser = true,
            _ => {}
        }
    }
    Ok(out)
}

/// The `medulla memory` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryAction {
    /// Print the memory-layer status.
    Status,
    /// Incremental ingest pass (LLM-backed; needs an API key).
    Ingest,
    /// Full backfill ingest pass (LLM-backed; needs an API key).
    Backfill,
    /// Recompile the pack from the persisted trees (offline).
    Compile,
    /// BM25 search over the persona corpus (offline).
    Search(String),
}

/// Parsed `medulla memory` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery.
    pub config: Option<String>,
    /// Emit JSON instead of human-readable output.
    pub json: bool,
    /// Optional `--facet <name>` filter (search only).
    pub facet: Option<String>,
    /// Optional `--k <n>` result cap (search only).
    pub k: usize,
    /// The selected action.
    pub action: MemoryAction,
}

/// Parse `medulla memory <action> [flags]`. Returns a usage error on a missing
/// or unknown action, or a `search` with no query.
pub fn parse_memory_args(args: &[String]) -> Result<MemoryArgs, String> {
    let action_word = args.first().map(String::as_str).ok_or_else(|| {
        "expected a subcommand: status|ingest|backfill|compile|search".to_string()
    })?;

    let mut config: Option<String> = None;
    let mut json = false;
    let mut facet: Option<String> = None;
    let mut k: usize = 5;
    let mut query_parts: Vec<String> = Vec::new();

    let mut it = args.iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => config = it.next().cloned(),
            "--json" => json = true,
            "--facet" => facet = it.next().cloned(),
            "--k" => {
                if let Some(v) = it.next() {
                    k = v
                        .parse::<usize>()
                        .map_err(|_| format!("invalid --k value '{v}'"))?;
                }
            }
            other => query_parts.push(other.to_string()),
        }
    }

    let action = match action_word {
        "status" => MemoryAction::Status,
        "ingest" => MemoryAction::Ingest,
        "backfill" => MemoryAction::Backfill,
        "compile" => MemoryAction::Compile,
        "search" => {
            let query = query_parts.join(" ");
            if query.trim().is_empty() {
                return Err("memory search: expected a query".to_string());
            }
            MemoryAction::Search(query)
        }
        other => return Err(format!("unknown memory subcommand '{other}'")),
    };

    Ok(MemoryArgs {
        config,
        json,
        facet,
        k,
        action,
    })
}

/// Parsed `medulla update` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateArgs {
    /// `--check`: report whether an update is available and exit without
    /// installing anything.
    pub check: bool,
}

/// Parse the flags following `medulla update`.
pub fn parse_update_args(args: &[String]) -> UpdateArgs {
    let mut out = UpdateArgs::default();
    for arg in args {
        if arg == "--check" {
            out.check = true;
        }
    }
    out
}

/// Parsed TUI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery
    /// (`./.medulla/config.toml` / `./medulla.toml` / `<home>/config.toml`).
    pub config: Option<String>,
    pub alt_screen: bool,
    pub core: bool,
}

impl Default for TuiArgs {
    fn default() -> Self {
        TuiArgs {
            config: None,
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
                    out.config = Some(v.clone());
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
medulla codex [args]    Run Codex in your terminal, bridged to tiny.place\n  \
medulla claude [args]   Run Claude Code in your terminal, bridged to tiny.place\n  \
medulla opencode [args] Run OpenCode in your terminal, bridged to tiny.place\n  \
medulla login [flags]   Log in to the backend and store credentials\n  \
medulla logout          Clear stored credentials\n  \
medulla memory <cmd>    Persona memory: status|ingest|backfill|compile|search <query>\n  \
medulla update [--check] Update to the latest release (--check only reports)\n  \
medulla version         Print the version\n  \
medulla help            Show this help\n\n\
Wrapper flags:\n  \
--no-bridge             Run the CLI as a plain passthrough (no tiny.place bridge)\n  \
--                      Pass all following arguments to the CLI verbatim\n\n\
Login flags:\n  \
--provider <name>       OAuth provider: google (default), github, twitter, discord\n  \
--no-browser            Print the login URL without launching a browser\n  \
--token <64-hex>        Redeem a one-time login token instead (headless)\n  \
--config <path>         Config file to read backend.baseUrl from (.toml or .json)\n\n\
Memory flags:\n  \
--json                  Emit JSON instead of human-readable output\n  \
--facet <name>          Restrict a search to one facet\n  \
--k <n>                 Max search results (default 5)\n  \
--config <path>         Explicit config file (.toml or .json) for the memory section\n\n\
TUI flags:\n  \
--config <path>         Explicit config file (.toml or .json); bypasses layered discovery\n  \
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

/// Resolve the core socket path (§1.1). An explicit `override_path` (the `--core`
/// flag or `[core].socketPath` config) wins; otherwise `$XDG_RUNTIME_DIR/medulla/
/// core.sock`, then `<state_dir>/core.sock`. `None` when nothing is available.
///
/// Pure path logic with no socket API, so it lives here (a cross-platform module)
/// rather than in the unix-only `runtime::core_client`; that keeps
/// [`core_socket_plan`] compiling on Windows.
pub fn resolve_socket_path(
    override_path: Option<&str>,
    runtime_dir: Option<&str>,
    state_dir: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = override_path.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(dir) = runtime_dir.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("medulla").join("core.sock"));
    }
    if let Some(dir) = state_dir.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("core.sock"));
    }
    None
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

/// Resolve the backend token from, in order: an inline `backend.token`, the
/// `backend.tokenEnv` variable (ignoring an empty value), then `stored`
/// credentials saved by `medulla login` — but only when their `baseUrl` matches
/// the configured backend (a mismatch is ignored).
pub fn resolve_backend_token(
    env: &HashMap<String, String>,
    backend: &BackendConfig,
    stored: Option<&Credentials>,
) -> Option<String> {
    if let Some(tok) = backend.token.clone() {
        return Some(tok);
    }
    if let Some(tok) = env
        .get(&backend.token_env)
        .cloned()
        .filter(|s| !s.is_empty())
    {
        return Some(tok);
    }
    let want = backend.base_url.trim_end_matches('/');
    stored
        .filter(|c| c.base_url.trim_end_matches('/') == want)
        .map(|c| c.jwt.clone())
}

/// The status note shown when no backend token is available and the mock runs.
pub fn missing_token_note(backend: &BackendConfig) -> String {
    format!(
        "backend token missing (set ${} or run `medulla login`) — running with mock runtime",
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
        assert_eq!(parse_command(&argv(&["login"])), Command::Login);
        assert_eq!(parse_command(&argv(&["logout"])), Command::Logout);
        assert_eq!(
            parse_command(&argv(&["codex", "resume"])),
            Command::Wrapper(HarnessProvider::Codex)
        );
        assert_eq!(
            parse_command(&argv(&["claude"])),
            Command::Wrapper(HarnessProvider::Claude)
        );
        assert_eq!(
            parse_command(&argv(&["opencode", "--foo"])),
            Command::Wrapper(HarnessProvider::Opencode)
        );
        assert_eq!(parse_command(&argv(&["memory", "status"])), Command::Memory);
        assert_eq!(parse_command(&argv(&["update"])), Command::Update);
        assert_eq!(
            parse_command(&argv(&["update", "--check"])),
            Command::Update
        );
        assert_eq!(parse_command(&argv(&["--config", "x.json"])), Command::Tui);
    }

    #[test]
    fn update_args_parse() {
        assert_eq!(parse_update_args(&argv(&[])), UpdateArgs { check: false });
        assert_eq!(
            parse_update_args(&argv(&["--check"])),
            UpdateArgs { check: true }
        );
        // Unknown flags are ignored.
        assert_eq!(
            parse_update_args(&argv(&["--check", "--force"])),
            UpdateArgs { check: true }
        );
    }

    #[test]
    fn memory_args_parse() {
        // Simple actions.
        assert_eq!(
            parse_memory_args(&argv(&["status"])).unwrap().action,
            MemoryAction::Status
        );
        assert_eq!(
            parse_memory_args(&argv(&["backfill"])).unwrap().action,
            MemoryAction::Backfill
        );
        // Search joins the query words and honors flags.
        let a = parse_memory_args(&argv(&[
            "search", "how", "to", "test", "--facet", "stack", "--k", "3", "--json", "--config",
            "c.toml",
        ]))
        .unwrap();
        assert_eq!(a.action, MemoryAction::Search("how to test".into()));
        assert_eq!(a.facet.as_deref(), Some("stack"));
        assert_eq!(a.k, 3);
        assert!(a.json);
        assert_eq!(a.config.as_deref(), Some("c.toml"));
        // Errors: no action, empty search, unknown action, bad --k.
        assert!(parse_memory_args(&argv(&[])).is_err());
        assert!(parse_memory_args(&argv(&["search"])).is_err());
        assert!(parse_memory_args(&argv(&["frobnicate"])).is_err());
        assert!(parse_memory_args(&argv(&["search", "q", "--k", "nan"])).is_err());
    }

    #[test]
    fn parses_tui_flags() {
        assert_eq!(parse_tui_args(&argv(&[])), TuiArgs::default());
        let a = parse_tui_args(&argv(&["--config", "c.json", "--core", "--no-alt-screen"]));
        assert_eq!(
            a,
            TuiArgs {
                config: Some("c.json".into()),
                alt_screen: false,
                core: true
            }
        );
        // A dangling --config keeps the default (None → layered discovery).
        assert_eq!(parse_tui_args(&argv(&["--config"])).config, None);
    }

    #[test]
    fn help_names_the_binary() {
        let text = help_text();
        assert!(text.starts_with("medulla "));
        assert!(text.contains("--no-alt-screen"));
    }

    #[test]
    fn resolve_prefers_override_then_xdg_then_state() {
        let over = resolve_socket_path(Some("/tmp/x.sock"), Some("/run/user/1000"), Some("/state"));
        assert_eq!(over.unwrap(), PathBuf::from("/tmp/x.sock"));

        let xdg = resolve_socket_path(None, Some("/run/user/1000"), Some("/state"));
        assert_eq!(
            xdg.unwrap(),
            PathBuf::from("/run/user/1000/medulla/core.sock")
        );

        let state = resolve_socket_path(None, None, Some("/state"));
        assert_eq!(state.unwrap(), PathBuf::from("/state/core.sock"));

        assert!(resolve_socket_path(None, None, None).is_none());
        // Empty strings are treated as unset.
        assert!(resolve_socket_path(Some(""), None, None).is_none());
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
            resolve_backend_token(&env, &backend, None).as_deref(),
            Some("from-env")
        );
        backend.token = Some("inline".into());
        assert_eq!(
            resolve_backend_token(&env, &backend, None).as_deref(),
            Some("inline")
        );

        let empty = HashMap::new();
        let backend = BackendConfig::default();
        assert_eq!(resolve_backend_token(&empty, &backend, None), None);
    }

    #[test]
    fn backend_token_ignores_empty_env_value() {
        let mut env = HashMap::new();
        env.insert("MEDULLA_TOKEN".to_string(), String::new());
        let backend = BackendConfig::default();
        // An empty env value is treated as absent.
        assert_eq!(resolve_backend_token(&env, &backend, None), None);
    }

    #[test]
    fn backend_token_uses_stored_credentials_when_baseurl_matches() {
        let empty = HashMap::new();
        let backend = BackendConfig::default();
        let matching = Credentials {
            base_url: backend.base_url.clone(),
            jwt: "stored-jwt".into(),
        };
        // Config token and env absent → stored credentials are used.
        assert_eq!(
            resolve_backend_token(&empty, &backend, Some(&matching)).as_deref(),
            Some("stored-jwt")
        );

        // A mismatched baseUrl is ignored.
        let mismatched = Credentials {
            base_url: "http://other:9999".into(),
            jwt: "stored-jwt".into(),
        };
        assert_eq!(
            resolve_backend_token(&empty, &backend, Some(&mismatched)),
            None
        );

        // Config token and env still win over stored credentials.
        let mut env = HashMap::new();
        env.insert("MEDULLA_TOKEN".to_string(), "from-env".to_string());
        assert_eq!(
            resolve_backend_token(&env, &backend, Some(&matching)).as_deref(),
            Some("from-env")
        );
    }

    #[test]
    fn login_args_parse() {
        assert_eq!(parse_login_args(&argv(&[])).unwrap(), LoginArgs::default());
        let a = parse_login_args(&argv(&[
            "--provider",
            "github",
            "--no-browser",
            "--token",
            "deadbeef",
            "--config",
            "c.json",
        ]))
        .unwrap();
        assert_eq!(a.provider, Provider::Github);
        assert!(a.no_browser);
        assert_eq!(a.token.as_deref(), Some("deadbeef"));
        assert_eq!(a.config.as_deref(), Some("c.json"));
        // Unknown provider is a friendly error.
        assert!(parse_login_args(&argv(&["--provider", "myspace"])).is_err());
    }

    #[test]
    fn missing_token_note_names_the_env_var() {
        let backend = BackendConfig::default();
        let note = missing_token_note(&backend);
        assert!(note.contains("MEDULLA_TOKEN"));
        assert!(note.contains("mock runtime"));
        assert!(note.contains("medulla login"));
    }

    #[test]
    fn help_text_carries_crate_version() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("medulla daemon"));
        assert!(text.contains("medulla login"));
        assert!(text.contains("medulla codex"));
        assert!(text.contains("--no-bridge"));
        assert!(text.contains("--provider"));
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
        assert_eq!(d.config, None);
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
