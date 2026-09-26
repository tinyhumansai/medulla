//! medulla-tui: the ratatui terminal app over the `medulla` SDK. [`ui`] owns
//! state, rendering, input, and chat persistence; [`cli`] owns argument
//! parsing and runtime-selection planning; [`harness_pty`] runs a wrapped
//! coding-agent CLI on a pseudo-terminal; [`remote`] serves sessions to a
//! client on another machine; process wiring lives in `main.rs`.

pub mod cli;
pub mod harness_pty;
pub mod log;
pub mod remote;
pub mod ui;
pub mod worker;
