//! Turn execution: the bounded/unbound split, the interactive and one-shot
//! transports, and the binding capture that gives an unbound session continuity.
//!
//! The one rule that shapes everything here: a **bounded** turn is a pure
//! function of its prompt — nothing to resume, nothing to bind, no
//! serialization, and the session is gone when the reply is sent. An **unbound**
//! turn is the opposite on every count.

use std::sync::{Arc, Mutex};

use crate::daemon::providers::{Abort, RunTaskOptions};

use super::super::interactive::{InteractiveSession, InteractiveSpec, StreamEvent};
use super::super::registry::{SessionIdentity, TurnPlan};
use super::super::routing::{route_transport, Transport};
use super::super::types::{SessionClass, SessionPhase, TurnOrigin, TurnOutcome, TurnRequest};
use super::{push_line, SessionManager, TranscriptRole};

impl SessionManager {
    /// Run one folded turn to completion.
    ///
    /// This is the daemon-facing entry: hand it whatever
    /// [`fold`](super::super::input::fold) produced and it routes the turn by
    /// class, opens or reuses a session as needed, and returns the outcome.
    pub async fn run_turn(&self, request: TurnRequest) -> Result<TurnOutcome, String> {
        match request.class {
            SessionClass::Bounded => self.run_bounded(request).await,
            SessionClass::Unbound => self.run_unbound(request).await,
        }
    }

    /// Submit an operator-typed turn into an already-open session.
    ///
    /// Rejects a session that is terminal or already mid-turn, rather than
    /// queueing — the operator should see why nothing happened.
    pub async fn submit(&self, id: &str, text: &str) -> Result<TurnOutcome, String> {
        let record = self.record(id).ok_or("no such session")?;
        if record.phase.is_terminal() {
            return Err(format!("session {id} is {}", record.phase));
        }
        if !record.phase.accepts_turn() {
            return Err(format!("session {id} is busy ({})", record.phase));
        }
        let request = TurnRequest {
            key: record.key,
            class: record.class,
            text: text.to_string(),
            origin: TurnOrigin::Operator,
            model: None,
        };
        match record.class {
            SessionClass::Unbound => self.run_turn(request).await,
            // A bounded turn dispatched from a *frame* leaves no record behind,
            // so `run_turn` deliberately does no bookkeeping. But an
            // operator-opened bounded session has a row on screen, and a turn
            // whose prompt and answer never appear in it reads as a turn that
            // silently did nothing. Record it against the session, then close
            // it — which is exactly what "one turn, then gone" promises.
            SessionClass::Bounded => {
                let abort = self.begin_turn(id, &request);
                let outcome = self
                    .run_one_shot(&request, None, Default::default(), abort)
                    .await;
                self.end_turn(id, &outcome);
                self.spend_bounded(id);
                outcome
            }
        }
    }

    /// Retire an operator-opened bounded session after its single turn.
    fn spend_bounded(&self, id: &str) {
        let now = self.now();
        {
            let mut sessions = self.inner.sessions.lock().unwrap();
            if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
                entry.record.phase = SessionPhase::Closed;
                entry.record.last_at = now;
                push_line(
                    &mut entry.transcript,
                    now,
                    TranscriptRole::Status,
                    "bounded session spent — press d to drop it",
                );
            }
        }
        self.notify();
    }

    /// Run a task-scoped turn: one one-shot process, no continuity, no session
    /// left behind.
    ///
    /// Deliberately does **not** take the conversation's turn chain. Queueing
    /// independent delegated work behind an unrelated conversation would turn
    /// the daemon's concurrency budget into a single file.
    async fn run_bounded(&self, request: TurnRequest) -> Result<TurnOutcome, String> {
        self.run_one_shot(&request, None, Default::default(), Abort::new())
            .await
    }

    /// Run a turn on a long-lived session, opening it if this is its first.
    async fn run_unbound(&self, request: TurnRequest) -> Result<TurnOutcome, String> {
        // Serialize per conversation: two concurrent turns would interleave onto
        // one transcript, and two concurrent *first* turns would each start a
        // session and race to bind, silently orphaning one.
        let _turn_guard = self
            .inner
            .registry
            .acquire_turn(&request.key, request.class)
            .await;

        // Resolved *before* the session is opened, because the plan is where a
        // resumed conversation's identity lives. A closed session keeps its
        // binding, so the next turn on that key opens a new record — and
        // deciding its identity from the turn alone would hand a person's named
        // session back as an unnamed orchestrator one the moment a task frame
        // touched it.
        let plan = self.inner.registry.plan(&request.key, request.class);
        let id = self.ensure_session(&request, &plan);
        let abort = self.begin_turn(&id, &request);

        let transport = route_transport(request.class, request.key.provider);
        let outcome = match transport {
            Transport::Interactive => self.run_interactive(&id, &request, abort).await,
            Transport::OneShot => {
                let workspace_context = Arc::new(Mutex::new(plan.workspace_context.clone()));
                let outcome = self
                    .run_one_shot(
                        &request,
                        plan.resume_session_id.as_deref(),
                        workspace_context.clone(),
                        abort,
                    )
                    .await;
                // Capture-then-bind: remember the id the CLI announced so the
                // next turn resumes it. Presetting an id instead would make
                // claude refuse the second start.
                if let (true, Ok(outcome)) = (plan.bind, &outcome) {
                    if let Some(session_id) = &outcome.harness_session_id {
                        self.inner.registry.record_with_workspace_context(
                            &request.key,
                            session_id.clone(),
                            workspace_context.lock().unwrap().clone(),
                        );
                        // The binding outlives the record — it is what a later
                        // turn resumes — so the session's identity is copied
                        // onto it the moment there is a binding to hold it.
                        // Skipping this would let a resumed conversation come
                        // back as an unnamed orchestrator session.
                        if let Some(record) = self.record(&id) {
                            self.inner.registry.record_identity(
                                &request.key,
                                SessionIdentity::new(record.origin, record.name),
                            );
                        }
                    }
                }
                // A resumed process may report authoritative workspace movement
                // before failing. Refresh an existing binding even on that
                // terminal path; on a failed first turn there is no binding,
                // so `record_workspace_context` remains a no-op.
                self.inner.registry.record_workspace_context(
                    &request.key,
                    workspace_context.lock().unwrap().clone(),
                );
                outcome
            }
        };

        self.end_turn(&id, &outcome);
        outcome
    }

    /// Find or register the session a turn belongs to.
    ///
    /// `plan` is the registry's answer for this key, resolved by the caller
    /// before this runs. It carries the identity of the binding a resumed turn
    /// continues, which is the only place that survives the session record
    /// being closed.
    fn ensure_session(&self, request: &TurnRequest, plan: &TurnPlan) -> String {
        let existing = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|entry| entry.record.key == request.key && !entry.record.phase.is_terminal())
            .map(|entry| entry.record.id.clone());
        existing.unwrap_or_else(|| {
            // Resuming a binding means this is the *same* session continuing,
            // whatever door this particular turn came through: closing a
            // session keeps its binding, so a person's named session can be
            // re-opened by a task frame, and reading the origin off that frame
            // would silently rename their session to nothing and move it into
            // the dispatch pool. The persisted identity wins whenever there is
            // one.
            //
            // Auto-creation (§4.1) is the other branch: with no binding to
            // continue, a session made to serve a task frame is the
            // orchestrator's and one made for a typed prompt is the person's,
            // and the turn's own provenance is the only thing that can say.
            let identity = match plan.resume_session_id {
                Some(_) => plan.identity.clone(),
                None => SessionIdentity::new(request.origin.session_origin(), None),
            };
            self.open(super::OpenSession {
                conversation: request.key.conversation.clone(),
                provider: Some(request.key.provider),
                class: Some(request.class),
                driver: request.origin.driver(),
                workspace: None,
                model: request.model.clone(),
                origin: identity.origin(),
                name: identity.name,
            })
        })
    }

    /// Mark a session as mid-turn, record the prompt, and hand back a fresh
    /// abort handle.
    ///
    /// The handle is fresh per turn so an interrupt aimed at the previous turn
    /// can never cancel this one.
    fn begin_turn(&self, id: &str, request: &TurnRequest) -> Abort {
        let abort = Abort::new();
        let now = self.now();
        {
            let mut sessions = self.inner.sessions.lock().unwrap();
            if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
                entry.record.phase = SessionPhase::Turn;
                entry.record.last_at = now;
                entry.abort = abort.clone();
                push_line(
                    &mut entry.transcript,
                    now,
                    TranscriptRole::User,
                    &request.text,
                );
            }
        }
        self.notify();
        abort
    }

    /// Settle a session after a turn: advance the counters, record the answer,
    /// and return it to an idle-but-live phase.
    fn end_turn(&self, id: &str, outcome: &Result<TurnOutcome, String>) {
        let now = self.now();
        {
            let mut sessions = self.inner.sessions.lock().unwrap();
            let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) else {
                return;
            };
            entry.record.last_at = now;
            // A fresh handle so a late interrupt cannot arm the next turn.
            entry.abort = Abort::new();
            match outcome {
                Ok(outcome) => {
                    entry.record.turns += 1;
                    if let Some(session_id) = &outcome.harness_session_id {
                        entry.record.harness_session_id = Some(session_id.clone());
                    }
                    // An interrupt ends the turn, never the session — the phase
                    // goes back to live so the next turn is accepted.
                    entry.record.phase = SessionPhase::Live;
                    if outcome.is_error {
                        entry.record.last_error = Some(outcome.reply.clone());
                    }
                    let role = if outcome.is_error {
                        TranscriptRole::Error
                    } else {
                        TranscriptRole::Agent
                    };
                    push_line(&mut entry.transcript, now, role, &outcome.reply);
                    if outcome.aborted {
                        push_line(
                            &mut entry.transcript,
                            now,
                            TranscriptRole::Status,
                            "turn interrupted — the session is still live",
                        );
                    }
                }
                Err(message) => {
                    // A failed turn does not kill a session that is still up;
                    // only a dead transport does, and that surfaces as the next
                    // turn's error.
                    entry.record.phase = SessionPhase::Live;
                    entry.record.last_error = Some(message.clone());
                    push_line(&mut entry.transcript, now, TranscriptRole::Error, message);
                }
            }
        }
        self.notify();
    }

    /// Run a turn over the live interactive transport, opening the process on
    /// first use.
    async fn run_interactive(
        &self,
        id: &str,
        request: &TurnRequest,
        abort: Abort,
    ) -> Result<TurnOutcome, String> {
        let live = match self.live_session(id) {
            Some(live) => live,
            None => self.spawn_interactive(id, request).await?,
        };

        // Stream events into the transcript as they arrive, so a long turn is
        // watchable rather than a spinner.
        let manager = self.clone();
        let id_owned = id.to_string();
        let outcome = live
            .submit(&request.text, &abort, move |event| {
                let (role, text) = match event {
                    StreamEvent::ReasoningDelta { .. } => return,
                    StreamEvent::AssistantDelta { .. } => return,
                    StreamEvent::Tool { label } => (TranscriptRole::Tool, label.clone()),
                    StreamEvent::Session { session_id } => (
                        TranscriptRole::Status,
                        format!("harness session {session_id}"),
                    ),
                    StreamEvent::Result { .. } => return,
                };
                manager.push_line_by_id(&id_owned, role, &text);
            })
            .await;

        if outcome.is_err() {
            // The transport is gone; drop the handle so the next turn respawns
            // rather than writing into a dead pipe.
            self.clear_live(id);
        }
        outcome
    }

    /// Spawn this session's interactive child process.
    async fn spawn_interactive(
        &self,
        id: &str,
        request: &TurnRequest,
    ) -> Result<Arc<InteractiveSession>, String> {
        self.set_phase(id, SessionPhase::Starting);
        let (workspace, model) = {
            let sessions = self.inner.sessions.lock().unwrap();
            let entry = sessions
                .iter()
                .find(|entry| entry.record.id == id)
                .ok_or("no such session")?;
            (entry.record.workspace.clone(), entry.model.clone())
        };
        let provider = request.key.provider;
        let spec = InteractiveSpec {
            provider,
            bin: crate::protocol::env::provider_bin(provider, &self.inner.config.env),
            cwd: workspace,
            env: self.inner.config.env.clone(),
            model: model.or_else(|| self.inner.config.model.clone()),
            append_system_prompt: None,
            skip_permissions: self.inner.config.skip_permissions,
            extra_args: self.inner.config.extra_args.clone(),
            hooks: self.inner.config.hooks.clone(),
        };
        match InteractiveSession::open(&spec).await {
            Ok(live) => {
                let mut sessions = self.inner.sessions.lock().unwrap();
                if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
                    entry.live = Some(live.clone());
                    entry.record.phase = SessionPhase::Turn;
                }
                drop(sessions);
                self.notify();
                Ok(live)
            }
            Err(message) => {
                self.fail_session(id, &message);
                Err(message)
            }
        }
    }

    /// Run a turn as one headless process, optionally resuming `resume`.
    async fn run_one_shot(
        &self,
        request: &TurnRequest,
        resume: Option<&str>,
        workspace_context: Arc<Mutex<crate::sessions::WorkspaceContext>>,
        abort: Abort,
    ) -> Result<TurnOutcome, String> {
        let provider = self.provider_for(request);
        let options = RunTaskOptions {
            origin: crate::daemon::providers::RunTaskOrigin::Interactive,
            conversation: String::new(),
            // A one-shot turn owns its process for exactly that turn; continuity
            // across turns comes from `resume`, not from a retained session.
            session_class: SessionClass::Bounded,
            provider,
            // Interactive session turns are CLI-driven; the shared app-server is
            // chosen per delegated task, not per operator session.
            transport: crate::protocol::HarnessTransport::Cli,
            prompt: request.text.clone(),
            cwd: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
            timeout_ms: self.inner.config.turn_timeout_ms,
            model: request
                .model
                .clone()
                .or_else(|| self.inner.config.model.clone()),
            agent: self.inner.config.agent.clone(),
            extra_args: self.inner.config.extra_args.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            resume_session_id: resume.map(str::to_string),
            workspace_context: workspace_context.lock().unwrap().clone(),
            abort: abort.clone(),
            router: self.inner.config.router.clone(),
            attribution: self.inner.config.attribution,
            hooks: self.inner.config.hooks.clone(),
            on_event: None,
            on_stdin: None,
            on_session: None,
            on_workspace_context: {
                let workspace_context = workspace_context.clone();
                Some(Box::new(move |context| {
                    *workspace_context.lock().unwrap() = context;
                }))
            },
        };
        let result = (self.inner.run_task)(options).await?;
        Ok(TurnOutcome {
            reply: result.reply,
            // A one-shot run has no in-band interrupt: an abort kills the child,
            // which surfaces as an error rather than an aborted outcome.
            aborted: false,
            is_error: false,
            harness_session_id: result.session_id,
        })
    }

    /// The live process for a session, if it has one.
    fn live_session(&self, id: &str) -> Option<Arc<InteractiveSession>> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|entry| entry.record.id == id)
            .and_then(|entry| entry.live.clone())
    }

    /// Forget a session's dead process handle.
    fn clear_live(&self, id: &str) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
            entry.live = None;
        }
    }

    /// Move a session to `phase`.
    fn set_phase(&self, id: &str, phase: SessionPhase) {
        let now = self.now();
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
            entry.record.phase = phase;
            entry.record.last_at = now;
        }
        drop(sessions);
        self.notify();
    }

    /// Mark a session failed with `message`.
    fn fail_session(&self, id: &str, message: &str) {
        let now = self.now();
        {
            let mut sessions = self.inner.sessions.lock().unwrap();
            if let Some(entry) = sessions.iter_mut().find(|entry| entry.record.id == id) {
                entry.record.phase = SessionPhase::Failed;
                entry.record.last_error = Some(message.to_string());
                entry.record.last_at = now;
                push_line(&mut entry.transcript, now, TranscriptRole::Error, message);
            }
        }
        self.notify();
    }
}
