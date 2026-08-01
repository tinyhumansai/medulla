//! Launching a harness on a fresh pty, and draining it into the emulator.

use std::io::Read;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};

use super::super::handle::{SessionHandle, SessionMeta};
use super::super::launch::{interactive_args, mint_session_id};
use super::super::types::{LaunchSpec, DEFAULT_COLS, DEFAULT_ROWS, SCROLLBACK};

use super::{write, PtyManager, BUF_LEN, OPENPTY_ATTEMPTS, OPENPTY_RETRY_CAP, OPENPTY_RETRY_PAUSE};

impl PtyManager {
    /// Launch a harness on a fresh PTY and start draining it.
    ///
    /// Returns the new session's id. The child is started immediately — unlike
    /// the headless session model there is no lazy handle, because the whole
    /// point is to have a screen to look at.
    ///
    /// **Blocking.** This forks and execs, and can sleep for its `openpty`
    /// backoff, so an async caller must reach it through
    /// `tokio::task::spawn_blocking` rather than calling it on a runtime worker.
    /// The executor does exactly that; doing it inline used to park a tokio
    /// worker for up to half a second per launch, which under a burst starved
    /// the runtime the inbox drain and the screen samplers also live on.
    pub fn open(&self, spec: LaunchSpec) -> Result<String, String> {
        let pty = open_pty()?;

        // Mint the id *before* spawning, so the transcript this session writes is
        // findable by name rather than by guessing which file is newest.
        let session_id = spec
            .session_id
            .clone()
            .or_else(|| mint_session_id(spec.provider));

        let mut command = CommandBuilder::new(&spec.bin);
        for arg in interactive_args(
            spec.provider,
            session_id.as_deref(),
            spec.skip_permissions,
            spec.model.as_deref(),
            &spec.extra_args,
        ) {
            command.arg(arg);
        }
        command.cwd(&spec.cwd);
        // The child gets exactly the environment we were handed, like the
        // headless path — no inherited surprises.
        command.env_clear();
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        // A harness decides whether to paint from TERM; without one it falls
        // back to dumb line mode and there is nothing to render.
        if !spec.env.contains_key("TERM") {
            command.env("TERM", "xterm-256color");
        }

        let child = pty
            .slave
            .spawn_command(command)
            .map_err(|err| format!("could not start {}: {err}", spec.bin))?;
        // Drop the slave once the child holds it: while we keep a handle the
        // master never sees EOF, so the reader would hang after the child exits.
        drop(pty.slave);

        let reader = pty
            .master
            .try_clone_reader()
            .map_err(|err| format!("could not read the pty: {err}"))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|err| format!("could not write to the pty: {err}"))?;

        let now = self.now();
        let id = format!("w_{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        let handle = Arc::new(SessionHandle::new(
            SessionMeta {
                id: id.clone(),
                label: spec.label,
                provider: spec.provider,
                cwd: spec.cwd,
                started_at: now,
            },
            session_id,
            vt100::Parser::new(DEFAULT_ROWS, DEFAULT_COLS, SCROLLBACK),
            pty.master,
            writer,
            child,
        ));

        write(&self.inner.sessions).push(handle.clone());

        // Only now: the reader marks output on every read, and a child that
        // greets the pty immediately would otherwise have its first output land
        // before there is a session to record it against — losing the
        // `last_output_at` that idle detection reads.
        self.spawn_reader(handle, reader);

        Ok(id)
    }

    /// Drain the PTY master into the emulator on a blocking thread.
    ///
    /// The thread owns the session's `Arc` outright, so draining costs no
    /// registry lock and no lookup at all: where this used to take a
    /// process-wide mutex and scan every session once per 8 KB read, it is now
    /// two atomic stores against a handle already in hand.
    ///
    /// The `yield_now` is not decoration. A child streaming at pipe speed keeps
    /// this loop runnable continuously, and `std::sync::Mutex` makes no fairness
    /// promise — so the reader can reacquire the emulator immediately, over and
    /// over, while a render pass waits on the same lock for tens of
    /// milliseconds. Yielding after a *full* buffer marks the one case where
    /// that is likely (the child had at least [`BUF_LEN`] queued, so it is
    /// flooding rather than idling) and lets a waiter in. A partial read means
    /// the child has caught up and there is nothing to be fair about.
    fn spawn_reader(&self, handle: Arc<SessionHandle>, mut reader: Box<dyn Read + Send>) {
        let manager = self.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; BUF_LEN];
            loop {
                match reader.read(&mut buf) {
                    // EOF: the child closed the pty. Its last screen stays
                    // readable — the operator usually wants to see how it ended.
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        handle.process(&buf[..n]);
                        handle.mark_output(manager.now());
                        if n == BUF_LEN {
                            std::thread::yield_now();
                        }
                    }
                }
            }
            handle.reap(manager.now());
        });
    }
}

/// Allocate a pty, retrying only the failures that are actually transient.
fn open_pty() -> Result<PtyPair, String> {
    let size = PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    };
    let mut attempt = 1;
    loop {
        match native_pty_system().openpty(size) {
            Ok(pty) => return Ok(pty),
            // Not a race — retrying would burn the whole budget and fail with
            // the same error. Descriptor exhaustion in particular is a capacity
            // signal: the answer is fewer live sessions or a higher
            // `RLIMIT_NOFILE`, not another twenty attempts.
            Err(err) if !is_transient(&err) => {
                return Err(format!("could not allocate a pty: {err}"))
            }
            Err(err) if attempt >= OPENPTY_ATTEMPTS => {
                return Err(format!(
                    "could not allocate a pty after {OPENPTY_ATTEMPTS} attempts: {err}"
                ))
            }
            Err(_) => {
                // Linear backoff to a cap: a burst of sessions opening together
                // would otherwise retry in lockstep, colliding on every attempt
                // for the same reason they collided on the first.
                let pause = OPENPTY_RETRY_PAUSE
                    .saturating_mul(attempt)
                    .min(OPENPTY_RETRY_CAP);
                std::thread::sleep(pause);
                attempt += 1;
            }
        }
    }
}

/// Whether an `openpty` failure is worth retrying.
///
/// Pty allocation genuinely does race — two processes can reach for the same
/// free slot — and those failures clear in milliseconds. Running out of file
/// descriptors does not: nothing about waiting frees one, so a retry loop only
/// delays the error it was always going to return, twenty pauses later.
fn is_transient(err: &anyhow::Error) -> bool {
    /// The per-process descriptor limit, on both Linux and macOS.
    const EMFILE: i32 = 24;
    /// The system-wide descriptor limit, on both Linux and macOS.
    const ENFILE: i32 = 23;

    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if matches!(io.raw_os_error(), Some(EMFILE) | Some(ENFILE)) {
                return false;
            }
        }
    }
    // Nothing to inspect — assume a race and let the bounded retry decide.
    true
}
