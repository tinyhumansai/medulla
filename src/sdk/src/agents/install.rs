//! Writing the built-in catalog into the store.
//!
//! The built-in roles are already TOML documents (see [`super::defaults`]), so
//! installing copies their bytes out verbatim rather than re-serializing parsed
//! templates. What lands in `~/.medulla/agents` is exactly what ships — header
//! comments, field order, and prose layout included — which is what makes the
//! result something an operator can read and edit rather than a machine dump.
//!
//! Installing never overwrites. A file that already exists is reported as
//! skipped, so running it twice cannot silently discard an edit, and a role
//! deleted on purpose is the only one that comes back.

use std::path::{Path, PathBuf};

use super::defaults::default_template_files;

/// What an install wrote, and what it left alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstallOutcome {
    /// The directory written to.
    pub dir: PathBuf,
    /// Filenames written as new files.
    pub written: Vec<String>,
    /// Filenames that already existed and were left untouched.
    pub skipped: Vec<String>,
}

impl InstallOutcome {
    /// A one-line operator-facing summary, for a status line.
    pub fn summary(&self) -> String {
        match (self.written.len(), self.skipped.len()) {
            (0, 0) => format!("No templates to install into {}", self.dir.display()),
            (0, skipped) => format!(
                "{skipped} template{} already installed in {}",
                plural(skipped),
                self.dir.display()
            ),
            (written, 0) => format!(
                "Installed {written} template{} into {}",
                plural(written),
                self.dir.display()
            ),
            (written, skipped) => format!(
                "Installed {written} template{} into {} ({skipped} already there)",
                plural(written),
                self.dir.display()
            ),
        }
    }
}

/// The plural suffix for `n` items.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Write the built-in catalog into `dir`, one file per template.
///
/// Creates `dir` (and its parents) if needed. Existing files are never
/// overwritten — they are reported in [`InstallOutcome::skipped`]. Fails only
/// when the directory cannot be created or a new file cannot be written.
pub fn install_default_templates(dir: &Path) -> std::io::Result<InstallOutcome> {
    std::fs::create_dir_all(dir)?;
    let mut outcome = InstallOutcome {
        dir: dir.to_path_buf(),
        ..Default::default()
    };
    for (name, body) in default_template_files() {
        let path = dir.join(name);
        if path.exists() {
            outcome.skipped.push((*name).to_string());
            continue;
        }
        std::fs::write(&path, body)?;
        outcome.written.push((*name).to_string());
    }
    Ok(outcome)
}
