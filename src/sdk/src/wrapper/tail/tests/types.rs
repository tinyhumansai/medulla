//! Test-only data types for transcript tailing.

use std::collections::HashMap;
use std::path::PathBuf;

/// Scratch paths and environment used by a transcript-tail test.
pub(super) struct Fixture {
    pub(super) dir: PathBuf,
    pub(super) codex_dir: PathBuf,
    pub(super) env: HashMap<String, String>,
    pub(super) cwd: String,
}
