//! [`PtySessionExecutor`] — runs a delegated task inside a live, watchable
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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::session_history::SessionAgentKind;
use medulla::sessions::{SessionClass, TurnStream};
use medulla::tinyplace::HarnessProvider;
use medulla::wrapper::tail::SessionTailer;

use super::pty::{HarnessControl, LaunchSpec, PtyManager};

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
        }
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
    pub(super) async fn run_for_test(
        self,
        options: RunTaskOptions,
    ) -> Result<RunTaskResult, String> {
        self.run(options).await
    }

    /// The live session manager, for assertions about what a run left behind.
    #[cfg(all(test, unix))]
    pub(super) fn sessions_for_test(&self) -> PtyManager {
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
        // borrow back; only the owned [`LaunchSpec`] crosses the await.
        let opened = match self.session_for(&options, class)? {
            SessionPlan::Reuse(opened) => opened,
            SessionPlan::Launch(spec) => self.launch(spec).await?,
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
        if let Err(err) = super::pty::inject_prompt(&self.sessions, &id, &options.prompt).await {
            self.finish_turn(&id, class);
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
                    if super::pty::inject_prompt(&sessions, &stdin_id, &text)
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
        let outcome = self
            .await_turn(&id, provider, tailer, abort, on_event, timeout_ms)
            .await;
        self.finish_turn(&id, class);
        outcome
    }

    /// Settle executor ownership without destroying a session the operator took.
    ///
    /// Bounded task sessions normally die with their reply. Operator takeover
    /// changes that lifetime: the PTY is now an interactive workspace, so the
    /// executor may release its busy claim but must not close the process.
    fn finish_turn(&self, id: &str, class: SessionClass) {
        if class == SessionClass::Bounded && self.sessions.control(id) != Some(HarnessControl::User)
        {
            self.sessions.close(id);
        } else {
            // Free it for the operator or this peer's next turn. Released on
            // error too: a failed turn left busy can never be reused again.
            self.sessions.release(id);
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
        let _ = self.sessions.write(id, &[0x03]);
        self.sessions.close(id);
    }

    /// Decide which session serves this task: reuse an idle one, or launch.
    ///
    /// Synchronous, and returns a plan rather than a session, because the launch
    /// itself must not happen here — see [`PtySessionExecutor::launch`].
    fn session_for(
        &self,
        options: &RunTaskOptions,
        class: SessionClass,
    ) -> Result<SessionPlan, String> {
        // Exclusivity, and it comes first. A workspace an operator is working in
        // is not available to the orchestrator at all — not "reuse nothing in
        // it", but "start nothing in it". Checked ahead of the reuse branch
        // below on purpose: a folder with a person in it is not shared, however
        // idle some other harness sitting in it happens to look.
        //
        // Refused with the shared prefix rather than silently routed around, so
        // the hub can settle it as `RunError::Held` and the orchestrator is told
        // *why* it cannot work here — a task that vanishes into a retry with no
        // reason is the failure this replaces.
        if let Some(held) = self.sessions.operator_hold(&options.cwd) {
            return Err(format!(
                "{}: an operator is working in {} ({} session {})",
                medulla::daemon::HARNESS_HELD_PREFIX,
                options.cwd,
                held.provider.as_str(),
                held.id,
            ));
        }
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
                }));
            }
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
        let (env, extra_args) = self.spawn_env(options)?;
        Ok(SessionPlan::Launch(LaunchSpec {
            provider: options.provider,
            bin: medulla::tinyplace::env::provider_bin(options.provider, &self.env),
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
            control: HarnessControl::Orchestrator,
            user_spawned: false,
        }))
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
        let sessions = self.sessions.clone();
        let id = tokio::task::spawn_blocking(move || sessions.open(spec))
            .await
            .map_err(|err| format!("pty launch did not complete: {err}"))??;
        let harness_session_id = self.sessions.row(&id).and_then(|row| row.session_id);
        Ok(OpenedSession {
            id,
            harness_session_id,
            reused: false,
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
    fn spawn_env(
        &self,
        options: &RunTaskOptions,
    ) -> Result<(HashMap<String, String>, Vec<String>), String> {
        let mut env = options.env.clone();
        let mut extra_args = options.extra_args.clone();
        // Commits made in a watched PTY session are just as much Medulla's work
        // as headless ones, so this path carries the same attribution.
        let attribution_env = medulla::attribution::attribution_env(options.attribution, &env);
        env.extend(attribution_env);
        extra_args.extend(medulla::attribution::attribution_args(
            options.provider,
            options.attribution,
        ));
        // OpenRouter-bound runs are re-pointed at Medulla's loopback attribution
        // proxy, and the real key is scrubbed from `env` here, before any of it
        // reaches the child. A no-op for every other endpoint.
        let mut router = options.router.clone();
        medulla::inference_proxy::route_spawn(options.provider, &mut router, &mut env)?;
        if let Some(router) = &router {
            let injection = medulla::tinyplace::env::router_env(options.provider, router);
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
        Ok((env, extra_args))
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
        provider: HarnessProvider,
        mut tailer: SessionTailer,
        abort: medulla::daemon::providers::Abort,
        mut on_event: Option<medulla::daemon::providers::OnEvent>,
        timeout_ms: u64,
    ) -> Result<RunTaskResult, String> {
        let mut stream = TurnStream::new(provider);
        let started = tokio::time::Instant::now();
        let mut last_line_at = medulla::clock::now_millis();

        loop {
            // Taking control is an ownership transfer, not merely a display
            // preference. Yield before processing aborts or transcript output
            // so the executor cannot send Ctrl-C, report a stale completion, or
            // later close the PTY underneath the operator.
            if self.sessions.control(id) == Some(HarnessControl::User) {
                return Err(format!(
                    "{}: operator took control of the {} session",
                    medulla::daemon::HARNESS_HELD_PREFIX,
                    provider.as_str()
                ));
            }
            if abort.is_aborted() {
                // A real interrupt, not a kill: Ctrl-C reaches the harness the
                // same way the operator's would, and the session survives it.
                let _ = self.sessions.write(id, &[0x03]);
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

            let poll = tailer.poll();
            // Codex cannot be told its id, so it is learned from the rollout the
            // first time the tailer locates one.
            if let Some(located) = &poll.located {
                self.sessions
                    .record_session_id(id, located.harness_session_id.clone());
            }
            for line in poll.lines {
                last_line_at = medulla::clock::now_millis();
                let fold = stream.observe(&line.text);
                // The peer watches its task through these. Dropping them would
                // leave it with an ack, silence, then a reply — which is what
                // this executor used to do.
                if let Some(callback) = on_event.as_mut() {
                    for event in &fold.events {
                        callback(event);
                    }
                }
                if let Some(reply) = fold.reply {
                    return Ok(RunTaskResult {
                        provider,
                        reply,
                        events: stream.events(),
                        usage: stream.usage(),
                        session_id: self.sessions.row(id).and_then(|row| row.session_id),
                    });
                }
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

/// The transcript dialect a provider writes, if this executor can read it.
pub fn agent_kind(provider: HarnessProvider) -> Option<SessionAgentKind> {
    match provider {
        HarnessProvider::Claude => Some(SessionAgentKind::Claude),
        HarnessProvider::Codex => Some(SessionAgentKind::Codex),
        // No flat transcript to tail, so no way to know a turn ended.
        HarnessProvider::Opencode => None,
    }
}

mod types;
pub use types::PtySessionExecutor;
use types::{OpenedSession, SessionPlan};
