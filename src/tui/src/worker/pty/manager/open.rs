//! Launching a harness on a fresh pty, and draining it into the emulator.

use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Weak};

use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};

use super::super::handle::{release_queued, SessionHandle, SessionMeta};
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
        // Before the pty, because it shells out to `git`: one more reason this
        // whole function belongs on a blocking thread.
        let branch = git_branch(&spec.cwd);
        let launch_commit = git_head(&spec.cwd);
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

        // The write half moves onto its own thread below; the session holds
        // only the queue, so no caller ever waits on a child that is not
        // reading. See `SessionIo::writes`.
        let (writes, queued) = channel::<Vec<u8>>();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let now = self.now();
        let id = format!("w_{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));
        let handle = Arc::new(SessionHandle::new(
            SessionMeta {
                id: id.clone(),
                provider: spec.provider,
                cwd: spec.cwd,
                branch,
                launch_commit,
                started_at: now,
                user_spawned: spec.user_spawned,
            },
            spec.label,
            session_id,
            spec.control,
            vt100::Parser::new(DEFAULT_ROWS, DEFAULT_COLS, SCROLLBACK),
            pty.master,
            writes,
            queued_bytes.clone(),
            child,
        ));

        write(&self.inner.sessions).push(handle.clone());

        // Only now: the reader marks output on every read, and a child that
        // greets the pty immediately would otherwise have its first output land
        // before there is a session to record it against — losing the
        // `last_output_at` that idle detection reads.
        self.spawn_writer(Arc::downgrade(&handle), writer, queued, queued_bytes);
        self.spawn_attention_poller(Arc::downgrade(&handle));
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

    /// Drain queued bytes onto the PTY master on a blocking thread.
    ///
    /// The mirror of [`spawn_reader`](Self::spawn_reader), and a thread for a
    /// stronger reason than symmetry: a pty write parks in the kernel until the
    /// child drains its stdin, so on the async runtime it would occupy a worker,
    /// and on the caller it froze whichever thread queued the bytes — the render
    /// thread, for a keystroke. Here it may park as long as it likes with nobody
    /// waiting on it.
    ///
    /// Ends when the session's queue is dropped, which reaping the session or
    /// forgetting it both do. A thread parked mid-write instead unblocks when
    /// the child is killed and the pty closes, which is what `close` and
    /// `shutdown` do.
    ///
    /// Holds a [`Weak`], not an `Arc`: the sender it waits on lives *inside* the
    /// handle, so an owning reference would keep alive the very thing whose drop
    /// is meant to end this loop.
    fn spawn_writer(
        &self,
        handle: Weak<SessionHandle>,
        mut writer: Box<dyn Write + Send>,
        queued: Receiver<Vec<u8>>,
        queued_bytes: Arc<AtomicUsize>,
    ) {
        std::thread::spawn(move || {
            let mut failure = None;
            // `recv` rather than `for`, so the receiver survives the loop and the
            // bytes still waiting can be accounted for below.
            while let Ok(bytes) = queued.recv() {
                let wrote = writer.write_all(&bytes).and_then(|()| writer.flush());
                // Released whether or not it landed: the budget counts what is
                // *waiting*, and these bytes are not waiting any more.
                release_queued(&queued_bytes, bytes.len());
                if let Err(err) = wrote {
                    failure = Some(err.to_string());
                    break;
                }
            }
            // Whatever is still queued will never reach the child. Counted, not
            // dropped in silence: every one of those bytes had a caller that was
            // told `Ok`, and "your paste was slow" and "your paste was lost" are
            // different problems with different fixes.
            let abandoned: usize = queued.try_iter().map(|bytes| bytes.len()).sum();
            // A queue nothing will ever drain must not keep occupying the budget,
            // or a later write would be refused as "full" against bytes that are
            // already gone. Racing a concurrent reservation can only leave this
            // permissive, which is safe: writes to a dead session fail anyway.
            queued_bytes.store(0, Ordering::Release);
            // Only a real failure is worth recording. A session closed normally
            // drops its sender to end this loop, and its handle is usually gone
            // by now anyway.
            let (Some(err), Some(handle)) = (failure, handle.upgrade()) else {
                return;
            };
            let lost = if abandoned > 0 {
                format!(" ({abandoned} queued byte(s) never written)")
            } else {
                String::new()
            };
            handle.record_error(format!("{}: {err}{lost}", handle.id()));
        });
    }
}

/// Resolve the checked-out branch without treating a non-repository or
/// detached `HEAD` as an error.
fn git_branch(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", cwd, "symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty())
}

/// Snapshot the repository commit before the harness can mutate the checkout.
fn git_head(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|head| !head.is_empty())
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
