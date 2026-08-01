//! Launching a harness on a fresh pty, and draining it into the emulator.

use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use super::super::launch::{interactive_args, mint_session_id};
use super::super::types::{
    LaunchSpec, PtySession, PtyState, SessionRow, DEFAULT_COLS, DEFAULT_ROWS, SCROLLBACK,
};

use super::{PtyManager, BUF_LEN, OPENPTY_ATTEMPTS, OPENPTY_RETRY_PAUSE};

impl PtyManager {
    /// Launch a harness on a fresh PTY and start draining it.
    ///
    /// Returns the new session's id. The child is started immediately — unlike
    /// the headless session model there is no lazy handle, because the whole
    /// point is to have a screen to look at.
    pub fn open(&self, spec: LaunchSpec) -> Result<String, String> {
        let branch = git_branch(&spec.cwd);
        let size = PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut attempt = 1;
        let pty = loop {
            match native_pty_system().openpty(size) {
                Ok(pty) => break pty,
                Err(err) if attempt < OPENPTY_ATTEMPTS => {
                    attempt += 1;
                    std::thread::sleep(OPENPTY_RETRY_PAUSE);
                    let _ = err;
                }
                Err(err) => {
                    return Err(format!(
                        "could not allocate a pty after {OPENPTY_ATTEMPTS} attempts: {err}"
                    ))
                }
            }
        };

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

        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            DEFAULT_ROWS,
            DEFAULT_COLS,
            SCROLLBACK,
        )));
        // The write half moves onto its own thread below; the session holds only
        // the queue, so no caller can block on the child while holding the
        // manager's lock. See `PtySession::writes`.
        let (writes, queued) = channel::<Vec<u8>>();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let now = self.now();
        let id = format!("w_{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));

        self.inner.sessions.lock().unwrap().push(PtySession {
            row: SessionRow {
                id: id.clone(),
                label: spec.label,
                provider: spec.provider,
                state: PtyState::Running,
                cwd: spec.cwd,
                branch,
                session_id,
                started_at: now,
                last_output_at: now,
                last_error: None,
                // Opened because a turn is about to run in it. Claimed here so a
                // concurrent task cannot take it in the gap before that turn
                // starts.
                //
                // Not so for an operator-spawned session: nothing is about to
                // run in it, it is sitting at a prompt waiting to be typed in.
                // Leaving it claimed would make the rail read "busy" for a
                // harness that is plainly idle.
                busy: !spec.user_spawned,
                control: spec.control,
                user_spawned: spec.user_spawned,
                // Nothing has painted yet, so there is nothing to be waiting on.
                attention: None,
            },
            screen: screen.clone(),
            master: pty.master,
            writes,
            queued_bytes: queued_bytes.clone(),
            seen_bells: 0,
            attention_generation: 0,
            suppress_next_bell: false,
            attention_checked_at: now,
            child: Some(child),
        });

        // Only now: the reader `touch`es the session on every read, and a child
        // that greets the pty immediately would otherwise have its first output
        // land before there is a session to record it against — losing the
        // `last_output_at` that idle detection reads. The writer records its
        // failures against the row, so it needs the record to exist for the same
        // reason.
        self.spawn_reader(id.clone(), reader, screen);
        self.spawn_writer(id.clone(), writer, queued, queued_bytes);
        // Watches the screen for the question that stops this harness — see
        // `spawn_attention_poller` for why it is a timer and not the reader.
        self.spawn_attention_poller(id.clone());

        Ok(id)
    }

    /// Drain the PTY master into the emulator on a blocking thread.
    fn spawn_reader(
        &self,
        id: String,
        mut reader: Box<dyn Read + Send>,
        screen: Arc<Mutex<vt100::Parser>>,
    ) {
        let manager = self.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; BUF_LEN];
            loop {
                match reader.read(&mut buf) {
                    // EOF: the child closed the pty. Its last screen stays
                    // readable — the operator usually wants to see how it ended.
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        screen.lock().unwrap().process(&buf[..n]);
                        manager.touch(&id);
                    }
                }
            }
            manager.mark_finished(&id);
        });
    }

    /// Drain queued bytes onto the PTY master on a blocking thread.
    ///
    /// The mirror of [`spawn_reader`](Self::spawn_reader), and a thread for a
    /// stronger reason than symmetry: a pty write parks in the kernel until the
    /// child drains its stdin, so on the async runtime it would occupy a worker,
    /// and on a caller holding the manager's lock it froze every render frame.
    /// Here it may park as long as it likes with nobody waiting on it.
    ///
    /// Ends when the session's queue is dropped — which removing the session
    /// record does. A thread parked mid-write instead unblocks when the child is
    /// killed and the pty closes, which is what `close` and `shutdown` do.
    fn spawn_writer(
        &self,
        id: String,
        mut writer: Box<dyn Write + Send>,
        queued: Receiver<Vec<u8>>,
        queued_bytes: Arc<AtomicUsize>,
    ) {
        let manager = self.clone();
        std::thread::spawn(move || {
            let mut failure = None;
            // `recv` rather than `for`, so the receiver survives the loop and the
            // bytes still waiting can be accounted for below.
            while let Ok(bytes) = queued.recv() {
                let wrote = writer.write_all(&bytes).and_then(|()| writer.flush());
                // Released whether or not it landed: the budget counts what is
                // *waiting*, and these bytes are not waiting any more.
                queued_bytes.fetch_sub(bytes.len(), Ordering::AcqRel);
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
            // drops its sender to end this loop, and its record is usually gone
            // by now anyway.
            if let Some(err) = failure {
                let lost = if abandoned > 0 {
                    format!(" ({abandoned} queued byte(s) never written)")
                } else {
                    String::new()
                };
                manager.record_write_error(&id, &format!("{id}: {err}{lost}"));
            }
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
