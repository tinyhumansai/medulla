//! The async connection driver for the core (`medulla-serve`) runtime, and the
//! [`CoreRuntime`] handle the UI holds.
//!
//! One background task owns the unix socket end-to-end: it connects, runs the
//! `ready`→`hello` handshake with a version check, then services the connection
//! — folding `event` frames into shared [`CoreState`] and correlating `res`
//! frames to the [`Command`]s the trait methods enqueue. A dropped socket
//! triggers a re-attach (serve-protocol §7 restart-and-retry); a fatal handshake
//! outcome (version mismatch / `hello` rejection) stops retrying and marks the
//! runtime unavailable. This milestone is attach-only: it never spawns serve.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use super::protocol::{
    check_ready, fold_event, hello_params, parse_line, port_unavailable_ret, req_line, Inbound,
    ReadyCheck,
};
use super::types::{Command, ConnState, CoreError, CoreState, HANDSHAKE_TIMEOUT, RECONNECT_DELAY};

/// A [`Runtime`](crate::runtime::Runtime) attached to a `medulla-serve` process
/// over a unix domain socket. Snapshot/subscribe read the shared state the
/// driver folds into; the mutating methods enqueue [`Command`]s the driver
/// serializes onto the wire.
pub struct CoreRuntime {
    /// Shared connection state the snapshot is rendered from.
    pub(super) state: Arc<Mutex<CoreState>>,
    /// Pinged after every fold/mutation so the UI re-pulls a snapshot.
    pub(super) tx: broadcast::Sender<()>,
    /// Commands to the connection driver.
    pub(super) cmd_tx: mpsc::UnboundedSender<Command>,
    /// The attached socket path, for diagnostics.
    pub(super) socket_path: PathBuf,
    /// The driver task handle, aborted on drop.
    driver: Mutex<Option<JoinHandle<()>>>,
}

impl CoreRuntime {
    /// Attach to a `medulla-serve` process already listening at `socket_path`.
    ///
    /// Spawns the connection driver and returns immediately; the handshake runs
    /// in the background. Callers observe readiness through
    /// [`snapshot`](crate::runtime::Runtime::snapshot)'s stream state /
    /// [`describe`](crate::runtime::Runtime::describe). Must be called from
    /// within a Tokio runtime.
    pub fn attach(socket_path: PathBuf) -> Self {
        let state = Arc::new(Mutex::new(CoreState::new()));
        let (tx, _rx) = broadcast::channel(256);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let driver = tokio::spawn(driver_loop(
            socket_path.clone(),
            state.clone(),
            tx.clone(),
            cmd_rx,
        ));
        CoreRuntime {
            state,
            tx,
            cmd_tx,
            socket_path,
            driver: Mutex::new(Some(driver)),
        }
    }

    /// Ping subscribers that state changed.
    pub(super) fn ping(&self) {
        let _ = self.tx.send(());
    }
}

impl Drop for CoreRuntime {
    fn drop(&mut self) {
        if let Some(handle) = self.driver.lock().unwrap().take() {
            handle.abort();
        }
    }
}

/// How one connection attempt ended, deciding whether the driver re-attaches.
enum ConnOutcome {
    /// The command channel closed (handle dropped) or a clean `shutdown`.
    Shutdown,
    /// A fatal handshake outcome; carries the reason. The driver stops retrying.
    Fatal(String),
    /// The socket dropped mid-session (or was not yet present). Re-attach.
    Dropped,
}

/// The driver's outer loop: attach, service, and re-attach on a transient drop
/// until the handle is dropped or a fatal outcome latches.
async fn driver_loop(
    path: PathBuf,
    state: Arc<Mutex<CoreState>>,
    tx: broadcast::Sender<()>,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
) {
    let mut attempt: u64 = 0;
    loop {
        match serve_connection(&path, &state, &tx, &mut cmd_rx, attempt).await {
            ConnOutcome::Shutdown => return,
            ConnOutcome::Fatal(reason) => {
                set_conn(&state, &tx, ConnState::Unavailable(reason));
                return;
            }
            ConnOutcome::Dropped => {
                set_conn(&state, &tx, ConnState::Reconnecting);
                attempt += 1;
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }
    }
}

/// Connect, handshake, and service one connection until it drops, a fatal
/// outcome latches, or the handle shuts the driver down.
async fn serve_connection(
    path: &PathBuf,
    state: &Arc<Mutex<CoreState>>,
    tx: &broadcast::Sender<()>,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
    attempt: u64,
) -> ConnOutcome {
    let stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        // Socket not present yet / refused: treat as a transient drop so the
        // driver keeps re-attaching (serve may still be coming up).
        Err(_) => return ConnOutcome::Dropped,
    };
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    let hello_ok = match tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        handshake(&mut reader, &mut write_half),
    )
    .await
    {
        Ok(Ok(ok)) => ok,
        Ok(Err(HandshakeError::Fatal(reason))) => return ConnOutcome::Fatal(reason),
        Ok(Err(HandshakeError::Dropped)) => return ConnOutcome::Dropped,
        Err(_) => return ConnOutcome::Dropped, // handshake timeout
    };

    // Handshake succeeded: record identity, reset the per-connection cursor, go
    // Live, and ping so the UI observes the attach.
    record_ready(state, hello_ok.serve, hello_ok.session_id);
    {
        let mut s = state.lock().unwrap();
        // On a re-attach we replay (below) to rebaseline; clear the fold-derived
        // state first so the replayed events rebuild it rather than double-count
        // onto what the dropped connection already folded.
        if attempt > 0 {
            s.reset_for_replay();
        }
        s.reset_stream_cursor();
        s.conn = ConnState::Live;
    }
    let _ = tx.send(());

    // Begin the event stream (idempotent); replay on a re-attach to rebaseline.
    let subscribe = if attempt == 0 {
        req_line("s1", "subscribe", &serde_json::json!({}))
    } else {
        req_line("s1", "subscribe", &serde_json::json!({ "replay": 256 }))
    };
    if write_half.write_all(subscribe.as_bytes()).await.is_err() {
        return ConnOutcome::Dropped;
    }

    // Reader task: forward parsed inbound frames over a channel (read_line is not
    // cancel-safe, so it must not share a select! with the command loop).
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Inbound>();
    let reader_task = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break, // EOF or transport error
                Ok(_) => {
                    if let Some(frame) = parse_line(&line) {
                        if in_tx.send(frame).is_err() {
                            break;
                        }
                    }
                    // Unparseable lines are skipped (serve-protocol §1).
                }
            }
        }
    });

    let mut pending: HashMap<String, oneshot::Sender<Result<serde_json::Value, CoreError>>> =
        HashMap::new();
    let mut next_id: u64 = 1;

    let outcome = loop {
        tokio::select! {
            frame = in_rx.recv() => match frame {
                None => break ConnOutcome::Dropped, // reader ended → socket dropped
                Some(inbound) => {
                    if handle_inbound(inbound, &mut pending, state, tx, &mut write_half)
                        .await
                        .is_err()
                    {
                        break ConnOutcome::Dropped;
                    }
                }
            },
            cmd = cmd_rx.recv() => match cmd {
                None => break ConnOutcome::Shutdown, // handle dropped
                Some(Command::Shutdown { reply }) => {
                    let stop = req_line("stop", "stop", &serde_json::json!({ "drain": false }));
                    let _ = write_half.write_all(stop.as_bytes()).await;
                    let _ = reply.send(());
                    break ConnOutcome::Shutdown;
                }
                Some(command) => {
                    if dispatch_command(command, &mut pending, &mut next_id, &mut write_half)
                        .await
                        .is_err()
                    {
                        break ConnOutcome::Dropped;
                    }
                }
            },
        }
    };

    // Fail any in-flight requests so their awaiters stop blocking, and stop the
    // reader task before re-attaching.
    fail_pending(&mut pending);
    reader_task.abort();
    outcome
}

/// The serve identity learned from a successful handshake.
struct HelloOk {
    /// The serve build version (from `ready`).
    serve: Option<String>,
    /// The session id serve owns (from `ready`).
    session_id: Option<String>,
}

/// The handshake: read the `ready` banner, check the version, send `hello`, and
/// await its `res` (serve-protocol §3).
async fn handshake(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut OwnedWriteHalf,
) -> Result<HelloOk, HandshakeError> {
    // 1. Read lines until the `ready` banner (serve writes it first).
    let (serve, session_id) = match read_until_ready(reader).await? {
        ReadyCheck::Ok { serve, session_id } => (serve, session_id),
        ReadyCheck::Fatal(reason) => return Err(HandshakeError::Fatal(reason)),
    };

    // 2. Send `hello`.
    let hello = req_line("h1", "hello", &hello_params());
    writer
        .write_all(hello.as_bytes())
        .await
        .map_err(|_| HandshakeError::Dropped)?;

    // 3. Await the `hello` response.
    loop {
        let line = read_line(reader).await?;
        match parse_line(&line) {
            Some(Inbound::Res { id, ok, error, .. }) if id == "h1" => {
                if ok {
                    return Ok(HelloOk { serve, session_id });
                }
                let reason = error
                    .map(|e| format!("hello rejected: {} ({})", e.message, e.code))
                    .unwrap_or_else(|| "hello rejected".to_string());
                return Err(HandshakeError::Fatal(reason));
            }
            _ => continue, // pre-hello noise is ignored
        }
    }
}

/// Read lines until a `ready` frame, returning its validated check.
async fn read_until_ready(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<ReadyCheck, HandshakeError> {
    loop {
        let line = read_line(reader).await?;
        if let Some(Inbound::Ready {
            protocol,
            serve,
            session_id,
            error,
        }) = parse_line(&line)
        {
            return Ok(check_ready(protocol, serve, session_id, error));
        }
    }
}

/// Read one non-empty line, mapping EOF/error to a dropped handshake.
async fn read_line(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<String, HandshakeError> {
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) | Err(_) => Err(HandshakeError::Dropped),
        Ok(_) => Ok(line),
    }
}

/// How the handshake can fail.
enum HandshakeError {
    /// A permanent rejection (version mismatch / `hello` error / startup error).
    Fatal(String),
    /// A transport drop mid-handshake; the driver re-attaches.
    Dropped,
}

/// Record the serve version + session id from the `ready` banner into state.
/// Called after a successful handshake and on any late/duplicate `ready`.
fn record_ready(state: &Arc<Mutex<CoreState>>, serve: Option<String>, session_id: Option<String>) {
    let mut s = state.lock().unwrap();
    if serve.is_some() {
        s.serve_version = serve;
    }
    if let Some(id) = session_id {
        if !id.is_empty() {
            s.session_id = id;
        }
    }
}

/// Handle one inbound frame: correlate a `res`, fold an `event`, or refuse an
/// unhosted port `call`. Returns `Err` only on a write failure (→ drop).
async fn handle_inbound(
    inbound: Inbound,
    pending: &mut HashMap<String, oneshot::Sender<Result<serde_json::Value, CoreError>>>,
    state: &Arc<Mutex<CoreState>>,
    tx: &broadcast::Sender<()>,
    writer: &mut OwnedWriteHalf,
) -> Result<(), ()> {
    match inbound {
        Inbound::Ready {
            serve, session_id, ..
        } => {
            // A late/duplicate ready (e.g. after replay): refresh diagnostics.
            record_ready(state, serve, session_id);
        }
        Inbound::Res {
            id,
            ok,
            result,
            error,
        } => {
            if let Some(reply) = pending.remove(&id) {
                let outcome = if ok {
                    Ok(result)
                } else {
                    Err(error
                        .map(CoreError::from)
                        .unwrap_or_else(|| CoreError::transport("request failed")))
                };
                let _ = reply.send(outcome);
            }
        }
        Inbound::Event { seq, event } => {
            let changed = {
                let mut s = state.lock().unwrap();
                s.note_stream_seq(seq);
                fold_event(&mut s, &event)
            };
            if changed {
                let _ = tx.send(());
            }
        }
        Inbound::Call { id, port } => {
            // Port hosting (reverse-RPC) is a later milestone; refuse for now so
            // serve never hangs on the missing `ret` (serve-protocol §5/§7).
            let line = port_unavailable_ret(&id, &port);
            writer.write_all(line.as_bytes()).await.map_err(|_| ())?;
        }
    }
    Ok(())
}

/// Serialize a queued command onto the wire, registering a `Request`'s waiter.
async fn dispatch_command(
    command: Command,
    pending: &mut HashMap<String, oneshot::Sender<Result<serde_json::Value, CoreError>>>,
    next_id: &mut u64,
    writer: &mut OwnedWriteHalf,
) -> Result<(), ()> {
    match command {
        Command::Request { op, params, reply } => {
            let id = format!("r{next_id}");
            *next_id += 1;
            pending.insert(id.clone(), reply);
            let line = req_line(&id, op, &params);
            if writer.write_all(line.as_bytes()).await.is_err() {
                if let Some(reply) = pending.remove(&id) {
                    let _ = reply.send(Err(CoreError::transport("socket write failed")));
                }
                return Err(());
            }
        }
        Command::Fire { op, params } => {
            let id = format!("r{next_id}");
            *next_id += 1;
            let line = req_line(&id, op, &params);
            // Fire-and-forget: the ack (`res`) is ignored; a write failure drops.
            writer.write_all(line.as_bytes()).await.map_err(|_| ())?;
        }
        Command::Shutdown { reply } => {
            let _ = reply.send(());
        }
    }
    Ok(())
}

/// Fail every in-flight request so its awaiter stops blocking after a drop.
fn fail_pending(
    pending: &mut HashMap<String, oneshot::Sender<Result<serde_json::Value, CoreError>>>,
) {
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err(CoreError::transport("connection dropped")));
    }
}

/// Set the connection lifecycle and ping subscribers.
fn set_conn(state: &Arc<Mutex<CoreState>>, tx: &broadcast::Sender<()>, conn: ConnState) {
    {
        let mut s = state.lock().unwrap();
        // Never demote a Live connection's running flag here; only the fold path
        // owns that. A reconnect leaves the last-known board in place.
        s.conn = conn;
    }
    let _ = tx.send(());
}
