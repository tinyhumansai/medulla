//! Deciding which session serves a task.
//!
//! [`PtySessionExecutor::probe_session`] asks, off the runtime, whether a
//! session already exists for this task and whether a person holds the checkout;
//! [`PtySessionExecutor::session_for`] turns that answer into a plan. The two
//! are split out of `run` because the first belongs on the blocking pool — it
//! waits out completion-chime grace and canonicalizes paths — and the second
//! must stay on the runtime, where a `RunTaskOptions` borrow can end; see their
//! own docs.
//!
//! A claimed idle session is carried in [`IdleClaim`], which releases the claim
//! on drop unless it was taken: a probe cancelled while its blocking call is
//! still in flight otherwise marks the session busy and loses the row that would
//! have freed it.

use medulla::daemon::providers::RunTaskOptions;
use medulla::protocol::HarnessProvider;
use medulla::sessions::SessionClass;

use super::super::pty::{LaunchSpec, SessionControl, SessionOrigin};
use super::types::{IdleClaim, OpenedSession, SessionPlan, SessionProbe};
use super::PtySessionExecutor;

impl PtySessionExecutor {
    /// Ask, off the runtime, whether a session already exists for this task.
    ///
    /// Both questions block, which is the whole reason this is a separate step
    /// on the blocking pool rather than the head of
    /// [`session_for`](Self::session_for):
    ///
    /// - [`claim_idle`](crate::worker::pty::PtyManager::claim_idle) waits out the
    ///   previous turn's completion-chime grace with a `std::thread::sleep`, up
    ///   to 300ms per claim.
    /// - [`checkout_writer`](Self::checkout_writer) canonicalizes both sides of
    ///   every live session's `cwd`, which is a filesystem call apiece.
    ///
    /// Owned arguments rather than `&RunTaskOptions`, because that type is
    /// `Send` but not `Sync`: a borrow of it held across this await would make
    /// the whole run future un-spawnable.
    ///
    /// **Candidacy (spec §4.1).** Only *orchestrator-owned* sessions are ever
    /// candidates. A user-owned session — born that way as an unmanaged spawn,
    /// or taken at runtime — is not one, so a person working in a session never
    /// makes a dispatch fail: it is simply not among the things the dispatch can
    /// pick up. That rule lives in
    /// [`try_claim`](crate::worker::pty::PtyManager::claim_idle), which is why
    /// reuse is consulted *first* here. It used to come second, behind a
    /// workspace-wide refusal that turned a person at a keyboard into a task
    /// error — even when the agent had another session sitting idle beside them.
    ///
    /// # Errors
    ///
    /// The blocking task panicked or was cancelled; the dispatch has claimed
    /// nothing and started nothing.
    pub(in crate::worker) async fn probe_session(
        &self,
        class: SessionClass,
        conversation: String,
        provider: HarnessProvider,
        cwd: &str,
    ) -> Result<SessionProbe, String> {
        let this = self.clone();
        let cwd = cwd.to_string();
        tokio::task::spawn_blocking(move || {
            if class == SessionClass::Unbound {
                // Reuse this peer's session only when it is *idle*. A harness
                // serves one turn at a time: a fan-out that pastes three prompts
                // into one composer gets them answered as a single conversation,
                // and all three tails settle on the same completion — three
                // different instructions, one answer, delivered three times. A
                // busy session therefore does not qualify, and the task gets a
                // fresh one.
                if let Some(row) = this.sessions.claim_idle(&conversation, provider) {
                    return SessionProbe::Reuse(Box::new(IdleClaim::new(
                        this.sessions.clone(),
                        row,
                    )));
                }
            }
            // Nothing to reuse, so this dispatch needs a session of its own —
            // and that is where the *second*, independent rule applies: under
            // `strategy: checkout` the working tree takes one writer at a time
            // (see [`checkout_writer`](PtySessionExecutor::checkout_writer)), so
            // a fresh harness cannot simply start beside the one that is there.
            // The work queues instead — the same exclusivity the blanket refusal
            // used to buy, without ending the dispatch to get it.
            //
            // Note what this is *not*: it is not "the workspace is held". Holds
            // are on sessions, and rule 1 above has already dealt with those.
            // This is the strategy's serialization, and under `worktree` it will
            // not apply at all.
            if this.checkout_writer(&cwd).is_some() {
                return SessionProbe::Queue;
            }
            SessionProbe::Fresh
        })
        .await
        .map_err(|err| format!("session lookup did not complete: {err}"))
    }

    /// Turn a [`SessionProbe`] into the plan that serves this task: reuse the
    /// session it claimed, launch, or queue behind the person in the checkout.
    ///
    /// Synchronous and non-blocking — every question that touches a pty or the
    /// filesystem was already answered by
    /// [`probe_session`](Self::probe_session) — and it returns a plan rather
    /// than a session because neither the launch nor the wait may happen here;
    /// see [`PtySessionExecutor::launch`].
    pub(super) fn session_for(
        &self,
        options: &RunTaskOptions,
        probe: SessionProbe,
    ) -> Result<SessionPlan, String> {
        match probe {
            SessionProbe::Reuse(claim) => {
                let row = claim.into_row();
                return Ok(SessionPlan::Reuse(OpenedSession {
                    id: row.id.clone(),
                    harness_session_id: row.session_id.clone(),
                    reused: true,
                    gh_repo_is_set: self.sessions.gh_repo_is_set(&row.id).unwrap_or(false),
                }));
            }
            SessionProbe::Queue => return Ok(SessionPlan::Queue(options.cwd.clone())),
            SessionProbe::Fresh => {}
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
}
