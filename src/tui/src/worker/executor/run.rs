//! Running delegated tasks inside live, watchable harness sessions.
//!
//! [`PtySessionExecutor`] runs a delegated task inside a live, watchable
//! harness session instead of a headless one-shot.
//!
//! This is the piece that makes the worker TUI more than a dashboard. It
//! implements [`RunTaskFn`], the same seam the headless executor fills, so
//! [`DaemonRuntime`](medulla::daemon::DaemonRuntime) needs no changes at all:
//! admission control, duplicate rejection, correlation, ack/status/reply framing
//! and the concurrency budget keep working exactly as they do today. Only *how a
//! turn runs* changes.
//!
//! One turn is:
//!
//! 1. route the lifetime class from the task's origin,
//! 2. find or open a PTY session for that conversation,
//! 3. type the prompt into it, as a human would,
//! 4. tail **that session's** transcript, pinned by id,
//! 5. fold the lines through [`TurnStream`] until it says the turn is done.
//!
//! Step 5 is why this is reliable rather than a guess: the harness states when
//! it has finished, in its own transcript. And the fold is *shared* with the
//! headless mode ([`medulla::sessions::turn_stream`]), so the progress a peer
//! sees does not depend on which mode served it — the two differ only in where
//! the raw lines come from.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::protocol::HarnessProvider;
use medulla::session_history::SessionAgentKind;
use medulla::sessions::SessionClass;
use medulla::wrapper::tail::SessionTailer;

use super::super::pty::{PtyManager, SessionControl};
use super::types::{PtySessionExecutor, SessionPlan, TurnSpec, WorkspaceContext};

/// How often the transcript is polled while a turn runs.
///
/// Fast enough that a short turn settles promptly, slow enough that a long one
/// costs almost nothing. The transcript is a file on local disk, so this is a
/// stat plus a short read.
pub(super) const POLL: Duration = Duration::from_millis(150);

/// How long to keep looking for a session's transcript before giving up.
///
/// A harness writes its first record only once it has started work, which on a
/// cold start can take a few seconds.
pub(super) const LOCATE_BUDGET: Duration = Duration::from_secs(30);

/// Silence that settles a turn whose completion record carried no stated reason.
///
/// Only reachable for the ~0.08% of claude records with no `stop_reason`; the
/// watcher refuses to stall while a tool call is outstanding, so a long build is
/// never mistaken for a finished turn.
pub(super) const STALL_BUDGET_MS: i64 = 120_000;

/// How long to wait for the rest of a terminal message before replying with what
/// arrived. The blocks of one message are written in a single burst, so this only
/// has to outlast that write — it is a safety net, not the normal path.
pub(super) const SETTLE_GRACE_MS: i64 = 1_500;

impl PtySessionExecutor {
    /// Build an executor over the TUI's live session manager.
    /// The provider is not configured here: `DaemonRuntime` resolves it from the
    /// task frame (or its own default) before calling, so `options.provider` is
    /// always already decided.
    pub fn new(sessions: PtyManager, env: HashMap<String, String>, workspace: String) -> Self {
        PtySessionExecutor {
            sessions,
            env,
            workspace,
            claims: Arc::new(Mutex::new(HashSet::new())),
            workspace_context: Arc::new(Mutex::new(HashMap::new())),
            log: None,
        }
    }

    /// Route executor diagnostics into the owning TUI's log surface.
    pub fn with_log(mut self, log: medulla::daemon::LogFn) -> Self {
        self.log = Some(log);
        self
    }

    /// Adapt this executor into the [`RunTaskFn`] the daemon runtime takes.
    pub fn into_run_task(self) -> RunTaskFn {
        Arc::new(move |options: RunTaskOptions| {
            let this = self.clone();
            Box::pin(async move { this.run(options).await })
        })
    }

    /// Run one delegated task to completion, bypassing the `RunTaskFn` adapter.
    ///
    /// Test seam: exercises the same body the daemon reaches, without needing a
    /// `DaemonRuntime` to route a frame first.
    #[cfg(all(test, unix))]
    pub(in crate::worker) async fn run_for_test(
        self,
        options: RunTaskOptions,
    ) -> Result<RunTaskResult, String> {
        self.run(options).await
    }

    /// The live session manager, for assertions about what a run left behind.
    #[cfg(all(test, unix))]
    pub(in crate::worker) fn sessions_for_test(&self) -> PtyManager {
        self.sessions.clone()
    }

    /// Run one delegated task to completion.
    async fn run(&self, options: RunTaskOptions) -> Result<RunTaskResult, String> {
        let provider = options.provider;
        // opencode writes no transcript this can read, so a turn on it could
        // never be known to have finished. Refusing is honest; accepting would
        // hang the peer until its timeout.
        let agent = agent_kind(provider)
            .ok_or_else(|| format!("{} cannot run watchable tasks", provider.as_str()))?;

        // A task frame is discrete work and gets its own session; a
        // conversational message continues the peer's. Taken from the caller
        // rather than inferred from `conversation`: the daemon sets that field
        // to the authenticated sender for *every* inbound run, so it is never
        // empty, and the old `is_empty()` test therefore classified every task
        // frame as unbound. Two unrelated delegated tasks from one orchestrator
        // then shared a harness, each able to read the other's prompt and tool
        // context — the exact opposite of what the comment above promises.
        let class = options.session_class;
        // Built *before* the session is opened, and deliberately so. The tailer
        // snapshots the transcripts that already exist and ignores them, so that
        // the one new file is unambiguously this session's — which means the
        // snapshot has to be taken before the harness can write. A harness that
        // creates its transcript in the milliseconds after spawning would
        // otherwise be ignored by the very tailer waiting for it, and the turn
        // dies reporting that it never started. That race is intermittent, which
        // is the worst way to have it.
        let mut tailer = SessionTailer::new(
            self.env.clone(),
            agent,
            self.workspace.clone(),
            medulla::clock::now_millis(),
        )
        .with_claims(self.claims.clone());
        // Two steps, and the split is load-bearing: `RunTaskOptions` is `Send`
        // but not `Sync`, so a borrow of it held across an await would make this
        // future un-spawnable. Deciding what to run is synchronous and gives the
        // borrow back — hence the `let` before the `match`, which ends the
        // borrow at the semicolon rather than at the end of the match — and only
        // owned values cross the awaits.
        //
        // A loop, because the third answer is "wait": a checkout with a person
        // in it is planned again once they are done, not refused (see
        // [`SessionPlan::Queue`]). The budget is the caller's own idle ceiling,
        // so a queued task cannot outlive the deadline its requester set for it.
        let queue_deadline = tokio::time::Instant::now() + self.queue_budget(options.timeout_ms);
        let queue_abort = options.abort.clone();
        let opened = loop {
            let plan = self.session_for(&options, class)?;
            match plan {
                SessionPlan::Reuse(opened) => break opened,
                SessionPlan::Launch(spec) => break self.launch(*spec).await?,
                SessionPlan::Queue(cwd) => {
                    self.await_checkout_release(&cwd, &queue_abort, queue_deadline)
                        .await?;
                }
            }
        };
        if let Some(pinned) = &opened.harness_session_id {
            // A reused session's transcript already exists, so the fresh-session
            // rules — ignore what is already there, discount anything older than
            // now — would rule it out and report that the harness never started.
            // Resume from its current end instead.
            tailer = if opened.reused {
                tailer.resuming(pinned.clone())
            } else {
                tailer.expecting(pinned.clone())
            };
        }
        let gh_repo_is_set = opened.gh_repo_is_set;
        let id = opened.id;
        // Tell the daemon which session serves this task, as soon as it exists.
        // A screen subscription names a task, so until this lands there is
        // nothing for it to resolve — and the whole point is to watch the task
        // while it runs, not to learn where it ran once it is over.
        if let Some(report) = options.on_session {
            report(id.clone());
        }

        // Latch the tailer *before* typing. Locating is lazy, and a resumed tail
        // takes its start offset from the file's length at the moment it
        // latches: leave that until after the prompt is sent and a fast harness
        // has already written its answer past the mark, so the turn waits out
        // its budget for lines it has just skipped.
        //
        // Whatever this locates must be recorded, exactly as the polling loop
        // does. Dropping it costs the session its harness id — and an id-less
        // session cannot be resumed, so the *next* turn on it falls back to
        // fresh-session discovery, finds its own transcript in the
        // ignore-what-was-here-before set, and dies reporting that the harness
        // never started. Intermittently, depending on whether the harness had
        // written its first record by this point.
        let pre = tailer.poll();
        if let Some(located) = &pre.located {
            self.sessions
                .record_session_id(&id, located.harness_session_id.clone());
        }

        // Type the prompt only after the tailer is latched, so nothing the
        // harness writes in response can be missed. This waits for the harness
        // to be listening and for the paste to land before pressing Enter — a
        // return sent in the same burst is absorbed by the paste, which leaves
        // the prompt sitting in the composer, complete and unsent.
        if let Err(err) =
            super::super::pty::inject_prompt(&self.sessions, &id, &options.prompt).await
        {
            self.finish_turn(&id, class, false);
            return Err(err);
        }

        // Register the peer's stdin channel now that the session exists: a
        // `TaskFrameKind::Input` for this task reaches nothing until this runs,
        // because nobody ever calls the registration callback. The background
        // task outlives this function (it is spawned, not awaited) and drains
        // itself once the sender side drops — when the dispatcher forgets this
        // task's `on_stdin` registration at turn end.
        if let Some(register) = options.on_stdin {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            register(tx);
            let sessions = self.sessions.clone();
            let stdin_id = id.clone();
            tokio::spawn(async move {
                while let Some(text) = rx.recv().await {
                    // The same path a human's keystrokes take: bracket, wait for
                    // it to land, then submit. A steering message arriving mid-turn
                    // is exactly the case `inject_prompt` exists for — it already
                    // waits out a busy composer rather than assuming one is ready.
                    if super::super::pty::inject_prompt(&sessions, &stdin_id, &text)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }

        // Only the plain data and the (owned) callback cross into the polling
        // loop: `RunTaskOptions` is `Send` but not `Sync`, so holding a borrow
        // of it across an await would make this future un-spawnable.
        let abort = options.abort.clone();
        let on_event = options.on_event;
        let timeout_ms = options.timeout_ms;
        // Kept for the hand-back turn: if an operator takes this session
        // mid-flight, what they hand back has to be finished against the task
        // that was asked for, and this is the only copy of it that survives the
        // borrow rules above.
        let instruction = options.prompt.clone();
        let outcome = self
            .await_turn(
                &id,
                TurnSpec {
                    provider,
                    gh_repo_is_set,
                    timeout_ms,
                    instruction,
                },
                tailer,
                abort,
                on_event,
            )
            .await;
        self.finish_turn(&id, class, outcome.is_ok());
        outcome
    }

    /// Settle executor ownership without destroying the session.
    ///
    /// A bounded task session used to die with its reply. It is *retained*
    /// instead: the PTY stays up, so the pane keeps showing the work the task
    /// actually did rather than falling through to the transcript the moment it
    /// finishes — which read as a finished task vanishing.
    ///
    /// Retention is not takeover. The session stays the orchestrator's, because
    /// [`checkout_writer`](Self::checkout_writer) counts any user-held session
    /// in a directory as the writer holding that checkout: marking these `User`
    /// would make the first task to finish in a workspace queue every task
    /// behind it until their budgets ran out. It is instead flagged retained,
    /// which `try_claim` refuses, so no later task lands in a transcript that
    /// has already answered someone else.
    ///
    /// Operator takeover keeps its own path: a session someone took is theirs,
    /// and was never a candidate for closing.
    fn finish_turn(&self, id: &str, class: SessionClass, settled: bool) {
        let control = self.sessions.control(id);
        let running = self
            .sessions
            .row(id)
            .is_some_and(|row| row.state.is_running());
        // Only a turn that actually answered is worth keeping. A bounded turn
        // that failed never got its prompt in — the harness is wedged on a modal
        // the injection could not clear — so retaining it would leave a stuck
        // process standing with nothing on its screen worth reading, which is
        // the leak the close was there to prevent.
        let retain =
            settled && class == SessionClass::Bounded && control != Some(SessionControl::User);
        // A retained session can serve the operator a later turn, so its mapper
        // state outlives the task the same way a taken session's does.
        if !retain && !retains_workspace_context(class, control, running) {
            self.workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .remove(id);
        }
        let bounded_for_the_orchestrator =
            class == SessionClass::Bounded && control != Some(SessionControl::User);
        if retain {
            self.sessions.retain(id);
            // Freed as well as retained: the turn is over, and a session left
            // busy would read as working forever in every surface that asks.
            self.sessions.release(id);
        } else if bounded_for_the_orchestrator {
            // A bounded turn that never answered. There is nothing to keep, and
            // the harness is most likely still sitting on whatever stopped the
            // injection — so this stays a close, exactly as it was before
            // retention existed. Handing it over instead would strand a wedged
            // process *and* make it hold the checkout against the tasks behind
            // it, which is the failure retention is careful to avoid.
            self.sessions.close(id);
        } else {
            // Free it for the operator or this peer's next turn. Released on
            // error too: a failed turn left busy can never be reused again.
            if settled {
                self.sessions.settle_turn(id);
            } else {
                // A failed reusable turn can leave the harness blocked on the
                // very prompt that caused the failure. The daemon is about to
                // drop its task-to-session binding, so hand it to the operator
                // unconditionally: attention sampling is asynchronous and may
                // not have latched a brand-new cue yet. User-held sessions have
                // a direct rail row and cannot be reclaimed behind their back.
                self.sessions.set_control(id, SessionControl::User);
                self.sessions.release(id);
            }
        }
    }

    /// Interrupt a running turn and take its session out of service.
    ///
    /// The interrupt goes first and is a real `Ctrl-C`, exactly what an operator
    /// would press: harnesses handle it as "stop what you are doing" and unwind
    /// their tool calls, where killing the process leaves whatever it was
    /// mid-write half-written.
    ///
    /// Then the session is closed rather than released, and deliberately so even
    /// for an unbound conversation. A harness that has just been interrupted is
    /// not *known* to be idle — the interrupt is a request, not a fence — and
    /// `claim_idle` only skips sessions that are no longer running. Handing a
    /// conversation a fresh session costs it continuity; handing the next task a
    /// harness still finishing the last one interleaves two prompts into one
    /// composer, which is the failure that produces confidently wrong answers
    /// rather than an error.
    pub(super) fn stop_turn(&self, id: &str) {
        let stopped = self.sessions.stop_if_orchestrator(id);
        retire_stopped_workspace_context(
            &mut self
                .workspace_context
                .lock()
                .expect("workspace context lock poisoned"),
            id,
            stopped,
        );
    }
}

pub(super) fn retains_workspace_context(
    class: SessionClass,
    control: Option<SessionControl>,
    running: bool,
) -> bool {
    running && (class == SessionClass::Unbound || control == Some(SessionControl::User))
}

/// Forget mapper state only when the orchestrator actually won the stop race.
pub(super) fn retire_stopped_workspace_context(
    context: &mut HashMap<String, WorkspaceContext>,
    id: &str,
    stopped: bool,
) {
    if stopped {
        context.remove(id);
    }
}

/// The transcript dialect a provider writes, if this executor can read it.
pub fn agent_kind(provider: HarnessProvider) -> Option<SessionAgentKind> {
    match provider {
        HarnessProvider::Claude => Some(SessionAgentKind::Claude),
        HarnessProvider::Codex => Some(SessionAgentKind::Codex),
        // No flat transcript to tail, so no way to know a turn ended.
        HarnessProvider::Opencode | HarnessProvider::Openhuman => None,
    }
}
