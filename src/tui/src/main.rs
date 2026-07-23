//! Binary entry point for the `medulla` TUI: load `.env`, parse the top-level
//! command, and dispatch. The heavy lifting lives in sibling modules:
//! [`terminal`] owns the crossterm terminal lifecycle, [`commands`] the non-TUI
//! subcommand runners and the pre-app login screen, [`app_loop`] TUI startup and
//! runtime selection, and [`event_loop`] the interactive event loop.

use std::io::{self, IsTerminal};

use medulla_tui::cli::{parse_command, sessions_json, Command};

use crate::app_loop::run_tui;
use crate::commands::{run_hub, run_init, run_lessons, run_login, run_logout, run_memory};
use crate::run::run_core;

mod app_loop;
mod commands;
mod event_loop;
mod hub_relay;
mod run;
mod terminal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load a cwd `.env` into the process env before anything reads it (this is
    // how local dev opts into `MEDULLA_DEV=1`). Never overrides existing vars.
    medulla::home::load_dotenv_from_cwd();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    match parse_command(&raw) {
        Command::Run => run_core(&raw[1..]).await,
        Command::Daemon => medulla::daemon::run_daemon(&raw[1..], onboarding_ui()).await,
        Command::Version => {
            println!("medulla {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            print!("{}", medulla_tui::cli::help_text());
            Ok(())
        }
        Command::Sessions => {
            let env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".to_string());
            match sessions_json(&env, &cwd) {
                Ok(json) => println!("{json}"),
                Err(err) => eprintln!("failed to serialize sessions: {err}"),
            }
            Ok(())
        }
        Command::Login => run_login(&raw[1..]).await,
        Command::Logout => run_logout(),
        Command::Memory => run_memory(&raw[1..]).await,
        Command::Init => run_init(&raw[1..]).await,
        Command::Lessons => run_lessons(&raw[1..]),
        Command::Hub => run_hub(&raw[1..]).await,
        Command::Update => {
            let args = medulla_tui::cli::parse_update_args(&raw[1..]);
            medulla::update::run_update(args.check).await
        }
        Command::Wrapper(provider) => {
            let code = medulla::wrapper::run_wrapper(
                provider,
                &raw[1..],
                onboarding_ui(),
                Some(medulla_tui::harness_pty::spawner()),
            )
            .await?;
            std::process::exit(code);
        }
        // Bare invocation, or the TUI's own --config/--no-alt-screen flags.
        Command::Tui => run_tui(&raw).await,
    }
}

/// Build the interactive onboarding callback when stdout is a TTY, else `None`
/// so the daemon/wrapper first-run flow auto-registers headlessly. This is the
/// app-side seam that keeps the SDK free of any terminal dependency.
fn onboarding_ui() -> Option<medulla::onboarding::OnboardingUi> {
    if io::stdout().is_terminal() {
        Some(Box::new(|ctx| {
            Box::pin(medulla_tui::ui::onboarding::run_onboarding_ui(ctx))
        }))
    } else {
        None
    }
}
