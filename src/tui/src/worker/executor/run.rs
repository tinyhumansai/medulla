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
use medulla::sessions::{SessionClass, TurnStream};
use medulla::wrapper::tail::SessionTailer;

use super::super::pty::{LaunchSpec, PtyManager, SessionControl};
use super::types::{OpenedSession, PtySessionExecutor, SessionPlan, TurnSpec, WorkspaceContext};

/// How often the transcript is polled while a turn runs.
///
/// Fast enough that a short turn settles promptly, slow enough that a long one
/// costs almost nothing. The transcript is a file on local disk, so this is a
/// stat plus a short read.
const POLL: Duration = Duration::from_millis(150);

/// How long to keep looking for a session's transcript before giving up.
///
/// A harness writes its first record only once it has started work, which on a
/// cold start can take a few seconds.
const LOCATE_BUDGET: Duration = Duration::from_secs(30);

/// Silence that settles a turn whose completion record carried no stated reason.
///
/// Only reachable for the ~0.08% of claude records with no `stop_reason`; the
/// watcher refuses to stall while a tool call is outstanding, so a long build is
/// never mistaken for a finished turn.
const STALL_BUDGET_MS: i64 = 120_000;

/// How long to wait for the rest of a terminal message before replying with what
/// arrived. The blocks of one message are written in a single burst, so this only
/// has to outlast that write — it is a safety net, not the normal path.
const SETTLE_GRACE_MS: i64 = 1_500;

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
            // The half of the decision that blocks runs on the blocking pool,
            // exactly as the launch below does. Reuse waits out the previous
            // turn's completion-chime grace with a `std::thread::sleep` of up
            // to 300ms, and the checkout check canonicalizes both paths; inline,
            // that parked a tokio worker per dispatch, and a burst of task
            // frames could park most of the runtime — taking the inbox drain
            // and every screen sampler down with it, since they share one.
            let probe = self
                .probe_session(class, options.conversation.clone(), provider, &options.cwd)
                .await?;
            let plan = self.session_for(&options, probe)?;
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
    fn stop_turn(&self, id: &str) {
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

    /// Start a fresh harness on the blocking pool.
    ///
    /// [`PtyManager::open`] forks, execs, and may back off while a pty frees up
    /// — all of it blocking. Calling it inline parked a tokio worker for up to
    /// half a second per launch, and a burst of task frames could park most of
    /// the runtime, taking the inbox drain and every screen sampler down with
    /// it since they share one. So the launch goes to the blocking pool, which
    /// is what it is for.
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
    pub(super) fn spawn_env(
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
    async fn await_turn(
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

/// Retain mapper state only while the PTY can serve a later turn.
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
