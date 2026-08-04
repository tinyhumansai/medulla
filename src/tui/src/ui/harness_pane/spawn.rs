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

use crate::worker::pty::{HarnessControl, LaunchSpec, SessionOrigin};

use super::{HarnessChoice, LocalHarnesses};

impl LocalHarnesses {
    /// Who currently holds `session_id`.
    pub fn control(&self, session_id: &str) -> Option<HarnessControl> {
        self.sessions.control(session_id)
    }

    /// Hand `session_id` to `control`; `false` when no such session exists.
    pub fn set_control(&self, session_id: &str, control: HarnessControl) -> bool {
        self.sessions.set_control(session_id, control)
    }

    /// Start a harness the operator owns, returning its session id.
    ///
    /// `cwd` is where the child runs; an empty string means the host's
    /// workspace. The session opens [`HarnessControl::User`]-held, which is the
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
        let bin = medulla::protocol::env::provider_bin(provider, &self.env);
        let (env, extra_args) = self.spawn_env(choice)?;
        let model = choice.preset.as_ref().map(|preset| preset.model.clone());

        self.sessions.open(LaunchSpec {
            provider,
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
            control: HarnessControl::User,
            // A person asked for this one, so it is theirs by origin as well as
            // by control — and it stays user-originated even after they hand it
            // to the orchestrator, which is the case the two fields exist to
            // tell apart.
            origin: SessionOrigin::User,
            name,
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
        // A harness the operator opened by hand is still one Medulla launched,
        // so its commits carry the same trailer a dispatched task's do. This is
        // the seam the executor's own `spawn_env` cannot reach — without it,
        // attribution depended on which door the session came through.
        let attribution_env = medulla::attribution::attribution_env(self.attribution, &env);
        env.extend(attribution_env);
        extra_args.extend(medulla::attribution::attribution_args(
            choice.provider,
            self.attribution,
        ));
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
