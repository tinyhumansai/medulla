//! medulla-tui: the ratatui terminal app over the `medulla` SDK. [`ui`] owns
//! state, rendering, input, and chat persistence; [`cli`] owns argument
//! parsing and runtime-selection planning; [`harness_pty`] runs a wrapped
//! coding-agent CLI on a pseudo-terminal; process wiring lives in `main.rs`.

pub mod cli;
pub mod harness_pty;
pub mod ui;
