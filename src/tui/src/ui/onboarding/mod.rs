//! The first-run worker onboarding screen and its interactive driver.
//!
//! The pure state machine ([`OnboardingScreen`]) mirrors [`crate::ui::login`]:
//! [`OnboardingScreen::handle_key`] turns keys into [`OnboardingCmd`]s,
//! [`OnboardingScreen::apply`] folds [`OnboardingEvent`]s from async work back
//! into state, and [`OnboardingScreen::draw`] renders the centered panel.
//!
//! Three steps: NAME (an input prefilled with the default worker name) →
//! CONNECTION (load the identity, then prompt for the OpenHuman owner) → CONFIRM
//! (a summary panel). The heavy work — minting the identity, sending the announce
//! DM, writing the profile — lives in [`medulla::onboarding`]; this crate owns
//! only the terminal rendering and the driver.
//!
//! Split by responsibility: [`types`] holds the command/event/outcome enums and
//! the [`OnboardingScreen`] struct; [`state`] holds the key → command and
//! event → state machine; [`draw`] holds the ratatui rendering; and [`run`] holds
//! the terminal-driving loop exposed as [`run_onboarding_ui`], which the app
//! wraps into a [`medulla::onboarding::OnboardingUi`] callback.

mod draw;
mod run;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use run::run_onboarding_ui;
pub use types::{OnboardingCmd, OnboardingEvent, OnboardingOutcome, OnboardingScreen};
