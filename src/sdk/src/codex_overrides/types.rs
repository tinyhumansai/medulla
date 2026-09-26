//! The explicit error type this module's two public boundaries return.
//!
//! [`super::write_catalog`] and [`super::launch_args`] are SDK library
//! boundaries, so they name what went wrong rather than returning an opaque
//! `String` — see the crate's coding guidelines. Callers at the TUI and daemon
//! spawn seams convert this to display text with `.to_string()` when they need
//! to fold it into their own `Result<_, String>`.

use std::fmt;
use std::path::PathBuf;

/// Why a routed Codex spawn's `-c` overrides or derived model catalog could not
/// be built.
///
/// Each variant names the file involved, so the message always points at what
/// to run or inspect rather than just "it failed".
#[derive(Debug)]
pub enum CodexOverridesError {
    /// Codex has never cached a model catalog at `path` (the operator has not
    /// run it yet).
    CacheMissing {
        /// The cache file Codex would have written.
        path: PathBuf,
        /// The read error, usually "not found".
        source: std::io::Error,
    },
    /// The cached catalog at `path` is not valid JSON.
    CacheInvalidJson {
        /// The cache file that failed to parse.
        path: PathBuf,
        /// The JSON parse error.
        source: serde_json::Error,
    },
    /// The cache at `path` holds no usable model to derive a catalog entry
    /// from.
    NoUsableTemplate {
        /// The cache file that was read.
        path: PathBuf,
    },
    /// The derived catalog could not be serialized.
    Encode {
        /// The JSON encoding error.
        source: serde_json::Error,
    },
    /// `path`'s parent directory could not be created.
    CreateDir {
        /// The directory that could not be created.
        path: PathBuf,
        /// The filesystem error.
        source: std::io::Error,
    },
    /// The derived catalog could not be written to a temporary file.
    Write {
        /// The temporary file that could not be written.
        path: PathBuf,
        /// The filesystem error.
        source: std::io::Error,
    },
    /// The temporary catalog file could not be renamed into place at `path`.
    Rename {
        /// The final catalog path the rename targeted.
        path: PathBuf,
        /// The filesystem error.
        source: std::io::Error,
    },
}

impl fmt::Display for CodexOverridesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CacheMissing { path, source } => write!(
                f,
                "no Codex model metadata at {}: {source}. Run `codex` once — it caches \
                 the catalog on start — then start this harness again",
                path.display()
            ),
            Self::CacheInvalidJson { path, source } => {
                write!(f, "{} is not valid JSON: {source}", path.display())
            }
            Self::NoUsableTemplate { path } => {
                write!(f, "{} holds no usable model to derive from", path.display())
            }
            Self::Encode { source } => {
                write!(f, "could not encode the derived catalog: {source}")
            }
            Self::CreateDir { path, source } => {
                write!(f, "could not create {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
            Self::Rename { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for CodexOverridesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CacheMissing { source, .. } => Some(source),
            Self::CacheInvalidJson { source, .. } => Some(source),
            Self::NoUsableTemplate { .. } => None,
            Self::Encode { source } => Some(source),
            Self::CreateDir { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::Rename { source, .. } => Some(source),
        }
    }
}
