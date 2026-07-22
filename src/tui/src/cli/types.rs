//! CLI data model: the top-level [`Command`], the per-subcommand parsed-flag
//! structs ([`LoginArgs`], [`MemoryArgs`]/[`MemoryAction`], [`UpdateArgs`],
//! [`TuiArgs`]). The parsers that build these live in the sibling `parse`
//! module.

use medulla::auth::Provider;
use medulla::tinyplace::HarnessProvider;

/// The top-level subcommand selected from `argv[1..]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the interactive TUI (bare invocation, or with TUI flags).
    Tui,
    /// Run the non-interactive core-runtime driver: submit one instruction over
    /// a `medulla-serve` socket and stream the cycle's events as JSON lines.
    Run,
    /// Run the headless daemon; carries the remaining args.
    Daemon,
    /// `medulla daemon --tui` — the worker daemon with its operator screen.
    DaemonTui,
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
    /// Author a `MEDULLA.md` workspace profile for a directory.
    Init,
    /// Run the orchestrator hub: relay hosted-backend tasks to tiny.place
    /// workers over Signal DMs; carries the remaining args.
    Hub,
}

/// Parsed `medulla init` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InitArgs {
    /// Directory to initialise. `None` means the current working directory.
    pub dir: Option<String>,
    /// Explicit `--config` path used to resolve the backend/model settings.
    /// `None` selects the layered config discovery.
    pub config: Option<String>,
    /// `--force`: overwrite an existing `MEDULLA.md`.
    pub force: bool,
    /// `--offline`: skip the model call and write the editable stub.
    pub offline: bool,
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

/// Parsed `medulla update` flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateArgs {
    /// `--check`: report whether an update is available and exit without
    /// installing anything.
    pub check: bool,
}

/// Parsed TUI flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery
    /// (`./.medulla/config.toml` / `./medulla.toml` / `<home>/config.toml`).
    pub config: Option<String>,
    pub alt_screen: bool,
    /// Force the offline demo runtime, skipping the token lookup and the login
    /// screen. The only headless way to reach a working runtime with no backend.
    pub mock: bool,
    /// Explicit `--core-socket <path>`: attach the core (`medulla-serve`) runtime
    /// at this socket instead of the backend. `None` leaves the choice to the
    /// `[core]` config section / `MEDULLA_CORE_SOCKET` env (see
    /// [`LoadedConfig::core_socket_request`](medulla::config::LoadedConfig::core_socket_request)).
    pub core_socket: Option<String>,
}

impl Default for TuiArgs {
    fn default() -> Self {
        TuiArgs {
            config: None,
            alt_screen: true,
            mock: false,
            core_socket: None,
        }
    }
}

/// Parsed `medulla run` flags: the non-interactive one-instruction core-runtime
/// driver for scripting / e2e.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunArgs {
    /// Explicit `--config` path. `None` selects the layered config discovery.
    pub config: Option<String>,
    /// Explicit `--core-socket <path>` override. `None` resolves the socket from
    /// the config / `MEDULLA_CORE_SOCKET` env / the default runtime dir.
    pub core_socket: Option<String>,
    /// The instruction to submit — every non-flag argument, joined by spaces.
    pub instruction: String,
}
