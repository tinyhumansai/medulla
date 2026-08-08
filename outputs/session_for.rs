    /// makes a dispatch fail: it is simply not among the things the dispatch can
    /// pick up. That rule lives in
    /// [`try_claim`](crate::worker::pty::PtyManager::claim_idle), which is why
    /// reuse is consulted *first* here now. It used to come second, behind a
    /// workspace-wide refusal that turned a person at a keyboard into a task
    /// error — even when the agent had another session sitting idle beside them.
    fn session_for(
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
