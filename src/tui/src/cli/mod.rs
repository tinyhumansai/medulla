//! Pure, testable CLI plumbing for `main`: subcommand dispatch, TUI flag
//! parsing, help text, and the `sessions` JSON. I/O-bound work (reading the
//! terminal) stays in `main`; everything here is a pure function over its
//! inputs so it can be unit-tested without a TTY or a live backend.
//!
//! The module is split by responsibility: [`types`] holds the data model
//! (the [`Command`] enum and the parsed-flag structs), [`parse`] the argument
//! parsers and [`help_text`], and [`plan`] the session listing. All public
//! items are re-exported here so callers use `medulla_tui::cli::*`.

mod parse;
mod plan;
mod types;

#[cfg(test)]
mod tests;

pub use parse::{
    help_text, parse_command, parse_init_args, parse_login_args, parse_memory_args, parse_tui_args,
    parse_update_args,
};
pub use plan::sessions_json;
pub use types::{Command, InitArgs, LoginArgs, MemoryAction, MemoryArgs, TuiArgs, UpdateArgs};
