//! The first-run worker onboarding screen: a pure state machine the onboarding
//! pre-run loop drives before a worker (daemon/wrapper) starts serving.
//!
//! It mirrors [`crate::ui::login`]: [`OnboardingScreen::handle_key`] turns keys
//! into [`OnboardingCmd`]s, [`OnboardingScreen::apply`] folds
//! [`OnboardingEvent`]s from async work back into state, and
//! [`OnboardingScreen::draw`] renders the centered panel. The loop reads
//! [`OnboardingScreen::outcome`] to know when to stop.
//!
//! Three steps: NAME (an input prefilled with the default worker name) →
//! CONNECTION (create/load the identity, then prompt for the OpenHuman owner) →
//! CONFIRM (a summary panel). The heavy work — minting the identity, sending the
//! announce DM, writing the profile — lives in [`crate::onboarding`]; this module
//! only owns state and rendering.
//!
//! Split by responsibility: [`types`] holds the command/event/outcome enums, the
//! [`OnboardingScreen`] struct, and its trivial accessors; [`state`] holds the
//! key → command and event → state machine ([`OnboardingScreen::handle_key`] and
//! [`OnboardingScreen::apply`]); and [`draw`] holds the ratatui rendering. All
//! public items are re-exported here so callers use `medulla::ui::onboarding::*`.

mod draw;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use types::{OnboardingCmd, OnboardingEvent, OnboardingOutcome, OnboardingScreen};
