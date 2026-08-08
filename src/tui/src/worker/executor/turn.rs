//! Poll the harness transcript until the turn is settled.
//!
//! [`super::PtySessionExecutor::fold_available`] drains the tailed transcript
//! through [`medulla::sessions::TurnStream`], and
//! [`super::PtySessionExecutor::await_turn`] is the main polling loop that
//! runs until the harness states the turn is done, the turn times out, or an
//! operator takes control.

use medulla::daemon::providers::{Abort, OnEvent, RunTaskResult};
use medulla::protocol::HarnessProvider;
use medulla::sessions::TurnStream;
use medulla::wrapper::tail::SessionTailer;

use super::super::pty::SessionControl;
use super::types::{PtySessionExecutor, TurnSpec};

impl PtySessionExecutor {
    /// Fold whatever the harness has written since the last poll, and answer
    /// with the turn's result if that fold completed it.
    ///
    /// Shared by the polling loop and by the suspend path, and shared
    /// deliberately: "read what is already there before doing anything else"
    /// has to mean the same thing in both, or a turn that finished
    /// microseconds before an operator took the session would have its answer
    /// read by one path and dropped by the other.
    ///
    /// `last_line_at` is advanced per line rather than per call, because it
    /// is the idle watchdog's clock and a batch of lines is progress at the
    /// time each of them was read, not at the time the batch was drained.
    pub(super) fn fold_available(
        &self,
        id: &str,
        provider: HarnessProvider,
        tailer: &mut SessionTailer,
        stream: &mut TurnStream,
        on_event: &mut Option<OnEvent>,
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
    /// budgets below: [`super::run::LOCATE_BUDGET`] covers a harness that
    /// never starts a turn at all, and [`super::run::STALL_BUDGET_MS`] is a
    /// soft "probably finished" signal for a transcript that stops without a
    /// stated reason. A caller configuring a shorter ceiling than either
    /// means it, and is honored ahead of them.
    pub(super) async fn await_turn(
        &self,
        id: &str,
        spec: TurnSpec,
        mut tailer: SessionTailer,
        abort: Abort,
        mut on_event: Option<OnEvent>,
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
                callback(event);
            }
        }
        let mut started = tokio::time::Instant::now();
        let mut last_line_at = medulla::clock::now_millis();

        // `TailPoll.located` is emitted only on first sighting, so the
        // Codex thread label discovered there would not reflect a later
        // /rename.  We stash the harness session id after the first
        // location and periodically re-index in the background.
        let mut poll_ticks: u64 = 0;

        loop {
            poll_ticks = poll_ticks.wrapping_add(1);
            // Taking control is an ownership transfer, not merely a display
            // preference, so it is answered before aborts or transcript
            // output: from here the executor must not send Ctrl-C, report a
            // stale completion, or close the PTY underneath the operator.
            //
            // What it does instead is **suspend** (spec §5). The turn used
            // to return an error here, throwing away everything the harness
            // had produced and telling the orchestrator its task had failed
            // — for the entirely ordinary event of a person opening the
            // session to look. Now the fold, its events, its usage and its
            // workspace context all stay exactly where they are, the session
            // keeps the work, and the task stays open.
            if self.sessions.control(id) == Some(SessionControl::User) {
                // Everything already written belongs to *this* turn — the
                // takeover cannot retroactively unwrite it. Folded out
                // before suspending, so a turn that finished in the instant
                // somebody took the session still reports the answer it had
                // reached.
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
                started = tokio::time::Instant::now();
                last_line_at = medulla::clock::now_millis();
                continue;
            }
            if abort.is_aborted() {
                if abort.is_terminated() {
                    self.stop_turn(id);
                } else {
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

            // Refresh the Codex thread label from the session index
            // periodically after initial transcript discovery.
            // `fold_available` only queries the index on first sighting
            // (when `TailPoll.located` is emitted); a later /rename would
            // otherwise not be observable until a subsequent turn recreates
            // the tailer.
            if provider == HarnessProvider::Codex && poll_ticks % 30 == 0 {
                if let Some(sid) =
                    self.sessions.row(id).and_then(|row| row.session_id.clone())
                {
                    if let Some(name) =
                        medulla::session_history::codex_thread_label(&self.env, &sid)
                    {
                        self.sessions.record_thread_name(id, name);
                    }
                }
            }

            if !tailer.is_located() && started.elapsed() > super::run::LOCATE_BUDGET {
                return Err(format!(
                    "{} never started a turn — check the session in the Sessions tab; \
                     it may be waiting on a prompt",
                    provider.as_str()
                ));
            }
            let idle_ms = medulla::clock::now_millis().saturating_sub(last_line_at);
            if timeout_ms > 0 && idle_ms as u64 >= timeout_ms {
                self.stop_turn(id);
                return Err(format!(
                    "{} task idle for {timeout_ms}ms (no events)",
                    provider.as_str()
                ));
            }
            if stream.terminal_pending() && idle_ms >= super::run::SETTLE_GRACE_MS {
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
            if tailer.is_located() && stream.stalled_for(idle_ms, super::run::STALL_BUDGET_MS) {
                return Ok(RunTaskResult {
                    provider,
                    reply: stream.settle_stalled(),
                    events: stream.events(),
                    usage: stream.usage(),
                    session_id: self.sessions.row(id).and_then(|row| row.session_id),
                });
            }
            tokio::time::sleep(super::run::POLL).await;
        }
    }
}
