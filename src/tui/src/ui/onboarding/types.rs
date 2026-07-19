//! The onboarding screen's data model: the command/event/outcome enums the
//! pre-run loop exchanges with the screen, the internal step marker, and the
//! [`OnboardingScreen`] struct with its constructor and trivial accessors.
//!
//! Fields, the [`Step`] marker, and the [`OnboardingScreen::spinner`] helper are
//! `pub(super)` so the sibling logic ([`super::state`]) and rendering
//! ([`super::draw`]) modules can drive and read them; nothing internal is exposed
//! outside the `onboarding` module tree.

use crate::ui::util::SPINNER;

/// The terminal outcome of the onboarding screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingOutcome {
    /// Registration confirmed: persist the profile with this name and owner.
    Register {
        /// The chosen worker name.
        name: String,
        /// The resolved OpenHuman owner (`@handle` or address), if any.
        owner: Option<String>,
    },
    /// Abort without writing anything (q / Ctrl-C).
    Abort,
}

/// An async action the pre-run loop must run on the screen's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingCmd {
    /// Create/load the tiny.place identity for the chosen worker name.
    LoadIdentity {
        /// The worker name to mint or load the identity for.
        name: String,
    },
}

/// An event fed back from spawned async work into [`OnboardingScreen::apply`].
#[derive(Debug, Clone)]
pub enum OnboardingEvent {
    /// The identity is ready: its wallet `address` and optional `@handle`.
    IdentityReady {
        /// The identity's wallet address.
        address: String,
        /// The identity's `@handle`, when one was claimed.
        handle: Option<String>,
    },
    /// Identity bootstrap failed.
    IdentityFailed(String),
}

/// Which step the flow is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// Naming the worker.
    Name,
    /// Waiting for the identity to load (spinner).
    Connecting,
    /// Identity ready; entering/confirming the owner.
    Owner,
    /// The summary panel.
    Confirm,
}

/// The pure onboarding-screen state machine.
pub struct OnboardingScreen {
    pub(super) endpoint: String,
    pub(super) step: Step,
    pub(super) name: String,
    pub(super) owner: String,
    pub(super) address: Option<String>,
    pub(super) handle: Option<String>,
    pub(super) error: Option<String>,
    pub(super) flash: Option<String>,
    pub(super) frame: usize,
    pub(super) outcome: Option<OnboardingOutcome>,
}

impl OnboardingScreen {
    /// A fresh screen prefilled with `default_name`, an optional `env_owner` (from
    /// the owner env chain), and the resolved `endpoint` for the summary.
    pub fn new(
        default_name: impl Into<String>,
        env_owner: Option<String>,
        endpoint: impl Into<String>,
    ) -> Self {
        OnboardingScreen {
            endpoint: endpoint.into(),
            step: Step::Name,
            name: default_name.into(),
            owner: env_owner.unwrap_or_default(),
            address: None,
            handle: None,
            error: None,
            flash: None,
            frame: 0,
            outcome: None,
        }
    }

    /// The terminal outcome, once reached.
    pub fn outcome(&self) -> Option<OnboardingOutcome> {
        self.outcome.clone()
    }

    /// The current worker name draft.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Advance the spinner (called on the pre-run loop tick).
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// The current spinner glyph for the connecting step.
    pub(super) fn spinner(&self) -> &'static str {
        SPINNER[self.frame % SPINNER.len()]
    }
}
