//! CLI data model: the top-level [`Command`], the per-subcommand parsed-flag
//! structs ([`LoginArgs`], [`MemoryArgs`]/[`MemoryAction`], [`UpdateArgs`],
//! [`TuiArgs`]), and the core-runtime [`CorePlan`] decision. The parsers that
//! build these live in the sibling `parse` module and the socket planning in
//! `plan`.

use std::path::PathBuf;

use medulla::auth::Provider;
use medulla::tinyplace::HarnessProvider;

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
