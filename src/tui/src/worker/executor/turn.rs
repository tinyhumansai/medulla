//! Poll the harness transcript until the turn is settled.
//!
//! [`super::PtySessionExecutor::fold_available`] drains the tailed transcript
//! through [`medulla::sessions::TurnStream`], and
//! [`super::PtySessionExecutor::await_turn`] is the main polling loop that
//! runs until the harness states the turn is done, the turn times out, or an
//! operator takes control.

use medulla::daemon::providers::RunTaskResult;
use medulla::protocol::HarnessProvider;
use medulla::sessions::TurnStream;
use medulla::wrapper::tail::SessionTailer;

use super::super::pty::SessionControl;
use super::run::{LOCATE_BUDGET, POLL, SETTLE_GRACE_MS, STALL_BUDGET_MS};
use super::types::{PtySessionExecutor, TurnSpec};

impl PtySessionExecutor {
    fn fold_available(
        &self,
        id: &str,
        provider: HarnessProvider,
        tailer: &mut SessionTailer,
        stream: &mut TurnStream,
        on_event: &mut Option<medulla::daemon::providers::OnEvent>,
        last_line_at: &mut i64,
    ) -> Option<RunTaskResult> {
        let poll = tailer.poll();
        // Codex cannot be told its id, so it is learned from the rollout the
        // first time the tailer locates one.
        if let Some(located) = &poll.located {
            self.sessions
                .record_session_id(id, located.harness_session_id.clone());
            if provider == HarnessProvider::Codex {
                if let Some(thread_name) = medulla::session_history::codex_thread_label(
                    &self.env,
                    &located.harness_session_id,
                ) {
                    self.sessions.record_thread_name(id, thread_name);
                }
            }
        }
        for line in poll.lines {
            *last_line_at = medulla::clock::now_millis();
            let fold = stream.observe(&line.text);
            self.workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .insert(id.to_string(), stream.workspace_context());
            // The peer watches its task through these. Dropping them would
            // leave it with an ack, silence, then a reply — which is what
            // this executor used to do.
            if let Some(callback) = on_event.as_mut() {
                for event in &fold.events {
                    callback(event);
                }
            }
            if let Some(reply) = fold.reply {
                return Some(RunTaskResult {
                    provider,
                    reply,
                    events: stream.events(),
                    usage: stream.usage(),
                    session_id: self.sessions.row(id).and_then(|row| row.session_id),
                });
            }
        }
        None
    }

    /// Poll the transcript until the harness says the turn is over.
    ///
    /// `timeout_ms` is the caller's configured idle watchdog (`[host]
    /// .taskTimeoutMs`, mirroring the headless executor's own `timeout_ms`) —
    /// the hard ceiling on how long a turn may go without producing a single
    /// transcript line. It is distinct from, and can override, the two fixed
    /// budgets below: [`LOCATE_BUDGET`] covers a harness that never starts a
    /// turn at all, and [`STALL_BUDGET_MS`] is a soft "probably finished"
    /// signal for a transcript that stops without a stated reason. A caller
    /// configuring a shorter ceiling than either means it, and is honored
    /// ahead of them.
    pub(super) async fn await_turn(
        &self,
        id: &str,
        spec: TurnSpec,
        mut tailer: SessionTailer,
        abort: medulla::daemon::providers::Abort,
        mut on_event: Option<medulla::daemon::providers::OnEvent>,
    ) -> Result<RunTaskResult, String> {
        let TurnSpec {
            provider,
            gh_repo_is_set,
            timeout_ms,
            instruction,
        } = spec;
        let mut stream = TurnStream::new_with_gh_repo_override(provider, gh_repo_is_set);
        if let Some((cwd, branch, pull_request)) = self
            .workspace_context
            .lock()
            .expect("workspace context lock poisoned")
            .get(id)
            .cloned()
        {
            stream.set_workspace_context(cwd, branch, pull_request);
            if let (Some(callback), Some(event)) =
                (on_event.as_mut(), stream.retained_workspace_event())
            {
                callback(&event);
            }
        }
        let mut started = tokio::time::Instant::now();
        let mut last_line_at = medulla::clock::now_millis();

        loop {
            // Taking control is an ownership transfer, not merely a display
            // preference, so it is answered before aborts or transcript output:
            // from here the executor must not send Ctrl-C, report a stale
            // completion, or close the PTY underneath the operator.
            //
            // What it does instead is **suspend** (spec §5). The turn used to
            // return an error here, throwing away everything the harness had
            // produced and telling the orchestrator its task had failed — for
            // the entirely ordinary event of a person opening the session to
            // look. Now the fold, its events, its usage and its workspace
            // context all stay exactly where they are, the session keeps the
            // work, and the task stays open.
            if self.sessions.control(id) == Some(SessionControl::User) {
                // Everything already written belongs to *this* turn — the
                // takeover cannot retroactively unwrite it. Folded out before
                // suspending, so a turn that finished in the instant somebody
                // took the session still reports the answer it had reached.
                if let Some(result) = self.fold_available(
                    id,
                    provider,
                    &mut tailer,
                    &mut stream,
                    &mut on_event,
                    &mut last_line_at,
                ) {
                    return Ok(result);
                }
                super::hold::report_held(&mut on_event, provider);
                self.await_handback(id, provider, &abort).await?;
                // The lines the operator's own work wrote are theirs, not this
                // turn's: dropped rather than folded, or the person's last
                // exchange would settle the task as its answer. What they did is
                // not lost — it is in the session, which is exactly what the
                // hand-back turn is told to go and read.
                let poll = tailer.poll();
                if let Some(located) = &poll.located {
                    self.sessions
                        .record_session_id(id, located.harness_session_id.clone());
                }
                super::hold::report_resumed(&mut on_event, provider);
                super::super::pty::inject_prompt(
                    &self.sessions,
                    id,
                    &super::hold::handback_prompt(&instruction),
                )
                .await?;
                // Both budgets restart with the hand-back turn, which is what
                // "the watchdog is paused, not lengthened" means on this side:
                // held time is excluded rather than counted, so a session held
                // over lunch is not a task that timed out at the desk.
                started = tokio::time::Instant::now();
                last_line_at = medulla::clock::now_millis();
                continue;
            }
            if abort.is_aborted() {
                if abort.is_terminated() {
                    self.stop_turn(id);
                } else {
                    // A requester abort is an interrupt: Ctrl-C reaches the
                    // harness the same way the operator's would, and the
                    // reusable session survives it.
                    let _ = self.sessions.write(id, &[0x03]);
                }
                return Err(format!("{} task aborted", provider.as_str()));
            }
            if !self
                .sessions
                .row(id)
                .is_some_and(|row| row.state.is_running())
            {
                return Err(format!(
                    "{} session ended before the turn did",
                    provider.as_str()
                ));
            }

            if let Some(result) = self.fold_available(
                id,
                provider,
                &mut tailer,
                &mut stream,
                &mut on_event,
                &mut last_line_at,
            ) {
                return Ok(result);
            }

            // Codex `/rename` names are re-read for the session's whole life by
            // the manager's per-session label poller (`PtyManager::spawn_codex_label_poller`),
            // which is keyed to the PTY session lifecycle and therefore also runs
            // while the turn is held, idle, or retained. Nothing else re-reads
            // the index here.

            if !tailer.is_located() && started.elapsed() > LOCATE_BUDGET {
                // A harness writes its transcript once it starts a turn, so an
                // absent one usually means it never started one — most often
                // because it is still waiting on something on screen that
                // `blocking_dialog` did not recognise. Say where to look; the
                // bare "could not find the transcript" sent operators hunting
                // through `~/.claude/projects` for a file that was never going
                // to exist.
                return Err(format!(
                    "{} never started a turn — check the session in the Sessions tab; \
                     it may be waiting on a prompt",
                    provider.as_str()
                ));
            }
            let idle_ms = medulla::clock::now_millis().saturating_sub(last_line_at);
            // The configured idle ceiling, checked first so a caller-set budget
            // shorter than the fixed ones below actually takes effect instead of
            // being silently outlived by them. `timeout_ms == 0` means no
            // configured ceiling (never observed from `[host]`, whose default is
            // nonzero, but a defensive floor all the same).
            if timeout_ms > 0 && idle_ms as u64 >= timeout_ms {
                // Stop the harness before reporting the failure. A timeout is
                // only silence on the *transcript* — the child is very much
                // alive and may still be editing the workspace. Returning
                // without stopping it tells the peer the task failed while the
                // work carries on unattributed, and an unbound session would
                // then be released as idle for the next task to claim, landing
                // its prompt in a harness that is still mid-turn.
                self.stop_turn(id);
                return Err(format!(
                    "{} task idle for {timeout_ms}ms (no events)",
                    provider.as_str()
                ));
            }
            // The turn ended, but its message is written one record per content
            // block and the reply usually lives in the last one. Normally the
            // records that follow close it immediately; this covers a transcript
            // that simply stops, so a finished turn is never held for the full
            // stall budget.
            if stream.terminal_pending() && idle_ms >= SETTLE_GRACE_MS {
                if let Some(reply) = stream.settle_pending() {
                    return Ok(RunTaskResult {
                        provider,
                        reply,
                        events: stream.events(),
                        usage: stream.usage(),
                        session_id: self.sessions.row(id).and_then(|row| row.session_id),
                    });
                }
            }
            if tailer.is_located() && stream.stalled_for(idle_ms, STALL_BUDGET_MS) {
                return Ok(RunTaskResult {
                    provider,
                    reply: stream.settle_stalled(),
                    events: stream.events(),
                    usage: stream.usage(),
                    session_id: self.sessions.row(id).and_then(|row| row.session_id),
                });
            }
            tokio::time::sleep(POLL).await;
        }
    }
}
