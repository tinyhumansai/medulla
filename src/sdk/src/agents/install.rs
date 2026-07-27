//! Writing the built-in catalog into the store as editable TOML.
//!
//! The point of installing is to get files you can open, so this renders them
//! by hand rather than through a generic serializer: field order matches how a
//! template reads (what it is, then what it may use, then what it is told), and
//! the instructions land in a multi-line literal string instead of one long
//! escaped line.
//!
//! Installing never overwrites. A file that already exists is reported as
//! skipped, so running it twice cannot silently discard an edit, and a role you
//! deleted on purpose is the only one that comes back.

use std::path::{Path, PathBuf};

use crate::runtime::fleet::AgentTemplate;

use super::defaults::default_templates;

/// What an install wrote, and what it left alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstallOutcome {
    /// The directory written to.
    pub dir: PathBuf,
    /// Template ids written as new files.
    pub written: Vec<String>,
    /// Template ids whose file already existed and was left untouched.
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

/// Write the built-in catalog into `dir`, one `<id>.toml` per template.
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
    for template in default_templates() {
        let path = dir.join(format!("{}.toml", template.id));
        if path.exists() {
            outcome.skipped.push(template.id.clone());
            continue;
        }
        std::fs::write(&path, render_template_toml(&template))?;
        outcome.written.push(template.id.clone());
    }
    Ok(outcome)
}

/// Render one template as the TOML document the store reads back.
///
/// Only the fields the built-ins set are emitted; `params`, `metadata`, and
/// per-harness overrides are documented in the header rather than written as
/// empty tables, which would read as noise in every file.
pub(super) fn render_template_toml(template: &AgentTemplate) -> String {
    let mut out = String::new();
    out.push_str("# A medulla agent template. Edit it, or delete the file to drop the role.\n");
    out.push_str("# Optional extras: `params` (provider parameters), `metadata`, and\n");
    out.push_str("# `[harnesses.<kind>]` tables, whose presence restricts the role to those\n");
    out.push_str("# harness kinds.\n\n");

    push_field(&mut out, "id", &template.id);
    if let Some(name) = &template.name {
        push_field(&mut out, "name", name);
    }
    push_field(&mut out, "description", &template.description);
    if let Some(model) = &template.model {
        push_field(&mut out, "model", model);
    }
    if let Some(effort) = &template.effort {
        push_field(&mut out, "effort", effort);
    }
    if let Some(tools) = &template.tools {
        push_list(&mut out, "tools", tools);
    }
    if !template.tags.is_empty() {
        push_list(&mut out, "tags", &template.tags);
    }
    if let Some(instructions) = &template.instructions {
        out.push_str("\ninstructions = ");
        out.push_str(&multiline(instructions));
        out.push('\n');
    }
    out
}

/// `key = "value"`, escaped as a TOML basic string.
fn push_field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = ");
    out.push_str(&basic_string(value));
    out.push('\n');
}

/// `key = ["a", "b"]`, each element escaped as a TOML basic string.
fn push_list(out: &mut String, key: &str, values: &[String]) {
    let rendered: Vec<String> = values.iter().map(|v| basic_string(v)).collect();
    out.push_str(key);
    out.push_str(" = [");
    out.push_str(&rendered.join(", "));
    out.push_str("]\n");
}

/// A TOML basic string, escaped by the serializer rather than by hand.
fn basic_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// A multi-line literal string (`'''…'''`) for prose, so instructions stay
/// readable and editable in the file.
///
/// Literal strings have no escapes at all, so a value that itself contains
/// `'''` cannot be represented; that falls back to a basic string, which can
/// always represent anything.
fn multiline(value: &str) -> String {
    if value.contains("'''") || value.ends_with('\'') {
        return basic_string(value);
    }
    format!("'''\n{}\n'''", value.trim_end())
}
