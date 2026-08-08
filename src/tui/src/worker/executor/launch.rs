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
    pub(super) fn session_for(
        &self,
        options: &RunTaskOptions,
        class: SessionClass,
    ) -> Result<SessionPlan, String> {
        if class == SessionClass::Unbound {
            // Reuse this peer's session only when it is *idle*. A harness serves
            // one turn at a time: a fan-out that pastes three prompts into one
            // composer gets them answered as a single conversation, and all
            // three tails settle on the same completion — three different
            // instructions, one answer, delivered three times. A busy session
            // therefore does not qualify, and the task gets a fresh one.
            if let Some(row) = self
                .sessions
                .claim_idle(&options.conversation, options.provider)
            {
                return Ok(SessionPlan::Reuse(OpenedSession {
                    id: row.id.clone(),
                    harness_session_id: row.session_id.clone(),
                    reused: true,
                    gh_repo_is_set: self.sessions.gh_repo_is_set(&row.id).unwrap_or(false),
                }));
            }
        }
        // Nothing to reuse, so this dispatch needs a session of its own — and
        // that is where the *second*, independent rule applies: under
        // `strategy: checkout` the working tree takes one writer at a time
        // (see [`checkout_writer`](Self::checkout_writer)), so a fresh harness
        // cannot simply start beside the one that is there. The work queues
        // instead — the same exclusivity the blanket refusal used to buy,
        // without ending the dispatch to get it.
        //
        // Note what this is *not*: it is not "the workspace is held". Holds are
        // on sessions, and rule 1 above has already dealt with those. This is
        // the strategy's serialization, and under `worktree` it will not apply
        // at all.
        if self.checkout_writer(&options.cwd).is_some() {
            return Ok(SessionPlan::Queue(options.cwd.clone()));
        }
        let label = if options.conversation.is_empty() {
            format!("task:{}", options.provider.as_str())
        } else {
            options.conversation.clone()
        };
        // Only a *fresh* launch applies the router and model: a reused session
        // (the `claim_idle` branch above) is a process already running with
        // whatever it was opened with, and there is no flag that reconfigures a
        // live harness mid-conversation. Router/model drift across turns of the
        // same conversation is the same trade the headless executor's own resume
        // path accepts.
        let (mut env, mut extra_args) = self.spawn_env(options)?;
        // Resolved once, from `self.env`, and then both *used* to launch and
        // *shown* to the trust decision below. Deriving it twice from two
        // different environments is what let an override live in `self.env`,
        // select the executable, and still be invisible to `attach_mcp`
        // reading the per-run child environment.
        let bin = medulla::protocol::env::provider_bin(options.provider, &self.env);
        // Medulla's own tools, on the same terms an ACP-dispatched session gets
        // them. A task frame that asked for a workflow to be run needs the verb
        // to run it with.
        let mcp_grant_session = super::super::pty::launch::attach_mcp(
            options.provider,
            &bin,
            &mut env,
            &mut extra_args,
            self.log.as_ref(),
        );
        // The managed skills that name the workflows those tools can start,
        // on the same terms the headless executor already hands them over.
        super::super::pty::launch::attach_skills(
            options.provider,
            &env,
            std::path::Path::new(&options.cwd),
            &mut extra_args,
        );
        Ok(SessionPlan::Launch(Box::new(LaunchSpec {
            provider: options.provider,
            preset: None,
            bin,
            cwd: options.cwd.clone(),
            env,
            extra_args,
            skip_permissions: options.skip_permissions,
            label,
            model: options.model.clone(),
            session_id: None,
            // Opened to serve a task frame, so the orchestrator holds it. An
            // operator can still take it over later; that is what stops the
            // next frame landing in a composer they are typing in.
            control: SessionControl::Orchestrator,
            // …and that later takeover does *not* touch this: the session was
            // auto-created by a dispatch (§4.1), which is true for the rest of
            // its life however many times control changes hands. Unnamed on
            // purpose — the UI labels it from the task it was created for.
            origin: SessionOrigin::Orchestrator,
            name: None,
            mcp_grant_session,
        })))
    }

    /// Start a fresh harness on the blocking pool.
    ///
    /// [`PtyManager::open`] forks, execs, and may back off while a pty frees up
    /// — all of it blocking. Calling it inline parked a tokio worker for up to
    /// half a second per launch, and a burst of task frames could park most of
    /// the runtime, taking the inbox drain and every screen sampler down with
    /// it since they share one. So the launch goes to the blocking pool, which
    /// is what it is for.
    pub(super) async fn launch(&self, spec: LaunchSpec) -> Result<OpenedSession, String> {
        let gh_repo_is_set = spec.env.contains_key("GH_REPO");
        let sessions = self.sessions.clone();
        let id = tokio::task::spawn_blocking(move || sessions.open(spec))
            .await
            .map_err(|err| format!("pty launch did not complete: {err}"))??;
        let harness_session_id = self.sessions.row(&id).and_then(|row| row.session_id);
        Ok(OpenedSession {
            id,
            harness_session_id,
            reused: false,
            gh_repo_is_set,
        })
    }

    /// The environment and extra argv a fresh launch spawns with: this
    /// task-scoped environment, layered with the `[router]` injection the
    /// headless executor already applies at its own spawn seam.
    ///
    /// Without this, switching the local host to `PtySessionExecutor` silently
    /// dropped a configured router — the child spawned against its own default
    /// endpoint instead of the one the operator pointed it at, with no error to
    /// say so.
    ///
    /// # Errors
    ///
    /// A configured `apiKeyEnv` whose named variable is unset in this
    /// executor's environment is a hard error, matching the headless path: a
    /// silently-empty key would spawn the harness unauthenticated against the
    /// routed endpoint.
    fn spawn_env(
        &self,
        options: &RunTaskOptions,
    ) -> Result<(HashMap<String, String>, Vec<String>), String> {
        let mut env = options.env.clone();
        // This executor launches the watched harness itself, bypassing the
        // daemon's transport dispatcher. Keep the embedded core workspace out
        // of that child for the same credential-store isolation as headless
        // and alternate transports.
        medulla::protocol::env::scrub_core_state(&mut env, options.provider);
        let mut extra_args = options.extra_args.clone();
        // Commits made in a watched PTY session are just as much Medulla's work
        // as headless ones, so this path carries the same attribution.
        let attribution_env = medulla::attribution::attribution_env(options.attribution, &env);
        env.extend(attribution_env);
        // Attribution and the operator's configured hooks share Claude Code's
        // single `--settings` flag, so both are built together — a watched PTY
        // session runs the same lifecycle policy a headless one does.
        let (launch_args, hook_notes) = medulla::harness_hooks::launch_args(
            options.provider,
            options.attribution,
            &options.hooks,
            &env,
        );
        extra_args.extend(launch_args);
        // Routed to the log rather than stderr: this crate draws a full-screen
        // TUI, where a stray line corrupts the pane. Covers both hooks the
        // harness cannot run and hooks it will not run until trusted.
        if let Some(log) = &self.log {
            for note in &hook_notes {
                log(note);
            }
        }
        // OpenRouter-bound runs are re-pointed at Medulla's loopback attribution
        // proxy, and the real key is scrubbed from `env` here, before any of it
        // reaches the child. A no-op for every other endpoint.
        let mut router = options.router.clone();
        medulla::inference_proxy::route_spawn(options.provider, &mut router, &mut env)?;
        if let Some(router) = &router {
            let injection = medulla::protocol::env::router_env(options.provider, router);
            for (key, value) in injection.env {
                env.insert(key, value);
            }
            for (child_var, source_name) in injection.secret_env {
                // Resolved from `env`, not `options.env`: when the run was routed
                // through the attribution proxy the name to resolve is the token
                // the routing just placed there, and the original key has been
                // scrubbed. Cloned before inserting so the read does not borrow
                // across the write.
                let secret = env
                    .get(&source_name)
                    .filter(|value| !value.is_empty())
                    .cloned();
                match secret {
                    Some(secret) => {
                        env.insert(child_var, secret);
                    }
                    None => {
                        return Err(format!(
                            "router API key env var `{source_name}` is not set; \
                             export it or remove apiKeyEnv from [router]"
                        ));
                    }
                }
            }
            extra_args.extend(injection.args);
        }
        // Codex needs more than an endpoint before a routed model will answer:
        // a provider block, an API-key auth preference, and a catalog entry it
        // is willing to describe. Read from `env`, which now holds both the
        // preset's opt-in knobs and the endpoint the routing above wrote.
        extra_args.extend(
            medulla::codex_overrides::launch_args(options.provider, options.model.as_deref(), &env)
                .map_err(|error| error.to_string())?,
        );
        Ok((env, extra_args))
    }
}
