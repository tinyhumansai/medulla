//! Session planning and spawn: find an idle session or launch a fresh harness.
//!
//! [`super::PtySessionExecutor::session_for`] applies the lifetime-class rules
//! that decide whether to reuse an idle session or start a new one, and
//! [`super::PtySessionExecutor::spawn_env`] builds the environment and
//! arguments a fresh harness spawns with.

use std::collections::HashMap;

use medulla::daemon::providers::RunTaskOptions;
use medulla::sessions::SessionClass;

use super::super::pty::{LaunchSpec, SessionControl, SessionOrigin};
use super::types::{OpenedSession, PtySessionExecutor, SessionPlan};

impl PtySessionExecutor {
    /// Apply the lifetime-class rules (spec §4) to find or plan a session.
    ///
    /// The route is:
    ///
    /// 1. Where the task came from (`origin`) controls whether it is a
    ///    *sharable* class (the orchestrator directs it to an idle session) or
    ///    a *dedicated* class (always a fresh harness).
    /// 2. An unbound session with no prompt-for-task already typed yet and no
    ///    operator in it — "ready, idle, and untouched" — is the prime
    ///    candidate for a shareable task.
    /// 3. A session that *could* be shared but is currently held by an operator
    ///    or already mid-turn is skipped; the caller will resume planning.
    pub(super) fn session_for(
        &self,
        origin: SessionOrigin,
        options: &RunTaskOptions,
        existing: &HashMap<String, OpenedSession>,
    ) -> SessionPlan {
        let class = SessionClass::from_origin(origin, options.provider);
        // An idle session that is ready for a shareable task: unbound, not
        // held, and not already mid-turn (no prompt typed yet).
        if class == SessionClass::Unbound {
            if let Some((id, session)) = existing.iter().find(|(_, session)| {
                session.class == SessionClass::Unbound
                    && session.control != SessionControl::User
                    && !session.prompt_set
            }) {
                return SessionPlan::Reuse {
                    id: id.clone(),
                    class,
                };
            }
        }
        // Always a fresh session for per-task, or when no reusable one is
        // available.
        SessionPlan::New {
            class,
            origin,
            provider: options.provider,
            model: options.model.clone(),
            agent_identity: options.agent_identity.clone(),
            gh_repo_is_set: options.gh_repo_is_set,
            gh_owner_repo: options.gh_owner_repo.clone(),
        }
    }

    /// Open a new session for a [`SessionPlan::New`].
    ///
    /// The plan's fields are folded into a [`LaunchSpec`], the environment and
    /// extra CLI arguments are built by [`Self::spawn_env`], and the PTY is
    /// spawned through [`PtyManager::launch`](super::super::pty::PtyManager::launch).
    pub(super) async fn launch(
        &self,
        plan: SessionPlan,
        options: &RunTaskOptions,
    ) -> Result<OpenedSession, String> {
        let (provider, model, agent_identity, gh_repo_is_set, gh_owner_repo, origin, class) =
            match &plan {
                SessionPlan::New {
                    provider,
                    model,
                    agent_identity,
                    gh_repo_is_set,
                    gh_owner_repo,
                    origin,
                    class,
                } => (
                    provider,
                    model.clone(),
                    agent_identity.clone(),
                    *gh_repo_is_set,
                    gh_owner_repo.clone(),
                    *origin,
                    *class,
                ),
                _ => unreachable!("launch called without a New plan"),
            };
        let (env, extra_args) = self.spawn_env(*provider, options)?;
        let spec = LaunchSpec {
            provider: *provider,
            model,
            agent_identity,
            gh_repo_is_set,
            gh_owner_repo,
            origin,
            class,
            env,
            extra_args,
        };
        self.pty.launch(spec).await
    }

    /// Build the environment and extra CLI arguments for a fresh harness.
    ///
    /// Environment variables come from the provider's preset plus the
    /// inference-proxy and API-host routing that resolves the configured
    /// model. Extra CLI arguments are provider-specific — headless,
    /// `--print`, prompts — and are only added when the provider's
    /// [`RunTaskOptions`] sideband says so.
    fn spawn_env(
        &self,
        provider: medulla::protocol::HarnessProvider,
        options: &RunTaskOptions,
    ) -> Result<(HashMap<String, String>, Vec<String>), String> {
        use medulla::protocol::HarnessProvider;
        let mut env = medulla::protocol::env::common(options);
        // Provider preset overrides, applied before the inference-proxy
        // routing below so the provider-specific `apiKeyHelp` notes land in
        // the right env keys.
        medulla::attribution::apply(&mut env, &self.env, provider);
        // Inference-proxy and API-host routing: read the model from the task
        // options, ask the harness hook for the routing that model resolves to,
        // and write it into the env the child inherits.
        if let Some(model) = &options.model {
            if let Some(hook) = medulla::harness_hooks::for_provider(provider) {
                if let Some(route) =
                    hook.inference_proxy(provider, model, options.gh_owner_repo.as_deref())
                {
                    medulla::inference_proxy::apply(&mut env, &route);
                }
            }
        }
        let mut extra_args: Vec<String> = Vec::new();
        // Provider-specific sideband: headless/`--print`/prompts/extra flags.
        // Only the keys each provider recognises are relevant; the provider
        // is willing to describe. Read from `env`, which now holds both the
        // preset's opt-in knobs and the endpoint the routing above wrote.
        extra_args.extend(
            medulla::codex_overrides::launch_args(options.provider, options.model.as_deref(), &env)
                .map_err(|error| error.to_string())?,
        );
        Ok((env, extra_args))
    }
}
