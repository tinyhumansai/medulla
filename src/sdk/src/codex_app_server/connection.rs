//! One supervised `codex app-server` child process, shared by every thread the
//! pool hands it to.
//!
//! A connection owns three things: the writer half of the child's stdin, a table
//! of requests waiting for a response, and a table of per-thread notification
//! subscribers. A single reader task decodes stdout and dispatches each message
//! to exactly one of them.
//!
//! # Concurrency
//!
//! Requests are multiplexed by JSON-RPC id, so two threads may have a call in
//! flight at once and neither blocks the other. Notifications fan out by
//! `threadId`, so a thread only ever sees its own — which is what makes sharing
//! a process safe rather than merely cheap.
//!
//! # Death
//!
//! A child that exits takes every in-flight call with it. Rather than hanging,
//! the reader task marks the connection dead on EOF and drains both tables, so
//! waiting callers get a [`AppServerError::Process`] and the pool discards the
//! connection on its next lookup.
//!
//! # Ownership
//!
//! Both background tasks hold the connection *weakly*, and deliberately so. A
//! strong handle would close a cycle — the reader waits on stdout, the child
//! that owns stdout is kept alive by the connection, and the connection is kept
//! alive by the reader — under which `Drop` never runs and a shut-down pool
//! leaves live Codex processes behind. Holding weakly makes dropping the last
//! caller-visible handle enough to kill the child, which is what closes stdout
//! and lets the tasks retire.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{json, Value};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

use super::jsonrpc::{
    notification_line, request_line, response_line, Message, Notification, RequestId,
};
use super::records::{read_record, Record, MAX_LINE_BYTES};
use super::types::{AppServerError, AppServerSpec, CLIENT_NAME};

/// The stream of notifications belonging to one thread.
///
/// Dropping it unsubscribes, which is what frees a finished lane's slot in the
/// connection's fan-out table.
pub struct ThreadSubscription {
    thread_id: String,
    connection: Arc<Connection>,
    rx: mpsc::UnboundedReceiver<Notification>,
}

impl ThreadSubscription {
    /// Await the next notification for this thread, or `None` once the
    /// connection is gone.
    pub async fn recv(&mut self) -> Option<Notification> {
        self.rx.recv().await
    }

    /// The thread this subscription belongs to.
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }
}

impl Drop for ThreadSubscription {
    fn drop(&mut self) {
        self.connection.unsubscribe(&self.thread_id);
    }
}

/// Per-thread state the reader task needs while dispatching.
struct ThreadState {
    /// Where this thread's notifications go.
    tx: mpsc::UnboundedSender<Notification>,
    /// Whether server-initiated approvals for this thread are granted.
    ///
    /// Held per thread rather than per connection because two threads in one
    /// process may legitimately run under different operator consent.
    approve: bool,
}

/// A live app-server process.
pub struct Connection {
    /// Outbound lines. The writer task owns the actual stdin handle.
    outbound: mpsc::UnboundedSender<String>,
    /// Requests awaiting a response, by id.
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, String>>>>,
    /// Notification subscribers, by thread id.
    threads: Mutex<HashMap<String, ThreadState>>,
    /// Monotonic request-id source.
    next_id: AtomicI64,
    /// Cleared when the child dies or the transport breaks.
    alive: AtomicBool,
    /// Retained so dropping the connection kills the child.
    child: Mutex<Option<Child>>,
}

impl Connection {
    /// Spawn an app-server, complete the `initialize` handshake, and return the
    /// connection ready for threads.
    ///
    /// The handshake is done here rather than lazily so a misconfigured binary
    /// or a missing credential fails at pool-acquisition time, where the error
    /// names the process, instead of on some later task that merely happened to
    /// be first.
    pub async fn open(spec: &AppServerSpec) -> Result<Arc<Self>, AppServerError> {
        let mut command = Command::new(&spec.bin);
        command.args(&spec.args);
        command.arg("app-server");
        command.env_clear();
        command.envs(&spec.env);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Codex logs diagnostics to stderr. Nothing here reads them, and
            // inheriting would interleave them with the TUI's own screen.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| AppServerError::Process(format!("spawn {}: {error}", spec.bin)))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppServerError::Process("child stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppServerError::Process("child stdout unavailable".into()))?;

        let (outbound, mut outbound_rx) = mpsc::unbounded_channel::<String>();
        let connection = Arc::new(Connection {
            outbound,
            pending: Mutex::new(HashMap::new()),
            threads: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            alive: AtomicBool::new(true),
            child: Mutex::new(Some(child)),
        });

        // Writer: one task owns stdin, so concurrent callers never interleave
        // half-written lines. The channel's sender lives on the connection, so
        // dropping the last strong handle closes this loop on its own.
        let writer_connection = Arc::downgrade(&connection);
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = outbound_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    if let Some(connection) = writer_connection.upgrade() {
                        connection.mark_dead("stdin closed");
                    }
                    return;
                }
            }
        });

        // Reader: decodes stdout and dispatches. Weak, so the child is torn down
        // when the pool drops the connection — see the module's ownership note.
        let reader_connection = Arc::downgrade(&connection);
        tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            loop {
                match read_record(&mut stdout).await {
                    Ok(Record::Line(line)) => {
                        let Some(message) = Message::parse(&line) else {
                            continue;
                        };
                        // Upgraded per message rather than once: the connection
                        // may be dropped at any point, and there is nothing to
                        // dispatch to once it is.
                        let Some(connection) = reader_connection.upgrade() else {
                            return;
                        };
                        connection.dispatch(message);
                    }
                    // Skipped, not fatal: the record was discarded as it
                    // arrived, and the framing resynchronised on its newline.
                    Ok(Record::Oversized) => tracing::warn!(
                        target: "medulla::codex_app_server",
                        cap = MAX_LINE_BYTES,
                        "dropped a codex app-server record longer than the line cap"
                    ),
                    Ok(Record::Eof) => break,
                    Err(error) => {
                        if let Some(connection) = reader_connection.upgrade() {
                            connection.mark_dead(&format!("stdout read failed: {error}"));
                        }
                        return;
                    }
                }
            }
            if let Some(connection) = reader_connection.upgrade() {
                connection.mark_dead("process exited");
            }
        });

        connection
            .call(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": CLIENT_NAME,
                        "title": "Medulla",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
            )
            .await?;
        connection.notify("initialized", json!({}))?;
        Ok(connection)
    }

    /// Terminate the child and fail everything still waiting on it.
    ///
    /// Shutdown does not merely drop the connection, because a caller may still
    /// hold a handle — a lane mid-turn, a live [`ThreadSubscription`] — and
    /// waiting for the last one to retire would leave a Codex process running
    /// past the point the operator asked for it to stop. Callers see the same
    /// [`AppServerError::Process`] a crashed child produces, which every path
    /// already handles.
    pub fn close(&self) {
        self.mark_dead("connection closed");
    }

    /// Whether the child is still serving.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Send a request and await its response.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, AppServerError> {
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("app-server pending lock");
            pending.insert(id.clone(), tx);
        }
        if let Err(error) = self.send(request_line(&id, method, params)) {
            self.pending
                .lock()
                .expect("app-server pending lock")
                .remove(&id);
            return Err(error);
        }
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(AppServerError::Rpc {
                method: method.to_string(),
                message,
            }),
            // The sender is only dropped when the connection dies, which is
            // exactly what `mark_dead` does to every pending entry it cannot
            // answer.
            Err(_) => Err(AppServerError::Process(
                "connection closed while awaiting a response".into(),
            )),
        }
    }

    /// Send a notification, expecting no answer.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), AppServerError> {
        self.send(notification_line(method, params))
    }

    /// Send a request without waiting for its response.
    ///
    /// The response is discarded when it arrives, because nothing is registered
    /// to receive it. For requests sent on a path that is already failing — an
    /// interrupt on the way out of an abort — where the answer could not change
    /// what happens next and awaiting it would only delay the error.
    pub fn call_detached(&self, method: &str, params: Value) -> Result<(), AppServerError> {
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::SeqCst));
        self.send(request_line(&id, method, params))
    }

    /// Begin receiving `thread_id`'s notifications.
    ///
    /// `approve` decides how server-initiated approvals on this thread are
    /// answered — see [`ThreadState::approve`]. Call this *before* starting the
    /// turn: notifications produced by a turn begin arriving as soon as the
    /// request is written, and a subscription opened afterwards would miss the
    /// start of its own turn.
    pub fn subscribe(self: &Arc<Self>, thread_id: &str, approve: bool) -> ThreadSubscription {
        let (tx, rx) = mpsc::unbounded_channel();
        self.threads
            .lock()
            .expect("app-server threads lock")
            .insert(thread_id.to_string(), ThreadState { tx, approve });
        ThreadSubscription {
            thread_id: thread_id.to_string(),
            connection: self.clone(),
            rx,
        }
    }

    /// Stop receiving a thread's notifications.
    fn unsubscribe(&self, thread_id: &str) {
        self.threads
            .lock()
            .expect("app-server threads lock")
            .remove(thread_id);
    }

    /// Queue one outbound line.
    fn send(&self, line: String) -> Result<(), AppServerError> {
        if !self.is_alive() {
            return Err(AppServerError::Process("connection is closed".into()));
        }
        self.outbound
            .send(line)
            .map_err(|_| AppServerError::Transport("writer task is gone".into()))
    }

    /// Route one decoded message.
    fn dispatch(&self, message: Message) {
        match message {
            Message::Response { id, result } => self.settle(id, Ok(result)),
            Message::Error { id, message } => self.settle(id, Err(message)),
            Message::Notification(notification) => {
                // A notification naming no thread is connection-scoped
                // (`remoteControl/status/changed` and friends). Nothing here
                // acts on those, and delivering one to an arbitrary thread would
                // be worse than dropping it.
                let Some(thread_id) = notification.thread_id() else {
                    return;
                };
                let threads = self.threads.lock().expect("app-server threads lock");
                if let Some(state) = threads.get(thread_id) {
                    // A closed receiver means the lane finished; its `Drop` will
                    // remove the entry.
                    let _ = state.tx.send(notification);
                }
            }
            Message::ServerRequest { id, method, params } => self.answer(id, &method, &params),
        }
    }

    /// Complete a waiting call, if anything is still waiting for it.
    fn settle(&self, id: RequestId, result: Result<Value, String>) {
        let waiter = self
            .pending
            .lock()
            .expect("app-server pending lock")
            .remove(&id);
        if let Some(waiter) = waiter {
            let _ = waiter.send(result);
        }
    }

    /// Answer a request the server made of us.
    ///
    /// Only approvals are answered substantively, and only according to the
    /// consent the thread was subscribed with. Everything else — dynamic tool
    /// calls, MCP elicitations, user-input prompts — is refused with a
    /// method-not-found error, because no operator is watching a delegated task
    /// and a fabricated answer would be worse than a refusal Codex can route
    /// around.
    fn answer(&self, id: RequestId, method: &str, params: &Value) {
        let approve = params
            .get("threadId")
            .and_then(Value::as_str)
            .and_then(|thread_id| {
                self.threads
                    .lock()
                    .expect("app-server threads lock")
                    .get(thread_id)
                    .map(|state| state.approve)
            })
            .unwrap_or(false);
        let decision = if approve { "accept" } else { "decline" };
        let line = match method {
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "applyPatchApproval"
            | "execCommandApproval" => {
                Some(response_line(&id, json!({ "decision": decision })))
            }
            _ => Some(
                json!({
                    "jsonrpc": "2.0",
                    "id": id.to_value(),
                    "error": {
                        "code": -32601,
                        "message": format!(
                            "medulla does not serve `{method}`: a delegated task has no operator to ask"
                        ),
                    },
                })
                .to_string(),
            ),
        };
        if let Some(line) = line {
            let _ = self.send(line);
        }
    }

    /// Latch the connection dead and fail everything still waiting on it.
    fn mark_dead(&self, reason: &str) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }
        let pending: Vec<_> = self
            .pending
            .lock()
            .expect("app-server pending lock")
            .drain()
            .collect();
        for (_, waiter) in pending {
            let _ = waiter.send(Err(reason.to_string()));
        }
        // Dropping the senders closes every subscription, so a lane awaiting a
        // notification wakes with `None` instead of hanging on a dead process.
        self.threads
            .lock()
            .expect("app-server threads lock")
            .clear();
        if let Some(mut child) = self.child.lock().expect("app-server child lock").take() {
            let _ = child.start_kill();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().expect("app-server child lock").take() {
            let _ = child.start_kill();
        }
    }
}
