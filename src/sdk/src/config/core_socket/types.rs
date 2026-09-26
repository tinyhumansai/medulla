//! Data types for the `core_socket` module.
#[allow(unused_imports)]
use super::*;
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
