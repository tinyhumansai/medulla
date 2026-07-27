//! The built-in agent-template catalog: the coding roles every install knows
//! without any files on disk.
//!
//! The roles live beside this module as TOML — the same format the
//! `.medulla/agents` store reads — and are compiled in with `include_str!`.
//! That is the point: the built-in catalog and an installed one are the same
//! documents, so what a contributor edits here is byte-for-byte what an
//! operator gets on disk, and adding a role is adding a file rather than
//! writing a constructor call.
//!
//! Two rules keep the defaults out of the way. They claim **no harness kind**:
//! an empty `harnesses` map means "runs anywhere", so a built-in role is never
//! the reason a workspace has nothing it may provision. And they are always
//! **outranked** by a template of the same id from the store or the config file.

use crate::runtime::fleet::AgentTemplate;

use super::store::parse_template;

/// The catalog's source documents, in the order the work happens: plan,
/// implement, test, review, then the roles that close a change out.
///
/// The order is the catalog's render order, so it is declared here rather than
/// derived from a directory listing — and `include_str!` needs literal paths
/// regardless. A new role is a new `.toml` file plus a line here.
const SOURCES: &[(&str, &str)] = &[
    ("plan-writer.toml", include_str!("plan-writer.toml")),
    ("implementer.toml", include_str!("implementer.toml")),
    ("test-writer.toml", include_str!("test-writer.toml")),
    ("code-reviewer.toml", include_str!("code-reviewer.toml")),
    ("debugger.toml", include_str!("debugger.toml")),
    ("verifier.toml", include_str!("verifier.toml")),
    ("doc-writer.toml", include_str!("doc-writer.toml")),
    ("refactorer.toml", include_str!("refactorer.toml")),
    ("merge-resolver.toml", include_str!("merge-resolver.toml")),
    ("pr-manager.toml", include_str!("pr-manager.toml")),
    ("triager.toml", include_str!("triager.toml")),
    (
        "repo-orchestrator.toml",
        include_str!("repo-orchestrator.toml"),
    ),
];

/// The default coding catalog, parsed from the embedded documents.
///
/// A document that fails to parse is skipped rather than panicking — a library
/// call has no business aborting a process over its own asset — and cannot
/// reach a release: [`super::tests`] asserts every source parses and that the
/// full id list is intact.
pub fn default_templates() -> Vec<AgentTemplate> {
    default_template_files()
        .iter()
        .filter_map(|(name, body)| parse_template(body, stem(name)).ok())
        .collect()
}

/// The catalog as `(filename, contents)`, for the installer.
///
/// Installing copies these bytes out verbatim instead of re-serializing a
/// parsed template, so the file an operator opens carries the comments and the
/// prose layout that were written for them.
pub fn default_template_files() -> &'static [(&'static str, &'static str)] {
    SOURCES
}

/// A source filename without its `.toml` extension — the id a document falls
/// back to when it does not name one.
fn stem(name: &str) -> &str {
    name.strip_suffix(".toml").unwrap_or(name)
}
