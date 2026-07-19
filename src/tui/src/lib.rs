//! medulla-tui: the ratatui terminal app over the `medulla` SDK. [`ui`] owns
//! state, rendering, input, and chat persistence; [`cli`] owns argument
//! parsing and runtime-selection planning; process wiring lives in `main.rs`.

pub mod cli;
pub mod ui;
