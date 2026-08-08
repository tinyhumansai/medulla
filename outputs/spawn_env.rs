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

    /// Fold whatever the harness has written since the last poll, and answer
    /// with the turn's result if that fold completed it.
    ///
    /// Shared by the polling loop and by the suspend path, and shared
    /// deliberately: "read what is already there before doing anything else" has
    /// to mean the same thing in both, or a turn that finished microseconds
    /// before an operator took the session would have its answer read by one
    /// path and dropped by the other.
    ///
    /// `last_line_at` is advanced per line rather than per call, because it is
    /// the idle watchdog's clock and a batch of lines is progress at the time
    /// each of them was read, not at the time the batch was drained.
