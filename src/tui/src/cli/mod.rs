//! Pure, testable CLI plumbing for `main`: subcommand dispatch, TUI flag
//! parsing, help text, the `sessions` JSON, and the runtime-selection decision
//! (core → backend → mock). I/O-bound work (connecting sockets, reading the
//! terminal) stays in `main`; everything here is a pure function over its
//! inputs so it can be unit-tested without a TTY or a live core/backend.
//!
//! The module is split by responsibility: [`types`] holds the data model
//! (the [`Command`] enum, the parsed-flag structs, and [`CorePlan`]), [`parse`]
//! the argument parsers and [`help_text`], and [`plan`] the session listing and
//! core-socket resolution. All public items are re-exported here so callers use
//! `medulla_tui::cli::*`.

mod parse;
mod plan;
mod types;

#[cfg(test)]
mod tests;

pub use parse::{
    help_text, parse_command, parse_login_args, parse_memory_args, parse_tui_args,
    parse_update_args,
};
pub use plan::{core_socket_plan, resolve_socket_path, sessions_json};
pub use types::{Command, CorePlan, LoginArgs, MemoryAction, MemoryArgs, TuiArgs, UpdateArgs};
