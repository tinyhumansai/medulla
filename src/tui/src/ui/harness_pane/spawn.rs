//! Starting a harness the orchestrator will not dispatch into.
//!
//! Every other harness on this device exists because a task frame needed one.
//! This is the other door: the operator asks for a `claude` in a folder, gets a
//! real terminal, and the orchestrator never touches it. Mechanically it is the
//! same [`PtyManager::open`](crate::worker::pty::PtyManager::open) call the
//! executor makes — the difference is entirely in who the session says it
//! belongs to, which is what [`claim_idle`] reads before reusing anything.
//!
//! [`claim_idle`]: crate::worker::pty::PtyManager::claim_idle

use crate::worker::pty::{LaunchSpec, SessionControl, SessionOrigin};

use super::{HarnessChoice, LocalSessions};

impl LocalSessions {
    /// Who currently holds `session_id`.
    pub fn control(&self, session_id: &str) -> Option<SessionControl> {
        self.sessions.control(session_id)
    }

    /// Hand `session_id` to `control`; `false` when no such session exists.
    pub fn set_control(&self, session_id: &str, control: SessionControl) -> bool {
        self.sessions.set_control(session_id, control)
    }

    /// Start a harness the operator owns, returning its session id.
    ///
    /// `cwd` is where the child runs; an empty string means the host's
    /// workspace. The session opens [`SessionControl::User`]-held, which is the
    /// whole of "unmanaged" — dispatch skips it until it is handed over.
    ///
    /// # Errors
    ///
    /// Fails when the working directory does not exist, when the provider's
    /// binary cannot be resolved, or when the PTY or child cannot be started.
    /// The directory is checked here rather than left to the spawn because
    /// `posix_spawn` reports a bad `cwd` as a generic failure, and "could not
    /// start claude" is a much worse message than naming the folder.
    pub fn open_unmanaged(
        &self,
        choice: &HarnessChoice,
        cwd: &str,
        skip_permissions: bool,
    ) -> Result<String, String> {
        self.open_unmanaged_named(choice, cwd, skip_permissions, None)
    }

    /// [`open_unmanaged`](Self::open_unmanaged), with the display name the
    /// person spinning the session up gave it.
    ///
    /// The seam for the picker's name prompt: a session a person starts is
    /// [`SessionOrigin::User`]-originated and is the only kind that carries a
    /// name, because a dispatched one is labelled from its task instead. `None`
    /// leaves it unnamed, which is what the picker passes until it asks.
    ///
    /// # Errors
    ///
    /// As [`open_unmanaged`](Self::open_unmanaged).
    pub fn open_unmanaged_named(
        &self,
        choice: &HarnessChoice,
        cwd: &str,
        skip_permissions: bool,
        name: Option<String>,
    ) -> Result<String, String> {
        let cwd = self.resolve_workspace(cwd);
        if !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("{cwd} is not a directory"));
        }

        let provider = choice.provider;
        let bin = choice.bin(&self.env);
        // A shell is not a harness, and every step below this line is a favour
        // done to a coding agent: tools it can call, skills describing them, a
        // router pointing it at a model, a commit trailer saying Medulla ran
        // it. None of them mean anything at a `$` prompt, and the attribution
        // one would be a lie — commits an operator types by hand are theirs.
        if choice.is_shell() {
            return self.open_shell(choice, &bin, &cwd, name);
        }
        let (mut env, mut extra_args) = self.spawn_env(choice)?;
        // The operator's own session gets Medulla's tools too. This is the door
        // a person is actually sitting in, so it is the one where a missing
        // `workflow_run` is noticed — and, until now, the one that never had it.
        let mcp_grant_session = crate::worker::pty::launch::attach_mcp(
            provider,
            &bin,
            &mut env,
            &mut extra_args,
            self.log.as_ref(),
        );
        // And the knowledge to use them: the tools alone leave the session
        // reaching for a `workflow_run` it has no reason to call and no idea
        // what to pass. Appends nothing unless managed skills are installed.
        crate::worker::pty::launch::attach_skills(
            provider,
            &env,
            std::path::Path::new(&cwd),
            &mut extra_args,
        );
        let model = choice.preset.as_ref().map(|preset| preset.model.clone());

        self.sessions.open(LaunchSpec {
            provider,
            // The preset is the agent, not the CLI under it: a declaration for a
            // preset records the preset's id, so a session that recorded only
            // `claude` could never be matched back to the agent that declared
            // it and was listed as belonging to none.
            preset: choice.preset.as_ref().map(|preset| preset.id.clone()),
            bin,
            cwd,
            env,
            extra_args,
            // An operator is sitting in this pane. The bypass flag exists
            // because a *watched* session is still unattended and a permission
            // dialog hangs it — the opposite is true here, so the harness keeps
            // its own guardrails and the person who asked for it answers them.
            skip_permissions,
            label: format!("you:{}", choice.id()),
            model,
            session_id: None,
            control: SessionControl::User,
            // A person asked for this one, so it is theirs by origin as well as
            // by control — and it stays user-originated even after they hand it
            // to the orchestrator, which is the case the two fields exist to
            // tell apart.
            origin: SessionOrigin::User,
            name,
            mcp_grant_session,
        })
    }

    /// Start a plain interactive shell in `cwd`.
    ///
    /// The whole of the difference from a harness launch is what is *not* here:
    /// no MCP registration, no managed skills, no router injection, no
    /// attribution environment. What is left is the operator's own environment
    /// with the embedded core's state scrubbed out of it — the one thing a
    /// shell must not inherit, because a `cargo test` run started from it would
    /// otherwise resolve the live credential store as its keyring (see
    /// [`medulla::protocol::env::scrub_core_state`]).
    ///
    /// `bin` is the shell the picker row named, already resolved. No argv: a
    /// shell handed a tty and no command is interactive by definition, and
    /// every flag that could be added here — `-l`, `-i` — changes which startup
    /// files run and so which prompt the operator gets.
    ///
    /// # Errors
    ///
    /// As [`open_unmanaged`](Self::open_unmanaged): a bad directory, or a shell
    /// that cannot be executed.
    fn open_shell(
        &self,
        choice: &HarnessChoice,
        bin: &str,
        cwd: &str,
        name: Option<String>,
    ) -> Result<String, String> {
        let mut env = self.env.clone();
        medulla::protocol::env::scrub_core_state(&mut env, choice.provider);
        self.sessions.open(LaunchSpec {
            provider: choice.provider,
            preset: None,
            bin: bin.to_string(),
            cwd: cwd.to_string(),
            env,
            extra_args: Vec::new(),
            // There are no permissions to skip: the shell is the permission.
            skip_permissions: false,
            label: format!("you:{}", choice.id()),
            model: None,
            session_id: None,
            // Held by the operator and, unlike a harness, never handed over:
            // dispatch has nothing it could send a shell and no way to read an
            // answer back. `HarnessProvider::Shell` is refused at the wire
            // parse for the same reason.
            control: SessionControl::User,
            origin: SessionOrigin::User,
            name,
            mcp_grant_session: None,
        })
    }

    /// Resolve picker input to the absolute directory a child will receive.
    ///
    /// Blank input uses the host workspace, `~` follows the harness
    /// environment, and relative input is rooted at the host workspace rather
    /// than whichever directory happens to contain the TUI process.
    pub fn resolve_workspace(&self, cwd: &str) -> String {
        let input = if cwd.trim().is_empty() {
            return self.workspace.clone();
        } else {
            expand_home(cwd.trim(), &self.env)
        };
        let path = std::path::Path::new(&input);
        if path.is_absolute() {
            input
        } else {
            std::path::Path::new(&self.workspace)
                .join(path)
                .to_string_lossy()
                .into_owned()
        }
    }

    /// The environment and extra argv an operator-started harness spawns with.
    ///
    /// Mirrors the executor's own spawn seam so a hand-started harness and a
    /// dispatched one reach the same endpoint: a configured `[router]` that
    /// applied to one but not the other would silently change which model
    /// answered, with nothing on screen to say so.
    pub(super) fn spawn_env(
        &self,
        choice: &HarnessChoice,
    ) -> Result<(std::collections::HashMap<String, String>, Vec<String>), String> {
        let mut env = self.env.clone();
        let mut extra_args = Vec::new();
        // This is an independent spawn seam from dispatched PTY work. A
        // harness opened by the operator must not inherit the embedded core's
        // workspace either, or its own nested OpenHuman commands can mutate
        // the live credential store.
        medulla::protocol::env::scrub_core_state(&mut env, choice.provider);
        // A harness the operator opened by hand is still one Medulla launched,
        // so its commits carry the same trailer a dispatched task's do. This is
        // the seam the executor's own `spawn_env` cannot reach — without it,
        // attribution depended on which door the session came through.
        let attribution_env = medulla::attribution::attribution_env(self.attribution, &env);
        env.extend(attribution_env);
        let (launch_args, hook_notes) = medulla::harness_hooks::launch_args(
            choice.provider,
            self.attribution,
            &self.hooks,
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
        let custom_router = choice.preset.as_ref().map(|preset| preset.router());
        // OpenRouter-bound sessions are re-pointed at Medulla's loopback
        // attribution proxy and the real key is scrubbed from `env`. A hand-opened
        // harness is Medulla's traffic just as much as a dispatched one, so it
        // carries the same attribution — and must be just as unable to bypass it.
        let mut router = custom_router.or_else(|| self.router.clone());
        medulla::inference_proxy::route_spawn(choice.provider, &mut router, &mut env)?;
        let Some(router) = router else {
            return Ok((env, extra_args));
        };
        let injection = medulla::protocol::env::router_env(choice.provider, &router);
        for (key, value) in injection.env {
            env.insert(key, value);
        }
        for (child_var, source_name) in injection.secret_env {
            // From `env`, not `self.env`: a routed run resolves the proxy token
            // the routing just placed there, and the real key is already gone.
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
        if let Some(preset) = &choice.preset {
            env.extend(preset.harness_env());
        }
        // Codex needs more than an endpoint before a routed model will answer: a
        // provider block, an API-key auth preference, and a catalog entry it is
        // willing to describe. Applied after the preset's environment, which is
        // where its opt-in and its knobs come from.
        extra_args.extend(
            medulla::codex_overrides::launch_args(
                choice.provider,
                choice.preset.as_ref().map(|preset| preset.model.as_str()),
                &env,
            )
            .map_err(|error| error.to_string())?,
        );
        Ok((env, extra_args))
    }
}

/// Expand a leading `~` against `$HOME`.
///
/// An operator typing a path into the composer writes `~/work/foo`, which no
/// syscall understands — the shell would have expanded it, and there is no shell
/// here.
fn expand_home(path: &str, env: &std::collections::HashMap<String, String>) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    let Some(home) = env.get("HOME") else {
        return path.to_string();
    };
    if rest.is_empty() {
        return home.clone();
    }
    match rest.strip_prefix('/') {
        Some(tail) => format!("{}/{tail}", home.trim_end_matches('/')),
        // `~other` is another user's home, which we cannot resolve; leaving it
        // alone produces an honest "not a directory" rather than a wrong path.
        None => path.to_string(),
    }
}
