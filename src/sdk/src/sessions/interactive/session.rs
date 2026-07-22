//! The live interactive harness process: spawn once, submit turns over stdin,
//! read semantic events off stdout, interrupt a turn without killing the
//! session, and close.
//!
//! One [`InteractiveSession`] owns one child. Turns are serialized by the event
//! receiver's mutex — a second `submit` waits rather than racing, which closes
//! the hazard the JS prior art left open (two concurrent submits there both
//! settled on the first `result` frame).

use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::daemon::providers::Abort;
use crate::tinyplace::HarnessProvider;

use super::super::types::TurnOutcome;
use super::frames::{encode_interrupt, encode_user_message, map_stream_frame, StreamEvent};

/// A record that never terminates in a newline is dropped past this size.
const MAX_RECORD_BYTES: usize = 1_048_576;

/// How long to wait for the interrupt's terminating `result` before settling the
/// turn ourselves.
///
/// The terminator belongs to *this* turn, so it must be consumed here — leaking
/// it into the next turn's reader would look like a premature completion there.
/// This grace window is the fallback for a transport that never emits one.
const INTERRUPT_GRACE: Duration = Duration::from_millis(2_000);

/// How a session's child process should be started.
#[derive(Debug, Clone)]
pub struct InteractiveSpec {
    /// Which CLI to spawn. Only [`HarnessProvider::Claude`] supports this
    /// transport today; see
    /// [`can_run_interactive`](super::super::routing::can_run_interactive).
    pub provider: HarnessProvider,
    /// The resolved binary name or path.
    pub bin: String,
    /// Working directory for the child.
    pub cwd: String,
    /// The full environment the child runs with (the parent env is cleared).
    pub env: std::collections::HashMap<String, String>,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional system-prompt suffix.
    pub append_system_prompt: Option<String>,
    /// Whether to pass the provider's skip-permissions flag.
    pub skip_permissions: bool,
    /// Extra argv appended to the built base args.
    pub extra_args: Vec<String>,
}

/// Build the argv for an interactive session.
///
/// The prompt is *not* an argument here — turns arrive on stdin, which is the
/// whole point of this transport. That also sidesteps the leading-`-` injection
/// hazard the one-shot path has to neutralize.
pub fn build_interactive_args(spec: &InteractiveSpec) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ]
    .map(String::from)
    .to_vec();
    if let Some(model) = &spec.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(prompt) = &spec.append_system_prompt {
        args.push("--append-system-prompt".to_string());
        args.push(prompt.clone());
    }
    if spec.skip_permissions {
        args.push("--dangerously-skip-permissions".to_string());
    }
    args.extend(spec.extra_args.iter().cloned());
    args
}

/// A live interactive harness process.
pub struct InteractiveSession {
    child: Mutex<Option<Child>>,
    stdin: AsyncMutex<Option<ChildStdin>>,
    /// Semantic events from the reader task. Held behind a mutex so exactly one
    /// turn consumes the stream at a time.
    events: AsyncMutex<mpsc::UnboundedReceiver<StreamEvent>>,
    /// The harness's own session id, once announced. Observability only.
    harness_session_id: Mutex<Option<String>>,
    interrupt_seq: AtomicU64,
}

impl InteractiveSession {
    /// Spawn the child and start reading its stream.
    ///
    /// Fails when the binary is missing or the pipes cannot be taken. The
    /// process is started eagerly here; callers that want a zero-cost handle
    /// keep the [`InteractiveSpec`] and open lazily on the first turn.
    pub async fn open(spec: &InteractiveSpec) -> Result<Arc<Self>, String> {
        // A provider binary is untrusted configuration — an override from a
        // config file or an env var. Check it against the PATH the *child* will
        // get (`spec.env`, since the environment is cleared), so a bad override
        // is named as such instead of arriving as a bare OS spawn error.
        let on_path = crate::daemon::providers::make_path_lookup(&spec.env);
        if !on_path(&spec.bin) {
            return Err(format!(
                "{} is not an executable on the session's PATH",
                spec.bin
            ));
        }
        let args = build_interactive_args(spec);
        let mut child = Command::new(&spec.bin)
            .args(&args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| format!("failed to start {}: {err}", spec.bin))?;

        let stdin = child.stdin.take().ok_or("child has no stdin")?;
        let stdout = child.stdout.take().ok_or("child has no stdout")?;

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break, // EOF or read error: the stream is done.
                    Ok(_) => {
                        if buf.len() > MAX_RECORD_BYTES {
                            continue; // unparseable oversized record — drop it.
                        }
                        let raw = String::from_utf8_lossy(&buf);
                        let raw = raw.trim();
                        if raw.is_empty() {
                            continue;
                        }
                        // A non-JSON line is not a frame; the CLI interleaves
                        // occasional human-readable noise on stdout.
                        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) else {
                            continue;
                        };
                        for event in map_stream_frame(&parsed) {
                            if tx.send(event).is_err() {
                                return; // the session was dropped
                            }
                        }
                    }
                }
            }
        });

        Ok(Arc::new(InteractiveSession {
            child: Mutex::new(Some(child)),
            stdin: AsyncMutex::new(Some(stdin)),
            events: AsyncMutex::new(rx),
            harness_session_id: Mutex::new(None),
            interrupt_seq: AtomicU64::new(0),
        }))
    }

    /// The harness's own session id, once it has announced one.
    ///
    /// Observability only — never a resume handle, never a durable key.
    pub fn harness_session_id(&self) -> Option<String> {
        self.harness_session_id.lock().unwrap().clone()
    }

    /// Run one turn to completion, streaming its events to `on_event`.
    ///
    /// Waits for any turn already in flight. Signalling `abort` interrupts *this
    /// turn* and leaves the session alive for the next one.
    pub async fn submit<F>(
        &self,
        text: &str,
        abort: &Abort,
        mut on_event: F,
    ) -> Result<TurnOutcome, String>
    where
        F: FnMut(&StreamEvent) + Send,
    {
        // Serializing on the event receiver is what makes concurrent submits
        // safe: the second turn cannot write its prompt until the first has
        // consumed its terminating `result`.
        let mut events = self.events.lock().await;

        self.write_line(&encode_user_message(text)).await?;

        let mut reply = String::new();
        let mut aborting = false;
        let mut grace = std::pin::pin!(tokio::time::sleep(Duration::from_secs(0)));
        // Park the grace timer far out until an interrupt actually arms it.
        grace
            .as_mut()
            .reset(tokio::time::Instant::now() + Duration::from_secs(86_400));

        loop {
            tokio::select! {
                // The abort arm is disabled once an interrupt is in flight, so a
                // second signal cannot re-arm the grace window.
                _ = abort.cancelled(), if !aborting => {
                    aborting = true;
                    let request_id = format!(
                        "req_interrupt_{}",
                        self.interrupt_seq.fetch_add(1, Ordering::SeqCst)
                    );
                    // A failed write means stdin is already gone and the turn is
                    // ending anyway; the grace timer still settles it.
                    let _ = self.write_line(&encode_interrupt(&request_id)).await;
                    grace.as_mut().reset(tokio::time::Instant::now() + INTERRUPT_GRACE);
                }
                _ = grace.as_mut(), if aborting => {
                    return Ok(TurnOutcome {
                        reply,
                        aborted: true,
                        is_error: false,
                        harness_session_id: self.harness_session_id(),
                    });
                }
                event = events.recv() => {
                    let Some(event) = event else {
                        // The stream ended without a `result`: the child died.
                        return Err("interactive session ended before the turn completed".into());
                    };
                    on_event(&event);
                    match event {
                        StreamEvent::Session { session_id } => {
                            *self.harness_session_id.lock().unwrap() = Some(session_id);
                        }
                        StreamEvent::AssistantDelta { text } => reply.push_str(&text),
                        StreamEvent::Result { reply: result, is_error, session_id } => {
                            if let Some(session_id) = session_id {
                                *self.harness_session_id.lock().unwrap() = Some(session_id);
                            }
                            // When an interrupt is in flight, the terminating
                            // `result` (subtype error_during_execution) IS this
                            // turn ending — settle it as aborted, not as a
                            // normal completion, and keep the streamed text
                            // rather than its empty `result` field.
                            return Ok(if aborting {
                                TurnOutcome {
                                    reply,
                                    aborted: true,
                                    is_error,
                                    harness_session_id: self.harness_session_id(),
                                }
                            } else {
                                TurnOutcome {
                                    reply: if result.is_empty() { reply } else { result },
                                    aborted: false,
                                    is_error,
                                    harness_session_id: self.harness_session_id(),
                                }
                            });
                        }
                        StreamEvent::ReasoningDelta { .. } | StreamEvent::Tool { .. } => {}
                    }
                }
            }
        }
    }

    /// Write one newline-terminated line to the child's stdin.
    async fn write_line(&self, line: &str) -> Result<(), String> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or("interactive session is closed")?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| format!("interactive session stdin write failed: {err}"))?;
        stdin
            .flush()
            .await
            .map_err(|err| format!("interactive session stdin flush failed: {err}"))
    }

    /// Close the session: drop stdin (the CLI's clean exit signal), then reap.
    ///
    /// A hard stop, unlike an interrupt: the conversation does not survive it.
    pub async fn close(&self) {
        // Dropping stdin gives the child EOF; most CLIs exit on it.
        *self.stdin.lock().await = None;
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let reaped = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            if reaped.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
    }
}
