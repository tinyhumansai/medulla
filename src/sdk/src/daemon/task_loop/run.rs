//! Executing one delegated task: slots, status forwarding, fallback.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::harness_work::{WorkFold, WorkSnapshot};
use crate::protocol::{HarnessEventKind, TaskFrame, TaskFrameKind};

use super::super::mappers;
use super::super::providers::{Abort, RunTaskOptions};
use super::super::status::{status_detail, work_detail};
use super::super::types::{
    DaemonRuntime, FrameAttachments, RunningTask, CAPACITY_REJECTION_PREFIX,
    SESSION_HELD_STATUS_PREFIX, SESSION_RESUMED_STATUS_PREFIX,
};

/// What a task waiting for a harness slot reports while it waits.
const QUEUED_STATUS: &str = "queued for a harness slot";

/// Whether a status detail is one of the two control markers the requester's
/// watchdog reads (see [`SESSION_HELD_STATUS_PREFIX`]).
fn is_control_marker(detail: &str) -> bool {
    detail.starts_with(SESSION_HELD_STATUS_PREFIX)
        || detail.starts_with(SESSION_RESUMED_STATUS_PREFIX)
}

/// What a running task reports through a stretch with no harness events —
/// a single long tool call, typically.
const HEARTBEAT_STATUS: &str = "still working";

impl DaemonRuntime {
    /// Admit, execute, and reply to a `task` frame, forwarding throttled status.
    ///
    /// `sender_device_local` is the receiver's own verdict on `from`, carried in
    /// from the transport (see
    /// [`handle_message_from`](crate::daemon::DaemonRuntime::handle_message_from)).
    /// It is the only thing that may let the frame's `workflow_node` marker
    /// grant [`RunTaskOrigin::Workflow`] — the marker alone is caller-controlled
    /// JSON and cannot be trusted from a remote peer.
    pub(super) async fn handle_task(
        &self,
        from: String,
        frame: TaskFrame,
        sender_device_local: bool,
    ) {
        let correlation = frame.correlation_id.clone();
        let custom_harness = match frame.custom_harness.as_deref() {
            Some(id) => match self
                .inner
                .config
                .custom_harnesses
                .iter()
                .find(|harness| harness.id == id)
            {
                Some(harness) => Some(harness.clone()),
                None => {
                    self.reply(
                        &from,
                        TaskFrameKind::Error,
                        &frame.task_id,
                        &format!("custom harness \"{id}\" is not configured on this host"),
                        correlation.as_deref(),
                        None,
                    )
                    .await;
                    return;
                }
            },
            None if frame.provider.is_none() => self
                .inner
                .config
                .custom_harnesses
                .iter()
                .find(|harness| harness.default && harness.key_present(&self.inner.config.env))
                .cloned(),
            None => None,
        };
        let requested_provider = custom_harness
            .as_ref()
            .map(|harness| harness.base_harness)
            .or(frame.provider);
        let provider = match self.select_provider(requested_provider) {
            Some(provider) => provider,
            None => {
                let offered = self
                    .inner
                    .config
                    .providers
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let offered = if offered.is_empty() {
                    "(none)".to_string()
                } else {
                    offered
                };
                let requested = requested_provider
                    .map(|p| format!(" for requested \"{}\"", p.as_str()))
                    .unwrap_or_default();
                self.reply(
                    &from,
                    TaskFrameKind::Error,
                    &frame.task_id,
                    &format!("no available provider{requested}; daemon offers: {offered}"),
                    correlation.as_deref(),
                    None,
                )
                .await;
                return;
            }
        };

        // The flavor the sender asked for, checked against the provider that was
        // actually selected. A frame that named `codex-server` but landed on a
        // host offering only `claude` is refused rather than run on the CLI: the
        // sender chose a flavor, and a silent substitution is the one outcome
        // that cannot be noticed from the other end.
        let transport = frame.transport.unwrap_or_default();
        if !transport.supported_by(provider) {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!(
                    "requested harness flavor \"{}\" cannot run on \"{}\"",
                    transport.as_str(),
                    provider.as_str()
                ),
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        }

        let mut run_env = self.inner.config.env.clone();
        if let Some(harness) = &custom_harness {
            run_env.extend(harness.harness_env());
        }
        let run_env =
            super::with_tool_mode_at_depth(run_env, frame.tool_mode.as_deref(), frame.fleet_depth);
        // Neither ACP nor the app-server has a follow-up-input operation today.
        // Record the effective transport before acknowledging the task so an
        // input frame racing the harness startup is rejected instead of buffered
        // forever.
        let accepts_stdin = super::super::providers::supports_stdin(provider)
            && transport == crate::protocol::HarnessTransport::Cli
            && !run_env
                .get(super::super::providers::HARNESS_PROTOCOL_ENV)
                .is_some_and(|value| value.eq_ignore_ascii_case("acp"));

        // Admission is RAII: the guard releases the slot on every exit from this
        // function, including an unwind, and carries the `running` record and
        // controller registration with it.
        let Some(mut admission) = self.admit() else {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!(
                    "{CAPACITY_REJECTION_PREFIX} ({} pending tasks); retry later",
                    self.inner.config.max_pending
                ),
                correlation.as_deref(),
                Some(provider),
            )
            .await;
            return;
        };

        let key = Self::task_key(&from, &frame.task_id);
        // An active duplicate (same sender + taskId) must not clobber the record.
        if self.inner.running.lock().unwrap().contains_key(&key) {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!("task {} is already running for this sender", frame.task_id),
                correlation.as_deref(),
                Some(provider),
            )
            .await;
            return;
        }
        // Register BEFORE acking so a racing `input` frame finds the record.
        let abort = Abort::new();
        self.inner.running.lock().unwrap().insert(
            key.clone(),
            RunningTask {
                provider,
                accepts_stdin,
                abort: abort.clone(),
                correlation_id: correlation.clone(),
                stdin: None,
                pending_input: Vec::new(),
                session_id: None,
            },
        );
        admission.attach_task(key.clone());
        admission.attach_controller(self.register_controller(abort.clone()));

        self.reply(
            &from,
            TaskFrameKind::Ack,
            &frame.task_id,
            "task accepted",
            correlation.as_deref(),
            Some(provider),
        )
        .await;

        self.log(&format!("task {} → {}", frame.task_id, provider.as_str()));

        // Slot-limited execution (FIFO via the semaphore).
        //
        // The wait is neither silent nor uninterruptible. Silent, it looks
        // exactly like a crashed worker to the requester's no-progress
        // watchdog: the ack satisfies the first-sign-of-life window and then
        // nothing is sent until a slot frees, which with `max_pending` at 16 and
        // `concurrency` at 2 is routinely longer than the watchdog allows. And
        // an abort that arrives while queued has to be honoured here, or the
        // task acquires its slot later and runs work nobody is waiting for.
        let heartbeat = self.heartbeat_interval();
        let permit = {
            let mut ticks = tokio::time::interval(heartbeat);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticks.tick().await; // the first tick completes immediately
            loop {
                tokio::select! {
                    biased;
                    // `Semaphore::acquire` is cancel-safe: losing this branch to
                    // another arm never consumes a permit.
                    permit = self.inner.slots.acquire() => {
                        break permit.expect("semaphore is never closed");
                    }
                    _ = abort.cancelled() => {
                        self.reply(
                            &from,
                            TaskFrameKind::Error,
                            &frame.task_id,
                            "task aborted while queued for a harness slot",
                            correlation.as_deref(),
                            Some(provider),
                        )
                        .await;
                        self.log(&format!("task {} ⨯ aborted while queued", frame.task_id));
                        return;
                    }
                    _ = ticks.tick() => {
                        self.reply(
                            &from,
                            TaskFrameKind::Status,
                            &frame.task_id,
                            QUEUED_STATUS,
                            correlation.as_deref(),
                            Some(provider),
                        )
                        .await;
                    }
                }
            }
        };

        // What the harness is working on, folded from its own stream. Shared
        // with the reply below so the terminal frame carries the final picture
        // even when the last change was throttled away.
        let work = Arc::new(Mutex::new(WorkFold::new()));

        // Status frames: onEvent (sync) throttles + forwards details over a
        // channel; a consumer sends them in order before the final reply. Each
        // frame carries the current work snapshot, so a dropped update is
        // corrected by the next one rather than lost.
        let (status_tx, mut status_rx) =
            mpsc::unbounded_channel::<(String, Option<WorkSnapshot>)>();
        let pending_thinking = Arc::new(Mutex::new(None));
        let final_status_tx = status_tx.clone();
        let on_event = {
            let now = self.inner.now.clone();
            let throttle = self.inner.config.status_throttle_ms;
            let mut last_status_at: i64 = i64::MIN;
            let status_tx = status_tx.clone();
            let work = work.clone();
            let pending_thinking = pending_thinking.clone();
            Box::new(move |semantic: &mappers::HarnessSemanticEvent| {
                // Fold first, unconditionally: throttling governs how often the
                // peer is told, never what this worker knows.
                let mut fold = work.lock().expect("work fold lock");
                let changed = fold.apply(
                    &semantic.event.kind,
                    &semantic.event.payload,
                    semantic.timestamp_ms,
                );
                // A todo write or a sub-agent spawn has no status wording of its
                // own; describing the work is what makes it visible at all.
                let detail = match status_detail(&semantic.event) {
                    Some(detail) => detail,
                    None if changed => match work_detail(fold.snapshot()) {
                        Some(detail) => detail,
                        None => return,
                    },
                    None => return,
                };
                let snapshot = fold.snapshot().clone();
                drop(fold);
                let current = now();
                let is_thinking =
                    matches!(semantic.event.decoded(), HarnessEventKind::AgentThinking(_));
                // A non-thinking transition ends the reasoning stream. Flush
                // its newest cumulative snapshot first so a quick turn cannot
                // leave the copilot displaying only the first fragment.
                if !is_thinking {
                    if let Some(pending) = pending_thinking.lock().unwrap().take() {
                        let _ = status_tx.send(pending);
                        last_status_at = current;
                    }
                }
                // Tool lifecycle events are state changes, not periodic
                // chatter. In particular ACP may announce a generic Terminal
                // call and supply its command in the next patch; throttling
                // either frame would leave the copilot generic or running.
                let changes_tool = matches!(
                    semantic.event.decoded(),
                    HarnessEventKind::ToolCall(_) | HarnessEventKind::ToolResult(_)
                );
                // Control changes are state transitions too, and the only ones
                // the *requester* acts on: the hold marker is what pauses its
                // no-progress watchdog, and the hand-back marker is what
                // resumes it. Throttled away, a hold that began a second after
                // the last tool call would never be announced, and the hub
                // would reap a task a person is sitting in.
                let changes_control = is_control_marker(&detail);
                if !changes_tool
                    && !changes_control
                    && current.saturating_sub(last_status_at) < throttle
                {
                    if is_thinking {
                        *pending_thinking.lock().unwrap() = Some((detail, Some(snapshot)));
                    }
                    return;
                }
                last_status_at = current;
                if is_thinking {
                    pending_thinking.lock().unwrap().take();
                }
                let _ = status_tx.send((detail, Some(snapshot)));
            }) as Box<dyn FnMut(&mappers::HarnessSemanticEvent) + Send>
        };
        drop(status_tx);

        let on_stdin = {
            let this = self.clone();
            let key = key.clone();
            Box::new(move |tx: mpsc::UnboundedSender<String>| {
                let mut running = this.inner.running.lock().unwrap();
                if let Some(record) = running.get_mut(&key) {
                    for buffered in record.pending_input.drain(..) {
                        let _ = tx.send(buffered);
                    }
                    record.stdin = Some(tx);
                }
            }) as Box<dyn FnOnce(mpsc::UnboundedSender<String>) + Send>
        };

        // The conversation this task belongs to, if it named one. Scoped to the
        // *authenticated* sender rather than taken bare from the frame: the
        // frame body cannot be trusted to name its own author, so without this
        // one peer could name another's conversation and resume into a session
        // holding someone else's context.
        let session_key = frame.conversation.as_deref().map(|conversation| {
            crate::sessions::SessionKey::new(format!("{from}/{conversation}"), provider)
        });
        // Held for the rest of this turn, past the `plan()` below and through
        // `run_task` — including the `on_session` callback that records the
        // binding once the harness opens one. Without it, two frames naming
        // the same conversation could both `plan()` before either recorded a
        // session: the first pair would each open their own session and race
        // to bind it (whichever `on_session` fires last silently wins, and the
        // other harness session is orphaned); a later pair could both resume
        // the *same* now-bound session concurrently, interleaving two turns on
        // one harness. `acquire_turn` is a no-op (`None`) for anything but
        // `Unbound`, so a task frame with no named conversation never
        // serializes against unrelated work.
        let turn_guard = match &session_key {
            Some(key) => {
                self.inner
                    .sessions
                    .acquire_turn(key, crate::sessions::SessionClass::Unbound)
                    .await
            }
            None => None,
        };
        // An `Unbound` class because these turns are a conversation, not
        // discrete work — that is the whole of what the sender opted into. The
        // registry still declines to resume a provider that cannot
        // (`opencode`), which costs continuity but never correctness.
        let plan = session_key.as_ref().map(|key| {
            self.inner
                .sessions
                .plan(key, crate::sessions::SessionClass::Unbound)
        });
        let resume_session_id = plan
            .as_ref()
            .and_then(|plan| plan.resume_session_id.clone());
        let workspace_context = plan
            .as_ref()
            .map(|plan| plan.workspace_context.clone())
            .unwrap_or_default();
        let bound_workspace_context = Arc::new(Mutex::new(workspace_context.clone()));
        {
            let mut fold = work.lock().expect("work fold lock");
            let _ = fold.apply(
                crate::harness_work::kinds::SESSION_INFO,
                &serde_json::json!({
                    "cwd": workspace_context.cwd.clone(),
                    "branch": workspace_context.branch.clone(),
                    "pull_request": workspace_context.pull_request.clone(),
                }),
                0,
            );
        }

        // Reported as soon as the executor opens a session, not with the
        // result: the point of knowing it is to watch the task while it runs.
        let on_session = {
            let this = self.clone();
            let key = key.clone();
            let session_key = session_key.clone();
            let bound_workspace_context = bound_workspace_context.clone();
            Box::new(move |session_id: String| {
                // Bind before the turn finishes. A turn that opens a session and
                // then times out has still moved the conversation on, and the
                // next instruction has to resume *that* session rather than
                // starting a third one the operator never sees.
                if let Some(session_key) = &session_key {
                    this.inner.sessions.record_with_workspace_context(
                        session_key,
                        session_id.clone(),
                        bound_workspace_context.lock().unwrap().clone(),
                    );
                }
                this.record_task_session(&key, session_id);
            }) as Box<dyn FnOnce(String) + Send>
        };

        let options = RunTaskOptions {
            // `workflowNode` is trusted only from a device-local sender. The
            // workflow plane is a daemon-local dispatch: the workflow host runs
            // each `agent` node over an in-process loopback bridge, so the only
            // frames that legitimately carry the marker arrive from a peer this
            // daemon itself serves. A remote authenticated peer can write the
            // key into its own JSON, so there it must be read as ordinary
            // delegated work — otherwise the marker would let any such peer
            // mint `RunTaskOrigin::Workflow`, which is what suppresses the
            // embedded harness's approval prompts.
            origin: if sender_device_local && frame.workflow_node {
                crate::daemon::providers::RunTaskOrigin::Workflow
            } else {
                crate::daemon::providers::RunTaskOrigin::DelegatedTask
            },
            // The *authenticated* sender, never anything from the frame body: a
            // frame cannot be trusted to name its own author. This says *whose*
            // the run is; `session_class` separately says whether it may share
            // context with the sender's other work.
            conversation: from.clone(),
            // A task frame is discrete work, so it gets its own session and can
            // see nothing of the sender's other tasks — `Bounded` even when the
            // frame names a conversation, because this field only chooses
            // between "a fresh session" and "whichever of the sender's sessions
            // is idle", and the latter cannot tell two of that sender's
            // conversations apart. Continuity, when a sender asks for it,
            // arrives through `resume_session_id` below, which names one
            // specific prior session scoped to `{sender}/{conversation}`.
            session_class: crate::sessions::SessionClass::Bounded,
            resume_session_id,
            workspace_context,
            provider,
            transport,
            prompt: frame.text.clone(),
            cwd: self.inner.config.workspace.clone(),
            // Per-task, like the model and provider hints below. A review turn
            // asks for the restricted workflow tools here, which is the only
            // channel that reaches the MCP server the harness spawns.
            env: run_env,
            timeout_ms: self.inner.config.task_timeout_ms,
            // Per-task model hint (parallels the per-task `provider`): honor the
            // orchestrator's requested model, falling back to the daemon default
            // — but only when the frame named no harness of its own at all.
            // `frame.model` is deliberately `None` (not merely absent) when a
            // caller's paired harness/model resolution named a harness without
            // a model of its own (see `flow_engine::harness_choice`): the model
            // this daemon has pinned is scoped to whatever runs when a frame
            // states no preference, not to whichever harness a frame happened to
            // land on. An explicit `provider`/`custom_harness` — even one that
            // resolves to this daemon's own default harness — must still get
            // that harness's own tool default, not the daemon's pin: pinning
            // `self.inner.config.model` to the default is itself a "no
            // preference stated" default, and a frame that stated one, even the
            // same one, is not that case.
            model: frame
                .model
                .clone()
                .or_else(|| custom_harness.as_ref().map(|harness| harness.model.clone()))
                .or_else(|| {
                    requested_provider
                        .is_none()
                        .then(|| self.inner.config.model.clone())
                        .flatten()
                }),
            agent: self.inner.config.agent.clone(),
            extra_args: self.inner.config.extra_args.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort: abort.clone(),
            attribution: self.inner.config.attribution,
            hooks: self.inner.config.hooks.clone(),
            router: custom_harness
                .as_ref()
                .map(crate::config::CustomHarnessConfig::router)
                .or_else(|| self.inner.config.router.clone()),
            on_event: Some(on_event),
            on_stdin: Some(on_stdin),
            on_session: Some(on_session),
            on_workspace_context: session_key.clone().map(|session_key| {
                let sessions = self.inner.sessions.clone();
                let bound_workspace_context = bound_workspace_context.clone();
                Box::new(move |context: crate::sessions::WorkspaceContext| {
                    *bound_workspace_context.lock().unwrap() = context.clone();
                    sessions.record_workspace_context(&session_key, context);
                }) as crate::daemon::providers::OnWorkspaceContext
            }),
        };

        // Consume status details in order while the task runs, and heartbeat
        // through silence.
        //
        // `on_event` only speaks when the harness does. One long tool call — a
        // repository-wide search, a slow test run — emits no semantic event at
        // all, and minutes of that is indistinguishable from a dead worker to
        // the requester's no-progress watchdog. The throttle above is a rate
        // *cap* on event-driven frames and never produces one of its own, so the
        // floor has to live here: every heartbeat period without a frame, send
        // the current work snapshot unchanged.
        let status_consumer = {
            let this = self.clone();
            let from = from.clone();
            let task_id = frame.task_id.clone();
            let correlation = correlation.clone();
            let work = work.clone();
            tokio::spawn(async move {
                let mut ticks = tokio::time::interval(heartbeat);
                ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                ticks.tick().await; // the first tick completes immediately
                loop {
                    let (detail, snapshot) = tokio::select! {
                        biased;
                        received = status_rx.recv() => match received {
                            Some((detail, snapshot)) => {
                                // A real update restarts the silence clock.
                                ticks.reset();
                                (detail, snapshot)
                            }
                            None => break,
                        },
                        _ = ticks.tick() => (
                            HEARTBEAT_STATUS.to_string(),
                            work.lock().ok().map(|fold| fold.snapshot().clone()),
                        ),
                    };
                    this.reply_with(
                        &from,
                        TaskFrameKind::Status,
                        &task_id,
                        &detail,
                        correlation.as_deref(),
                        Some(provider),
                        FrameAttachments {
                            usage: None,
                            work: snapshot,
                            ..Default::default()
                        },
                    )
                    .await;
                }
            })
        };

        // Hand the consumer to the scheduler before the executor can pin this
        // thread.
        //
        // A task spawned from inside a worker lands in that worker's LIFO slot,
        // which tokio deliberately does not let other workers steal. `run_task`
        // is an executor seam and may block its thread rather than await — a pty
        // write to a child that has stopped draining its stdin does exactly
        // that, for as long as the child stays wedged. The consumer spawned two
        // lines up would then be stranded in the slot of a thread that never
        // comes back, and the heartbeat below it — the *only* frame a silent
        // turn ever sends — would never fire. The requester sees an ack and then
        // nothing at all, its no-progress watchdog never trips because a stalled
        // dispatch and a dead one look identical, and the task hangs until
        // something upstream gives up.
        //
        // Yielding once puts the consumer on a stealable queue and parks it on
        // its own timer, after which any worker can drive it. Costs one
        // scheduler hop per task; buys the liveness guarantee this whole
        // heartbeat exists to provide, without making it depend on every present
        // and future executor being scrupulously non-blocking.
        tokio::task::yield_now().await;

        let result = (self.inner.run_task)(options).await;
        // A turn may finish directly after a burst of thought chunks, without
        // another semantic event to trigger the transition flush above.
        if let Some(pending) = pending_thinking.lock().unwrap().take() {
            let _ = final_status_tx.send(pending);
        }
        drop(final_status_tx);
        // Released only now: the guard must outlive the `on_session` callback
        // above, which is the thing recording the binding the next queued turn
        // will plan against.
        drop(turn_guard);
        // The task future is dropped here, dropping its on_event (and its status
        // sender); the consumer then drains and ends.
        let _ = status_consumer.await;

        match result {
            Ok(run) => {
                // The reply carries the finished picture — the completed todo
                // list, every sub-agent, every file touched — so a caller that
                // only ever reads terminal frames still gets the whole story.
                let final_work = work.lock().expect("work fold lock").snapshot().clone();
                self.reply_with(
                    &from,
                    TaskFrameKind::Reply,
                    &frame.task_id,
                    &run.reply,
                    correlation.as_deref(),
                    Some(provider),
                    FrameAttachments {
                        usage: run.usage,
                        work: Some(final_work),
                        // Which session did the work. Only this process knows —
                        // it opened (or resumed) it — and the caller has no way
                        // to derive it, so a task whose session goes unreported
                        // is one nobody upstream can point at afterwards.
                        session_id: run.session_id.clone(),
                    },
                )
                .await;
                // The captured turn content, as it was read out of the harness's
                // own transcript. Logged next to the send below so "the harness
                // answered" and "the peer was told" stop being one fact: an
                // empty reply here and a failed send there look identical from
                // the outside, and only one of them is the harness's fault.
                self.log(&format!(
                    "task {} ✓ ({} events) · captured {} chars: {}",
                    frame.task_id,
                    run.events,
                    run.reply.chars().count(),
                    crate::logging::preview(&run.reply),
                ));
            }
            Err(message) => {
                self.reply(
                    &from,
                    TaskFrameKind::Error,
                    &frame.task_id,
                    &message,
                    correlation.as_deref(),
                    Some(provider),
                )
                .await;
                self.log(&format!("task {} ✗ {message}", frame.task_id));
            }
        }

        drop(permit);
        // Released only now, after the terminal frame is on the wire: the guard
        // frees the task id as well as the admission slot, and a peer's
        // immediate follow-up task reusing that id must not be admitted before
        // its predecessor's reply lands.
        drop(admission);
    }

    /// Run a plain-text DM through the default provider, replying with raw text.
    pub(in crate::daemon) async fn handle_plain_text(&self, from: String, text: String) {
        let provider = self.inner.config.default_provider;
        if !self.inner.config.providers.contains(&provider) {
            self.send_raw(&from, "No coding agent is available on this daemon.")
                .await;
            return;
        }
        let Some(mut admission) = self.admit() else {
            // Prose for a human reading a DM, not the machine-readable rejection
            // the frame path sends: nothing parses this one.
            self.send_raw(
                &from,
                &format!(
                    "Daemon at capacity ({} pending tasks); retry later.",
                    self.inner.config.max_pending
                ),
            )
            .await;
            return;
        };
        let abort = Abort::new();
        admission.attach_controller(self.register_controller(abort.clone()));

        let permit = self
            .inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");
        self.log(&format!("plaintext DM → {}", provider.as_str()));
        let options = RunTaskOptions {
            origin: crate::daemon::providers::RunTaskOrigin::Conversation,
            conversation: from.clone(),
            // A conversational message continues the sender's session — that is
            // what makes a DM a conversation rather than a series of unrelated
            // one-shots.
            session_class: crate::sessions::SessionClass::Unbound,
            resume_session_id: None,
            workspace_context: Default::default(),
            provider,
            // A plain DM states no flavor; it runs the host's default transport.
            transport: crate::protocol::HarnessTransport::Cli,
            prompt: text,
            cwd: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
            timeout_ms: self.inner.config.task_timeout_ms,
            model: self.inner.config.model.clone(),
            agent: self.inner.config.agent.clone(),
            extra_args: self.inner.config.extra_args.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort,
            router: self.inner.config.router.clone(),
            attribution: self.inner.config.attribution,
            hooks: self.inner.config.hooks.clone(),
            on_event: None,
            on_stdin: None,
            on_session: None,
            on_workspace_context: None,
        };
        let result = (self.inner.run_task)(options).await;
        match result {
            Ok(run) => self.send_raw(&from, &run.reply).await,
            Err(message) => {
                self.send_raw(&from, &format!("Task failed: {message}"))
                    .await
            }
        }
        drop(permit);
        drop(admission);
    }
}
