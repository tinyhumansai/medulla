//! The onboarding module's data types: the [`Registration`] result, the
//! [`OnboardingContext`] handed to an interactive UI, and the [`OnboardingUi`]
//! callback the app injects to render the interactive screen.
//!
//! Keeping these here lets [`super`] own only the async wiring while the app
//! crate (which supplies the actual rendering) depends on stable public types.

use std::sync::Arc;

use crate::worker_profile::WorkerProfile;
use ::tinyplace::LocalSigner;

/// The result of an onboarding check.
pub struct Registration {
    /// The active worker profile (loaded or freshly written).
    pub profile: WorkerProfile,
    /// True when this call ran onboarding and wrote a new profile.
    pub newly_registered: bool,
}

/// Everything an interactive onboarding UI needs to render the worker-setup flow
/// and run its identity step. The heavy lifting (minting the identity, writing
/// the profile, the announce DM) stays in [`super::ensure_registered`]; the UI
/// only chooses a `(name, owner)` and may call [`super::reverse_handle_lookup`].
pub struct OnboardingContext {
    /// The default worker name to prefill the name input with.
    pub default_name: String,
    /// The OpenHuman owner resolved from env/config, prefilled into the owner input.
    pub prefill_owner: Option<String>,
    /// The resolved tiny.place endpoint, shown in the confirm summary and used
    /// for the reverse `@handle` lookup.
    pub endpoint: String,
    /// This worker's wallet address (already minted before the UI runs).
    pub address: String,
    /// The identity signer, used to authenticate the reverse-lookup client.
    pub signer: Arc<LocalSigner>,
}

/// A boxed async callback that renders the interactive onboarding screen and
/// returns the operator's choice: `Some((name, owner))` to register, or `None`
/// when the operator aborts (q / Ctrl-C).
///
/// The app crate builds one of these from its ratatui screen and hands it to
/// [`super::ensure_registered`]; the SDK stays free of any terminal dependency.
pub type OnboardingUi = Box<
    dyn FnOnce(
            OnboardingContext,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<Option<(String, Option<String>)>>>
                    + Send,
            >,
        > + Send,
>;
