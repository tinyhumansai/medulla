//! Where agent templates come from: the built-in coding catalog, the on-disk
//! `.medulla/agents/*.toml` store that supersedes it, and the installer that
//! writes the catalog into that store so it can be edited.
//!
//! A template is a declaration — a role, its instructions, the tools it may use
//! — and a declaration belongs in a file the operator can open, not only in a
//! compiled constant. So the catalog exists twice, on purpose:
//!
//! - [`defaults`] holds the roles every install knows without any files. They
//!   are what a fresh client declares, and the corpus the installer writes.
//! - [`store`] reads `<medulla home>/agents/*.toml` and the project-local
//!   `./.medulla/agents/*.toml`, one template per file. Any file at all in
//!   those directories replaces the built-ins wholesale, so deleting a role you
//!   do not want makes it stay deleted.
//! - [`install`] writes the built-ins into the store as readable TOML, skipping
//!   files that already exist. It is explicit — nothing here writes to disk
//!   unless a caller asks — because a tool that seeds files behind your back is
//!   a tool you cannot trust with a directory you edit.
//!
//! Precedence, lowest to highest: built-in defaults, the user-global store, the
//! project-local store, and finally `[fleet].agentTemplates` in the config file,
//! which the config layer applies. Every layer merges by template id.

mod defaults;
mod install;
mod store;
#[cfg(test)]
mod tests;

pub use defaults::default_templates;
pub use install::{install_default_templates, InstallOutcome};
pub use store::{load_templates, merge_templates, template_dirs, TemplateStore};
