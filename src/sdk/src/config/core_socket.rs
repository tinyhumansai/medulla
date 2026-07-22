//! Core (`medulla-serve`) socket resolution and validation: where the unix
//! socket path the core runtime attaches to comes from (`--core-socket` flag,
//! `MEDULLA_CORE_SOCKET` env var, `[core]` config section, or the default
//! runtime dir), and the fail-fast check that a resolved path is actually
//! attachable *before* [`CoreRuntime::attach`] is handed a value it can only
//! spin on. AGENTS.md treats socket paths as untrusted configuration to be
//! validated at boundaries — this module is that boundary.
//!
//! [`CoreRuntime::attach`]: crate::runtime::core::CoreRuntime::attach

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use super::types::LoadedConfig;

/// Where a resolved core socket path came from, so a validation error can tell
/// the operator *which* knob to fix (flag vs env var vs config file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreSocketSource {
    /// An explicit `--core-socket <path>` command-line flag.
    CliFlag,
    /// The `MEDULLA_CORE_SOCKET` environment variable.
    EnvVar,
    /// The `[core]` config section (its `socketPath`, or the resolved default).
    ConfigSection,
    /// No explicit opt-in: the default runtime-dir path
    /// (`$XDG_RUNTIME_DIR/medulla/serve.sock`, then `<stateDir>/serve.sock`),
    /// used by `medulla run` which always drives the core runtime.
    DefaultPath,
}

impl fmt::Display for CoreSocketSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CoreSocketSource::CliFlag => "the --core-socket flag",
            CoreSocketSource::EnvVar => "the MEDULLA_CORE_SOCKET environment variable",
            CoreSocketSource::ConfigSection => "the [core] config section",
            CoreSocketSource::DefaultPath => "the default runtime directory",
        })
    }
}

/// Why a resolved core socket path cannot be attached. Typed so the binary
/// layers (`medulla run`, the TUI startup) can fail fast with an operator-facing
/// message naming the path *and* how it was configured, instead of handing the
/// bad path to the runtime driver to spin on in reconnect.
#[derive(Debug, thiserror::Error)]
pub enum CoreSocketError {
    /// The path exists on disk but is not a unix domain socket (a regular file,
    /// a directory, …) — attaching can never succeed, so this is a config error.
    #[error(
        "core socket path {} (from {origin}) exists but is not a unix socket — \
         point it at a listening medulla-serve socket",
        path.display()
    )]
    NotASocket {
        /// The offending resolved path.
        path: PathBuf,
        /// Which knob produced it. Named `origin` (not `source`) so thiserror
        /// does not treat it as an error-chain source.
        origin: CoreSocketSource,
    },
}

/// Validate that `path` can plausibly be attached as a `medulla-serve` socket.
///
/// A *missing* path passes: attach-before-serve is a supported flow (the
/// runtime driver waits for the socket to appear, and the headless driver
/// bounds that wait with its ready timeout). An *existing* non-socket path can
/// never be attached, so it fails fast with a [`CoreSocketError`] naming the
/// path and its source. On non-unix platforms the check is a no-op (the core
/// runtime does not exist there).
pub fn validate_core_socket(path: &Path, source: CoreSocketSource) -> Result<(), CoreSocketError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if !meta.file_type().is_socket() {
                return Err(CoreSocketError::NotASocket {
                    path: path.to_path_buf(),
                    origin: source,
                });
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (path, source);
    Ok(())
}

impl LoadedConfig {
    /// Resolve the NDJSON `medulla-serve` socket path for the core runtime.
    ///
    /// Precedence follows the serve-protocol transport contract (plan §2.2): an
    /// explicit `[core] socketPath` wins; otherwise
    /// `$XDG_RUNTIME_DIR/medulla/serve.sock`, then `<stateDir>/serve.sock`. A
    /// blank or whitespace-only `socketPath` is treated as unset. The returned
    /// path is where the runtime *attaches*; this milestone never spawns serve,
    /// so a missing socket surfaces as an attach error, not a spawn.
    pub fn core_socket_path(&self, env: &HashMap<String, String>) -> PathBuf {
        if let Some(explicit) = self
            .config
            .core
            .as_ref()
            .and_then(|c| c.socket_path.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return PathBuf::from(explicit);
        }
        if let Some(xdg) = env
            .get("XDG_RUNTIME_DIR")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return PathBuf::from(xdg).join("medulla").join("serve.sock");
        }
        PathBuf::from(&self.config.state_dir).join("serve.sock")
    }

    /// Whether the core (`medulla-serve`) runtime is *requested*, and if so the
    /// socket to attach to.
    ///
    /// Opting into the core runtime is explicit — the backend runtime stays the
    /// default. A request comes from, in precedence order: an explicit
    /// `--core-socket <path>` (`cli_socket`), the `MEDULLA_CORE_SOCKET`
    /// environment variable, or the presence of a `[core]` config section. The
    /// first two carry the socket path directly; a `[core]` section resolves the
    /// path through [`core_socket_path`](Self::core_socket_path) (explicit
    /// `socketPath`, then `$XDG_RUNTIME_DIR`, then the state dir). Blank/whitespace
    /// values for the flag and env var are treated as unset so an empty override
    /// never masks the config. Returns `None` when nothing opts in, which the
    /// caller reads as "use the backend/mock chain".
    pub fn core_socket_request(
        &self,
        env: &HashMap<String, String>,
        cli_socket: Option<&str>,
    ) -> Option<PathBuf> {
        self.core_socket_request_sourced(env, cli_socket)
            .map(|(path, _)| path)
    }

    /// [`core_socket_request`](Self::core_socket_request), but also naming
    /// *which* knob supplied the path, so the caller can validate it (see
    /// [`validate_core_socket`]) with an error message pointing at the right
    /// place to fix.
    pub fn core_socket_request_sourced(
        &self,
        env: &HashMap<String, String>,
        cli_socket: Option<&str>,
    ) -> Option<(PathBuf, CoreSocketSource)> {
        if let Some(explicit) = cli_socket.map(str::trim).filter(|s| !s.is_empty()) {
            return Some((PathBuf::from(explicit), CoreSocketSource::CliFlag));
        }
        if let Some(from_env) = env
            .get("MEDULLA_CORE_SOCKET")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Some((PathBuf::from(from_env), CoreSocketSource::EnvVar));
        }
        if self.config.core.is_some() {
            return Some((self.core_socket_path(env), CoreSocketSource::ConfigSection));
        }
        None
    }
}
