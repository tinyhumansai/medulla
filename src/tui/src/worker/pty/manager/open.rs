//! Launching a harness on a fresh pty, and draining it into the emulator.

use std::io::Read;
use std::sync::atomic::Ordering;
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
        let now = self.now();
        let id = format!("w_{}", self.inner.next_id.fetch_add(1, Ordering::SeqCst));

        self.inner.sessions.lock().unwrap().push(PtySession {
            row: SessionRow {
                id: id.clone(),
                label: spec.label,
                provider: spec.provider,
                state: PtyState::Running,
                cwd: spec.cwd,
                session_id,
                started_at: now,
                last_output_at: now,
                last_error: None,
                // Opened because a turn is about to run in it. Claimed here so a
                // concurrent task cannot take it in the gap before that turn
                // starts.
                busy: true,
            },
            screen: screen.clone(),
            master: pty.master,
            writer,
            child: Some(child),
        });

        // Only now: the reader `touch`es the session on every read, and a child
        // that greets the pty immediately would otherwise have its first output
        // land before there is a session to record it against — losing the
        // `last_output_at` that idle detection reads.
        self.spawn_reader(id.clone(), reader, screen);

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
}
