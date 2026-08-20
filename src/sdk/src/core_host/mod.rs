//! Boot the embedded OpenHuman core in this process.
//!
//! One place builds the [`EmbeddedCore`], so there is exactly one answer to
//! "which workspace does the core write to" and one place to change the domain
//! and service composition.
//!
//! # Workspace isolation is the load-bearing part
//!
//! OpenHuman resolves its own state directory from `OPENHUMAN_WORKSPACE`,
//! defaulting to `~/.openhuman/...`. Medulla resolves its state from
//! `MEDULLA_HOME`. Left alone those are independent, which quietly breaks the
//! scratch-run recipe this repo documents:
//!
//! ```text
//! MEDULLA_HOME=$(mktemp -d) ./target/debug/medulla
//! ```
//!
//! That recipe exists so a test run reads its own workflow store, agent
//! templates, and state rather than the developer's. Without deriving
//! `OPENHUMAN_WORKSPACE` from `MEDULLA_HOME`, every such run would still write
//! memory, flows, and credentials into the developer's real `~/.openhuman` —
//! silently, because nothing fails. [`workspace_dir`] is that derivation, and
//! it is why this module exists rather than callers building the runtime
//! themselves.
//!
//! # Composition
//!
//! [`DomainSet::embedded`] and [`ServiceSet::embedded`] describe a long-lived
//! host that drives the core in-process through the typed facade and owns its
//! own presentation layer — no HTTP listener, no Socket.IO, but the background
//! work a long session expects. Both are OpenHuman presets named for that
//! shape; see their docs for why this host is not `harness()`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openhuman_core::embed::{Core, CoreError};
use openhuman_core::{CoreBuilder, DomainSet, HostKind, ServiceSet, TokenSource};

pub mod auth;
mod hooks;
pub mod shared;
pub mod turn_cwd;

#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod hooks_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod turn_cwd_tests;

/// The embed facade, re-exported so a host can name what [`boot`] returns
/// without depending on the `openhuman` crate directly.
pub use openhuman_core::embed::Core as EmbeddedCore;

/// Environment variable OpenHuman reads for its state directory.
pub const OPENHUMAN_WORKSPACE_ENV: &str = "OPENHUMAN_WORKSPACE";

/// Environment variable OpenHuman reads for the agent's read/write root.
pub const OPENHUMAN_ACTION_DIR_ENV: &str = "OPENHUMAN_ACTION_DIR";

/// Environment variable OpenHuman reads for the Medulla backend it dials.
pub const OPENHUMAN_MEDULLA_BASE_URL_ENV: &str = "OPENHUMAN_MEDULLA_BASE_URL";

/// Environment variable OpenHuman reads for its **own** backend API base — the
/// one `/auth/me` is resolved against.
pub const OPENHUMAN_BACKEND_URL_ENV: &str = "BACKEND_URL";

/// The alternate spelling OpenHuman's config chain also honours, checked so an
/// operator who exported that one is not overridden.
pub const OPENHUMAN_BACKEND_URL_ALT_ENV: &str = "VITE_BACKEND_URL";

/// The core's state directory for a given Medulla home.
///
/// Nested under the Medulla home rather than beside it so that removing a
/// scratch `MEDULLA_HOME` removes the core's state with it — a half-deleted
/// scratch run that leaves an OpenHuman workspace behind is worse than none,
/// because the next run silently inherits it.
///
/// Directly under the home, not under an `openhuman/` level of its own: the
/// home is already scoped to one account (see [`crate::home`]), so the core's
/// state is that account's state and has nothing to be separated from. The name
/// `workspace` is load-bearing — OpenHuman derives its *config* directory from
/// the workspace path, and a directory called `workspace` puts that config at
/// `<medulla_home>/.openhuman` beside it rather than inside the state tree.
pub fn workspace_dir(medulla_home: &Path) -> PathBuf {
    medulla_home.join("workspace")
}

/// Point OpenHuman at a workspace derived from this process's Medulla home.
///
/// Idempotent and **non-overriding**: an operator who sets
/// `OPENHUMAN_WORKSPACE` explicitly keeps it, which is what lets a developer
/// aim the embedded core at an existing OpenHuman install on purpose. Returns
/// the directory in effect either way.
///
/// Call before [`boot`]; the core reads the variable during construction.
pub fn bind_workspace(env: &HashMap<String, String>, medulla_home: &Path) -> PathBuf {
    if let Some(explicit) = env
        .get(OPENHUMAN_WORKSPACE_ENV)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        let dir = PathBuf::from(explicit);
        tracing::debug!(
            "[core_host] workspace from {OPENHUMAN_WORKSPACE_ENV} (operator override): {}",
            dir.display()
        );
        return dir;
    }

    let dir = workspace_dir(medulla_home);
    std::env::set_var(OPENHUMAN_WORKSPACE_ENV, &dir);
    tracing::debug!(
        "[core_host] workspace derived from MEDULLA_HOME: {}",
        dir.display()
    );
    dir
}

/// Point the agent's read/write root at the operator's own workspace roots.
///
/// OpenHuman defaults `action_dir` to `~/OpenHuman/projects`, which is not
/// where a Medulla operator works — their repos are the workspace roots already
/// in Medulla's config. Leaving the default would aim the agent's write root at
/// a directory this host has never used.
///
/// Non-overriding for the same reason as [`bind_workspace`]. A `None` or empty
/// `root` leaves the variable alone rather than binding something arbitrary.
pub fn bind_action_dir(env: &HashMap<String, String>, root: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = env
        .get(OPENHUMAN_ACTION_DIR_ENV)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return Some(PathBuf::from(explicit));
    }
    let root = root?;
    if root.as_os_str().is_empty() {
        return None;
    }
    std::env::set_var(OPENHUMAN_ACTION_DIR_ENV, root);
    tracing::debug!("[core_host] action_dir bound to {}", root.display());
    Some(root.to_path_buf())
}

/// Point the core's Medulla client at the backend this host is configured for.
///
/// The core resolves its own backend URL from OpenHuman's config chain, which
/// on a default install lands on the same hosted deployment Medulla's
/// `backend.baseUrl` names. On a staging or self-hosted install it does not: the
/// login screen would verify a token against the configured endpoint while the
/// core probed and stored it against a different one — the readiness probe fails
/// or the backend rejects a freshly verified token, and neither failure names
/// the mismatch that caused it.
///
/// Non-overriding, like [`bind_workspace`]: an operator who exported
/// `OPENHUMAN_MEDULLA_BASE_URL` themselves is aiming the core somewhere on
/// purpose. Returns the URL in effect either way.
///
/// Call before [`boot`]; the core reads the variable when it resolves a client.
pub fn bind_medulla_base_url(env: &HashMap<String, String>, base_url: &str) -> String {
    if let Some(explicit) = env
        .get(OPENHUMAN_MEDULLA_BASE_URL_ENV)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        tracing::debug!("[core_host] medulla base url from the operator's override");
        return explicit.to_string();
    }
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return String::new();
    }
    std::env::set_var(OPENHUMAN_MEDULLA_BASE_URL_ENV, base_url);
    tracing::debug!("[core_host] medulla base url bound from the Medulla config");
    base_url.to_string()
}

/// Point the core's *own* backend client at the same deployment.
///
/// [`bind_medulla_base_url`] covers the core's Medulla client only. Auth is a
/// different client: `openhuman.auth_store_session` validates a token against
/// `/auth/me` on the base OpenHuman's own config chain resolves, which consults
/// `BACKEND_URL` / `VITE_BACKEND_URL` and otherwise falls back to *production*
/// — it has never heard of `MEDULLA_STAGING`.
///
/// So on staging the login flow verified a staging JWT, handed it to the core,
/// and the core asked production whether it was valid. It is not, and the
/// failure surfaces as `auth_store_session: Session validation failed (GET
/// /auth/me): backend rejected session token` — which names the symptom and not
/// the two endpoints that disagreed. The same mismatch hits any self-hosted
/// install, staging just makes it certain.
///
/// Non-overriding, like the bindings above, and either spelling counts as the
/// operator having aimed the core somewhere on purpose. Returns the URL in
/// effect either way.
///
/// Call before [`boot`]; the core reads the variable when it resolves a client.
pub fn bind_backend_api_url(env: &HashMap<String, String>, base_url: &str) -> String {
    for key in [OPENHUMAN_BACKEND_URL_ENV, OPENHUMAN_BACKEND_URL_ALT_ENV] {
        if let Some(explicit) = env.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            tracing::debug!("[core_host] backend api url from the operator's {key} override");
            return explicit.to_string();
        }
    }
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return String::new();
    }
    std::env::set_var(OPENHUMAN_BACKEND_URL_ENV, base_url);
    tracing::debug!("[core_host] backend api url bound from the Medulla config");
    base_url.to_string()
}

/// Point the embedded core at everything a loaded config names, before it can
/// boot.
///
/// [`boot`] reads the core's environment during construction, and the lazy boot
/// path ([`shared`]) has no caller of its own to bind for it. A host that has
/// its layered config in hand — `medulla workflow run`, an MCP server — calls
/// this once to reproduce the startup bindings the TUI and `medulla run`
/// perform: the workspace derived from `MEDULLA_HOME`, the agent's action
/// directory from the configured workspace roots, and both backend URLs from
/// `backend.base_url`. A workflow `agent` node or MCP turn that reaches the
/// core first then reads this account's state and the configured deployment
/// rather than ambient `~/.openhuman`.
///
/// Non-overriding, like every individual binding: an operator who exported any
/// of the variables wins.
pub fn bind_from_config(
    env: &HashMap<String, String>,
    config: &crate::config::TuiConfig,
    home: &Path,
) {
    // The first configured workspace root is the agent's read/write root, the
    // same precedence `app_loop.rs` gives it when the TUI binds its primary
    // host. A blank or absent root leaves the variable alone.
    let root = config
        .workflow
        .workspaces
        .first()
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    bind_workspace(env, home);
    bind_action_dir(env, root.as_deref());
    bind_medulla_base_url(env, &config.backend.base_url);
    bind_backend_api_url(env, &config.backend.base_url);
}

/// Build the embedded core and wrap it in the typed facade.
///
/// Callers must have bound the workspace first — see [`bind_workspace`]. This
/// does not do it implicitly, because the binding reads process environment and
/// hiding that inside a constructor makes the ordering requirement invisible.
///
/// # Errors
///
/// Propagates any failure from [`CoreBuilder::build`] — a workspace that cannot
/// be created, or a token source that cannot be resolved.
pub async fn boot() -> anyhow::Result<Core> {
    boot_with_hooks(&crate::harness_hooks::HooksConfig::default()).await
}

/// Boot the embedded core with Medulla's supported in-process lifecycle hooks.
///
/// Registers Medulla's configured `Stop`, `PreToolUse`, and `PostToolUse` hooks
/// as OpenHuman embedder lifecycle hooks **before** constructing the core — see
/// [`hooks`]. Registration is process-global, but each boot replaces Medulla's
/// previous hook registrations, including removing a kind no longer configured.
///
/// # Errors
///
/// Propagates any failure from [`CoreBuilder::build`], the same cases as
/// [`boot`]: a workspace that cannot be created, or a token source that cannot
/// be resolved. A hook command that fails to spawn also surfaces here only if
/// the core startup itself fails; hook execution happens later, at runtime.
pub async fn boot_with_hooks(hooks: &crate::harness_hooks::HooksConfig) -> anyhow::Result<Core> {
    hooks::register_lifecycle_hooks(hooks);
    tracing::debug!("[core_host] boot start host_kind=detect_standalone");
    let runtime = CoreBuilder::new(HostKind::detect_standalone())
        .domains(DomainSet::embedded())
        .services(ServiceSet::embedded())
        .token(TokenSource::EnvOrFile)
        .build()
        .await?;
    tracing::debug!("[core_host] boot ok services={:?}", runtime.services());
    Ok(Core::from_runtime(Arc::new(runtime)))
}

/// Boot a core with nothing running but the surface auth needs.
///
/// The CLI verbs (`medulla login`, `logout`, `memory`, `hub`) need the core only
/// to read or write the app session, and paying for [`boot`]'s full composition
/// — cron, heartbeat, the memory queue, harness init — to answer one question
/// would make every one of them slow to start and leave background work running
/// for the length of a `--help`-sized command.
///
/// `security` is the family that owns the `auth` controllers; `platform` stays
/// enabled for the core's shared platform infrastructure. Every other domain
/// and every service is off. Callers must still have bound the workspace first
/// — see [`bind_workspace`].
///
/// # Errors
///
/// Propagates any failure from [`CoreBuilder::build`], same as [`boot`].
pub async fn boot_for_auth() -> anyhow::Result<Core> {
    let mut domains = DomainSet::none();
    domains.platform = true;
    domains.security = true;
    tracing::debug!("[core_host] boot_for_auth start");
    let runtime = CoreBuilder::new(HostKind::detect_standalone())
        .domains(domains)
        .services(ServiceSet::none())
        .token(TokenSource::EnvOrFile)
        .build()
        .await?;
    Ok(Core::from_runtime(Arc::new(runtime)))
}

/// Whether a booted core can actually reach a Medulla backend.
///
/// Three outcomes rather than two, because a host answers each differently:
/// run, sign in, or stop. Collapsing the last two would either send an operator
/// to a login screen that cannot fix a missing URL, or refuse to start over a
/// state one keystroke resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// The Medulla surface answered — the core is usable as a runtime.
    Ready,
    /// Reachable, but nobody is signed in. The host should run its login flow.
    SignedOut,
    /// The core cannot reach Medulla at all: no base URL, or the surface was
    /// compiled out. Carries the core's own operator-safe message (no URLs, no
    /// tokens) rather than wording that would drift from the core's.
    Unusable(String),
}

/// Stable `data.kind` for "nobody is signed in on this host".
const SIGNED_OUT_KIND: &str = "MedullaNoSessionToken";

/// Stable `data.kind` for "there is no Medulla backend to talk to".
const NO_BASE_URL_KIND: &str = "MedullaNoBaseUrl";

/// Classify a Medulla call's outcome.
///
/// Only the two "not set up" discriminators — and `Unavailable`, which means the
/// surface was compiled out — are treated as anything but ready. Everything else
/// ([`CoreError::Rpc`], a decode failure, a rejected call that named some other
/// `kind`) reads as *ready*: those are transient or genuine faults, and a flaky
/// network must not read as "sign in again".
pub fn classify(outcome: Result<(), CoreError>) -> Readiness {
    match outcome {
        Ok(()) => Readiness::Ready,
        Err(CoreError::Domain { kind, message, .. }) => match kind.as_deref() {
            Some(SIGNED_OUT_KIND) => Readiness::SignedOut,
            Some(NO_BASE_URL_KIND) => Readiness::Unusable(message),
            _ => {
                tracing::debug!("[core_host] readiness probe rejected, assuming ready: {message}");
                Readiness::Ready
            }
        },
        Err(CoreError::Unavailable { method }) => {
            Readiness::Unusable(format!("{method} is not available in this build"))
        }
        Err(err) => {
            tracing::debug!("[core_host] readiness probe failed, assuming ready: {err}");
            Readiness::Ready
        }
    }
}

/// Ask the core whether its Medulla surface is usable.
///
/// Uses the session list because it is the cheapest read that goes through the
/// same client resolution every drive method does — a host that can list
/// sessions can submit to one. Booting successfully is not the same as being
/// usable, and the difference is what tells a host whether to start, to ask the
/// operator to sign in, or to stop.
pub async fn probe_medulla(core: &Core) -> Readiness {
    classify(core.medulla().list_sessions().await.map(|_| ()))
}
