    async fn launch(&self, spec: LaunchSpec) -> Result<OpenedSession, String> {
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
