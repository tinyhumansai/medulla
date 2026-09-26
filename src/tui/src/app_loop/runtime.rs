//! Building the runtime the UI drives, and reading who this process is signed in
//! as.
//!
//! Split out of [`super`] because it is one responsibility with three pieces —
//! the workspace roots that ride a session mint, the runtime construction both
//! the signed-in and just-logged-in paths need, and the session/account read
//! every surface takes its answer from — and because keeping it there put the
//! file over the repository's 500-line ceiling.

use std::sync::Arc;

use medulla::runtime::Runtime;

/// The configured workspace roots, as paths.
///
/// Their `MEDULLA.md` profiles ride every session mint, so the backend can brief
/// a delegated turn on a repository it has never seen.
pub(super) fn workspace_roots(config: &medulla::config::TuiConfig) -> Vec<std::path::PathBuf> {
    config
        .workflow
        .workspaces
        .iter()
        .map(std::path::PathBuf::from)
        .collect()
}

/// Both the already-signed-in path and the just-logged-in path need the same
/// two priming steps, and getting either wrong is invisible until the UI sits
/// empty or inert.
pub(super) async fn cloud_runtime(
    client: Arc<medulla::client::MedullaClient>,
    hub: crate::hub_relay::HubSlot,
    backend: &medulla::config::BackendConfig,
    workspaces: Vec<std::path::PathBuf>,
) -> Arc<dyn Runtime> {
    // The backend URL rides along for the feedback board, which sits on a
    // different route family from the orchestration API and mints its own
    // short-lived client per call.
    let rt = medulla::runtime::cloud::CloudRuntime::with_hub(client, hub)
        .with_backend(backend.clone())
        .with_workspaces(workspaces);
    // First fetch before the UI paints, so the initial frame shows real state
    // rather than an empty one that fills in a beat later.
    rt.refresh().await;
    let rt = Arc::new(rt);
    // Start replaying events before the UI paints. Without this a submitted turn
    // is accepted and nothing ever returns to the transcript, which reads as a
    // hang rather than a missing loop.
    rt.spawn_poll_loop();
    rt
}

/// Read the core's session once: the bearer for backend-facing services, and
/// who it belongs to for the Account subpage.
///
/// `base_url` comes from the loaded Medulla config rather than the core — they
/// address the same deployment by construction, and the core exposes no RPC for
/// the URL it resolved. A failed read degrades to signed out: the surfaces that
/// take this simply go without a backend, which is exactly what they do for a
/// signed-out host.
pub(super) fn session_of(
    env: &std::collections::HashMap<String, String>,
    backend: &medulla::config::BackendConfig,
) -> (
    Option<medulla::auth::Credentials>,
    Option<medulla::auth::AuthState>,
) {
    // Resolved through the same chain the runtime's own client uses — an inline
    // `backend.token`, then `backend.tokenEnv`, then the stored session — not
    // read from the store directly. Reading the store would leave every
    // `MEDULLA_TOKEN` operator with a working runtime and a `None` here, so the
    // hub uplink would go out unauthenticated while the UI showed a live host.
    //
    // An absent token is indistinguishable from signed out for every consumer,
    // and this runs before the terminal guard is up — reporting here would land
    // on the screen the login flow is about to take over.
    let jwt = medulla::auth::resolve_backend_token(
        env,
        backend,
        medulla::auth::session_token(env).as_deref(),
    );
    // The account, by contrast, genuinely comes from the store: a bearer from
    // the environment carries no identity, so an operator running on
    // `MEDULLA_TOKEN` has a session and no account to name — which is what the
    // Account subpage should show.
    let account = Some(medulla::auth::state(env));
    let session = jwt.map(|jwt| medulla::auth::Credentials {
        base_url: backend.base_url.clone(),
        jwt,
    });
    (session, account)
}
