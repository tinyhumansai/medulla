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

use medulla::tinyplace::HarnessProvider;

use crate::worker::pty::{HarnessControl, LaunchSpec};

use super::LocalHarnesses;

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
        provider: HarnessProvider,
        cwd: &str,
        skip_permissions: bool,
    ) -> Result<String, String> {
        let cwd = if cwd.trim().is_empty() {
            self.workspace.clone()
        } else {
            expand_home(cwd.trim())
        };
        if !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("{cwd} is not a directory"));
        }

        let bin = medulla::tinyplace::env::provider_bin(provider, &self.env);
        let (env, extra_args) = self.spawn_env(provider)?;

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
            label: format!("you:{}", provider.as_str()),
            model: None,
            session_id: None,
            control: HarnessControl::User,
            user_spawned: true,
        })
    }

    /// The environment and extra argv an operator-started harness spawns with.
    ///
    /// Mirrors the executor's own spawn seam so a hand-started harness and a
    /// dispatched one reach the same endpoint: a configured `[router]` that
    /// applied to one but not the other would silently change which model
    /// answered, with nothing on screen to say so.
    fn spawn_env(
        &self,
        provider: HarnessProvider,
    ) -> Result<(std::collections::HashMap<String, String>, Vec<String>), String> {
        let mut env = self.env.clone();
        let mut extra_args = Vec::new();
        let Some(router) = &self.router else {
            return Ok((env, extra_args));
        };
        let injection = medulla::tinyplace::env::router_env(provider, router);
        for (key, value) in injection.env {
            env.insert(key, value);
        }
        for (child_var, source_name) in injection.secret_env {
            match self.env.get(&source_name).filter(|v| !v.is_empty()) {
                Some(secret) => {
                    env.insert(child_var, secret.clone());
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
        Ok((env, extra_args))
    }
}

/// Expand a leading `~` against `$HOME`.
///
/// An operator typing a path into the composer writes `~/work/foo`, which no
/// syscall understands — the shell would have expanded it, and there is no shell
/// here.
fn expand_home(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    let Ok(home) = std::env::var("HOME") else {
        return path.to_string();
    };
    if rest.is_empty() {
        return home;
    }
    match rest.strip_prefix('/') {
        Some(tail) => format!("{}/{tail}", home.trim_end_matches('/')),
        // `~other` is another user's home, which we cannot resolve; leaving it
        // alone produces an honest "not a directory" rather than a wrong path.
        None => path.to_string(),
    }
}
