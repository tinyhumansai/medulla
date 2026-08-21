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
//!
//! # Settings are values, not environment variables
//!
//! This module used to configure the core by calling `std::env::set_var` four
//! times before [`boot`], because that was the only channel the core offered.
//! It has drawbacks that are easy to state and hard to notice in review: the
//! calls have to happen before a constructor they do not appear in, they are
//! process-global so a second consumer in the same process silently inherits
//! them, they leak into every child process this host spawns, and newer Rust
//! editions make mutating the environment increasingly hostile.
//!
//! [`CoreSettings`] replaces them. The *precedence* is unchanged and still
//! belongs here — an operator who exported `OPENHUMAN_WORKSPACE` still wins —
//! but it is now resolved into a value and handed to the builder, so the
//! ordering requirement is a parameter rather than a convention.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openhuman_core::embed::{Core, CoreError};
use openhuman_core::{
    CoreBuilder, DomainSet, Harness, HarnessError, HostKind, ServiceSet, TokenSource, Workspace,
};

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

/// Who the core is signed in as.
///
/// Re-exported rather than mirrored: this host used to keep its own narrow
/// projection of the core's auth response, decoded by hand out of the RPC
/// envelope. The core models it now, so a second definition would be one more
/// thing to keep in step for no gain.
pub use openhuman_core::embed::AuthState;

/// A session to install into the core's credential store.
///
/// Re-exported for the same reason as [`AuthState`]: hosts in this workspace
/// (the TUI, the CLI verbs) name it without taking a direct dependency on the
/// `openhuman` crate.
pub use openhuman_core::embed::Session;

/// The embedded harness this host boots.
pub use openhuman_core::Harness;

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

/// Everything this host decides about the core before it is built.
///
/// Resolved once by [`CoreSettings::resolve`], then handed to [`boot`]. Held as
/// a value rather than written to the process environment — see the module docs
/// on why that distinction is worth a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSettings {
    /// The core's state directory.
    pub workspace: PathBuf,
    /// The agent's read/write root, when this host has one to name.
    pub action_dir: Option<PathBuf>,
    /// The backend deployment the core dials. Empty when nothing named one, in
    /// which case the core resolves its own.
    pub backend_url: String,
}

impl CoreSettings {
    /// The floor every core gets: workspace isolation and nothing else.
    ///
    /// A host that reaches the core without a loaded config — a lazily booted
    /// workflow run, an MCP subprocess — still must not write into the
    /// developer's real `~/.openhuman`. This is the minimum that prevents it.
    pub fn floor(env: &HashMap<String, String>, medulla_home: &Path) -> Self {
        Self {
            workspace: resolve_workspace(env, medulla_home),
            action_dir: None,
            backend_url: String::new(),
        }
    }

    /// Everything a loaded config names.
    ///
    /// The first configured workspace root is the agent's read/write root, the
    /// same precedence `app_loop.rs` gives it when the TUI binds its primary
    /// host. A blank or absent root leaves it unset rather than binding
    /// something arbitrary.
    pub fn resolve(
        env: &HashMap<String, String>,
        config: &crate::config::TuiConfig,
        medulla_home: &Path,
    ) -> Self {
        let root = config
            .workflow
            .workspaces
            .first()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty());
        Self {
            workspace: resolve_workspace(env, medulla_home),
            action_dir: resolve_action_dir(env, root.as_deref()),
            backend_url: resolve_backend_api_url(env, &config.backend.base_url),
        }
    }
}

/// The workspace this process's core should use.
///
/// **Non-overriding**: an operator who exported `OPENHUMAN_WORKSPACE` keeps it,
/// which is what lets a developer aim the embedded core at an existing
/// OpenHuman install on purpose. Otherwise it derives from `MEDULLA_HOME`, so
/// the scratch-run recipe in the module docs isolates the core too.
pub fn resolve_workspace(env: &HashMap<String, String>, medulla_home: &Path) -> PathBuf {
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
    tracing::debug!(
        "[core_host] workspace derived from MEDULLA_HOME: {}",
        dir.display()
    );
    dir
}

/// The agent's read/write root.
///
/// OpenHuman defaults `action_dir` to `~/OpenHuman/projects`, which is not
/// where a Medulla operator works — their repos are the workspace roots already
/// in Medulla's config. Leaving the default would aim the agent's write root at
/// a directory this host has never used.
///
/// Non-overriding for the same reason as [`resolve_workspace`]. A `None` or
/// empty `root` yields `None` rather than something arbitrary.
pub fn resolve_action_dir(
    env: &HashMap<String, String>,
    root: Option<&Path>,
) -> Option<PathBuf> {
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
    tracing::debug!("[core_host] action_dir resolved to {}", root.display());
    Some(root.to_path_buf())
}

/// The backend deployment the core should dial.
///
/// Covers both of the core's backend clients at once. `openhuman.auth_store_session`
/// validates a token against `/auth/me` on the base OpenHuman's own config chain
/// resolves — which otherwise falls back to *production* and has never heard of
/// `MEDULLA_STAGING`. So on staging the login flow verified a staging JWT,
/// handed it to the core, and the core asked production whether it was valid; it
/// is not, and the failure surfaced as "backend rejected session token", which
/// names the symptom and not the two endpoints that disagreed. The same mismatch
/// hits any self-hosted install.
///
/// The core's Medulla client resolves through the same value: with no
/// `OPENHUMAN_MEDULLA_BASE_URL` override it falls through to
/// `effective_backend_api_url`, which reads exactly the `api_url` this sets.
/// That is why one setting now replaces the two bindings this used to need.
///
/// Non-overriding, and either environment spelling counts as the operator
/// having aimed the core somewhere on purpose.
pub fn resolve_backend_api_url(env: &HashMap<String, String>, base_url: &str) -> String {
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
    tracing::debug!("[core_host] backend api url resolved from the Medulla config");
    base_url.to_string()
}

/// The settings a later, caller-less boot should use.
///
/// The lazy boot in [`shared`] has no caller to hand settings to — a workflow
/// `agent` node runs deep inside the engine, and the dispatch signature belongs
/// to every harness, not just this one. So whoever *does* have the loaded config
/// leaves it here first.
///
/// This is process-global, like the environment variables it replaces, and that
/// is not an accident of the port — it is the same problem. What changes is the
/// blast radius: a typed cell this crate owns affects this process's core and
/// nothing else, where `OPENHUMAN_WORKSPACE` and friends affected every library
/// in the process and every child it spawns.
static SETTINGS: std::sync::OnceLock<CoreSettings> = std::sync::OnceLock::new();

/// Publish the settings a later lazy boot should use.
///
/// Returns whether this call is the one that installed them. `false` means
/// settings were already present and *these* are not them — reported rather
/// than swapped, because a swap after a core has booted would describe a core
/// that no longer matches.
pub fn install_settings(settings: CoreSettings) -> bool {
    let installed = SETTINGS.set(settings).is_ok();
    if !installed {
        tracing::debug!("[core_host] core settings already installed; keeping the first");
    }
    installed
}

/// The installed settings, or the floor derived from this process's environment.
///
/// The floor is workspace isolation only. It is deliberately not "nothing":
/// a lazily booted core with no settings at all would write into the
/// developer's real `~/.openhuman`.
pub fn settings_or_floor() -> CoreSettings {
    if let Some(settings) = SETTINGS.get() {
        return settings.clone();
    }
    let env: HashMap<String, String> = std::env::vars().collect();
    let home = crate::home::medulla_home(&env);
    tracing::debug!("[core_host] no settings installed; using the workspace-isolation floor");
    CoreSettings::floor(&env, &home)
}

/// Build the embedded harness.
///
/// Unlike the old `boot`, this takes its configuration as an argument. There is
/// nothing to bind first and nothing to get wrong about ordering: what the core
/// runs on is what was passed in.
///
/// # Errors
///
/// Propagates any failure from [`Harness::builder`] — a workspace that cannot be
/// created, or a second harness in a process that already has one.
pub async fn boot(settings: CoreSettings) -> Result<Harness, HarnessError> {
    boot_with_hooks(settings, &crate::harness_hooks::HooksConfig::default()).await
}

/// Boot the embedded harness with Medulla's supported in-process lifecycle hooks.
///
/// Registers Medulla's configured `Stop`, `PreToolUse`, and `PostToolUse` hooks
/// as OpenHuman embedder lifecycle hooks **before** constructing the core — see
/// [`hooks`]. Registration is process-global, but each boot replaces Medulla's
/// previous hook registrations, including removing a kind no longer configured.
///
/// # Errors
///
/// Same cases as [`boot`]. A hook command that fails to spawn surfaces later, at
/// runtime, not here.
pub async fn boot_with_hooks(
    settings: CoreSettings,
    hooks: &crate::harness_hooks::HooksConfig,
) -> Result<Harness, HarnessError> {
    hooks::register_lifecycle_hooks(hooks);
    tracing::debug!(
        "[core_host] boot start workspace={} action_dir={:?} backend={}",
        settings.workspace.display(),
        settings.action_dir,
        if settings.backend_url.is_empty() {
            "<core default>"
        } else {
            "<configured>"
        }
    );

    let mut builder = Harness::builder()
        .host_kind(HostKind::detect_standalone())
        // `Workspace::Dir`, not `Ephemeral`: this state is the operator's
        // account and outlives the process. The path is Medulla's own
        // derivation, so a scratch `MEDULLA_HOME` still gets an isolated core.
        .workspace(Workspace::Dir(settings.workspace))
        // The presets this host has always used. `embedded()` is the long-lived
        // shape — cron, heartbeat, the memory queue — which is why exactly one
        // of these may exist per process; see [`shared`].
        .domains(DomainSet::embedded())
        .services(ServiceSet::embedded());

    if let Some(action_dir) = settings.action_dir {
        builder = builder.action_dir(action_dir);
    }
    if !settings.backend_url.is_empty() {
        builder = builder.backend_url(settings.backend_url);
    }

    let harness = builder.build().await?;
    tracing::debug!("[core_host] boot ok");
    Ok(harness)
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
/// and every service is off.
///
/// Deliberately **not** a [`Harness`]: this is a short-lived core for one
/// question, and claiming the process's single harness slot for it would refuse
/// a later real boot in the same process. It uses [`CoreBuilder`] with the same
/// typed settings instead.
///
/// # Errors
///
/// Propagates any failure from [`CoreBuilder::build`].
pub async fn boot_for_auth(settings: CoreSettings) -> anyhow::Result<Core> {
    let mut domains = DomainSet::none();
    domains.platform = true;
    domains.security = true;
    tracing::debug!("[core_host] boot_for_auth start");

    let mut builder = CoreBuilder::new(HostKind::detect_standalone())
        .domains(domains)
        .services(ServiceSet::none())
        .token(TokenSource::EnvOrFile)
        .workspace(settings.workspace);

    if let Some(action_dir) = settings.action_dir {
        builder = builder.action_dir(action_dir);
    }
    if !settings.backend_url.is_empty() {
        // Auth is the whole point of this core: `/auth/me` has to be resolved
        // against the deployment the operator's token came from, or a valid
        // staging token is handed to production and rejected.
        builder = builder.backend_url(settings.backend_url);
    }

    let runtime = builder.build().await?;
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
