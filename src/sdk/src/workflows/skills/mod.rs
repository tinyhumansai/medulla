//! Harness-native skills that trigger saved workflows over MCP.
//!
//! `medulla mcp` already serves `workflow_run` to any MCP client, so an
//! operator's own Claude Code or Codex session is one tool call away from
//! starting a saved workflow. What is missing is discovery and phrasing:
//! nothing in that session knows a `babysit` workflow exists, what it takes, or
//! that the way to start it is a tool call with a particular argument shape.
//! This module closes that gap by generating, per workflow, a skill whose body
//! instructs the model to make exactly that call.
//!
//! The pipeline is three steps, one per submodule:
//!
//! 1. [`render`] turns a [`WorkflowSummary`](crate::workflows::WorkflowSummary)
//!    into target-independent skill text — frontmatter the model matches on, a
//!    concrete call example built from the declared inputs, and a fallback for
//!    when the server is not attached.
//! 2. [`targets`] decides where that text lands for each harness.
//! 3. [`install`] writes it, under a marker discipline that never overwrites a
//!    file Medulla did not generate.
//!
//! Everything here is pure filesystem work against a caller-supplied root, so
//! the CLI, a future MCP verb, and a TUI action all drive the same code and a
//! test can point it at a tempdir.

pub mod install;
pub mod registration;
pub mod render;
pub mod targets;
mod types;

#[cfg(test)]
mod marker_tests;
#[cfg(test)]
mod prune_tests;
#[cfg(test)]
mod registration_tests;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod targets_tests;
#[cfg(test)]
mod tests;

pub use install::{install, installed, sync, uninstall};
pub use registration::{register, RegistrationOptions, RegistrationOutcome};
pub use render::{render, render_command, slug_for};
pub use targets::{
    command_path, default_targets, managed_dir, managed_root, scope_root, skill_path, spawn_args,
};
pub use types::{
    FileAction, FileOutcome, InstallOptions, InstallReport, InstalledSkill, RenderedSkill,
    SkillScope, SkillTarget,
};
