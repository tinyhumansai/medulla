//! [`PtyManager`] — owns every live harness PTY and its terminal emulator.
//!
//! One session is: a real `claude`/`codex`/`opencode` child on a pseudo-terminal,
//! a reader thread draining the master into a [`vt100::Parser`], and the write
//! half kept open so keystrokes and injected peer prompts can reach the child.
//!
//! The reader runs on a **blocking thread**, not a tokio task: `portable-pty`'s
//! reader is a synchronous `Read` with no async variant, and parking it on the
//! async runtime would occupy a worker forever. It feeds the shared emulator and
//! exits when the master closes.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use medulla::tinyplace::HarnessProvider;

use super::launch::{interactive_args, mint_session_id};
use super::types::{
    LaunchSpec, PtySession, PtyState, SessionRow, DEFAULT_COLS, DEFAULT_ROWS, SCROLLBACK,
};

/// Read buffer for the PTY master, sized for a full-screen redraw burst.
const BUF_LEN: usize = 8192;

/// How many times to retry a failed `openpty` before giving up.
///
/// Pty allocation is a shared, finite system resource, so it can fail
/// transiently when several processes open sessions at once — on a busy build
/// machine, or simply when a peer's tasks arrive in a burst. Mirrors the
/// ETXTBSY spawn retry the headless executor already carries for the same class
/// of momentary failure.
const OPENPTY_ATTEMPTS: u32 = 20;
/// Pause between `openpty` retries.
const OPENPTY_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(25);

/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Owns the live harness sessions the worker TUI renders.
///
/// Cheap to clone (an `Arc`), so the daemon's inbound-frame path and the render
/// loop share one.
#[derive(Clone)]
pub struct PtyManager {
    inner: Arc<Inner>,
}

struct Inner {
    /// Sessions in open order, so the list does not reshuffle under the cursor.
    sessions: Mutex<Vec<PtySession>>,
    next_id: AtomicU64,
    now: NowFn,
}

/// Kill every surviving child when the last handle goes away.
///
/// A pty and its harness outlive the manager otherwise, because neither
/// `portable-pty`'s `Child` nor the master fd terminates the process on drop.
/// Relying on an explicit `shutdown()` makes that a discipline the panic path
/// does not follow — and each leaked session holds a pty device, which the OS
/// has a fixed supply of.
impl Drop for Inner {
    fn drop(&mut self) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return; // poisoned: another thread panicked, nothing safe to do here
        };
        for session in sessions.iter_mut() {
            if let Some(child) = session.child.as_mut() {
                let _ = child.kill();
                // Reap it: a killed child left unwaited is a zombie holding its
                // slot until this process exits.
                let _ = child.wait();
            }
        }
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        PtyManager::new()
    }
}

impl PtyManager {
    /// Build an empty manager on the system clock.
    pub fn new() -> Self {
        PtyManager {
            inner: Arc::new(Inner {
                sessions: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                now: Arc::new(medulla::clock::now_millis),
            }),
        }
    }

    /// Override the clock (tests).
    pub fn with_now(now: NowFn) -> Self {
        PtyManager {
            inner: Arc::new(Inner {
                sessions: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                now,
            }),
        }
    }

    fn now(&self) -> i64 {
        (self.inner.now)()
    }

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

    /// Record that a session produced output.
    fn touch(&self, id: &str) {
        let now = self.now();
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.last_output_at = now;
        }
    }

    /// Reap a session whose PTY has closed and record its exit status.
    ///
    /// The child is taken out of the record and waited on with the lock
    /// **released**. EOF on the pty master and the child's exit are not
    /// simultaneous, so `wait()` can block for a moment — and holding the
    /// manager's lock across it stalls every render frame, which shows up as the
    /// whole TUI freezing when a session ends.
    fn mark_finished(&self, id: &str) {
        let child = {
            let mut sessions = self.inner.sessions.lock().unwrap();
            match sessions.iter_mut().find(|s| s.row.id == id) {
                Some(session) => session.child.take(),
                None => return,
            }
        };
        let code = child
            .and_then(|mut child| child.wait().ok())
            .map(|status| status.exit_code() as i32);

        let now = self.now();
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.state = PtyState::Exited { code };
            session.row.last_output_at = now;
        }
    }

    /// Every session, open order — the list pane's rows.
    /// Take an idle session for `label` on `provider`, marking it busy.
    ///
    /// Find-and-claim under one lock, deliberately. Checking `busy` and then
    /// setting it in two steps lets two concurrent tasks both observe the same
    /// idle session and both take it — which is precisely the collision this
    /// exists to prevent, and it would show up only under a real fan-out.
    ///
    /// `None` when there is no idle session, and the caller opens a fresh one.
    pub fn claim_idle(&self, label: &str, provider: HarnessProvider) -> Option<SessionRow> {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter_mut().find(|s| {
            s.row.label == label
                && s.row.provider == provider
                && s.row.state.is_running()
                && !s.row.busy
        })?;
        session.row.busy = true;
        Some(session.row.clone())
    }

    /// Mark a session free for the next turn.
    pub fn release(&self, id: &str) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.busy = false;
        }
    }

    pub fn rows(&self) -> Vec<SessionRow> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.row.clone())
            .collect()
    }

    /// One session's row by id.
    pub fn row(&self, id: &str) -> Option<SessionRow> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.row.id == id)
            .map(|s| s.row.clone())
    }

    /// How many sessions are still running.
    pub fn running_count(&self) -> usize {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.row.state.is_running())
            .count()
    }

    /// Whether the child has turned bracketed-paste mode on (DECSET 2004).
    ///
    /// We are this child's terminal, so this is not a preference to guess at: a
    /// real terminal sends `ESC[200~` markers only to an application that asked
    /// for them, and sending them to one that did not delivers the escape bytes
    /// as literal keystrokes. It doubles as the readiness signal — a harness
    /// sets its terminal modes when its input layer comes up, so `true` means
    /// there is something listening to type at.
    ///
    /// `None` when the session is unknown.
    pub fn bracketed_paste(&self, id: &str) -> Option<bool> {
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.row.id == id)?;
        let parser = session.screen.lock().unwrap();
        Some(parser.screen().bracketed_paste())
    }

    /// Render `id`'s current screen as `(rows_of_cells, cursor)`.
    ///
    /// Returns owned rows rather than a borrow of the emulator: the render pass
    /// must not hold the parser's lock while the reader thread wants it.
    pub fn screen_rows(&self, id: &str) -> Option<ScreenSnapshot> {
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.row.id == id)?;
        let parser = session.screen.lock().unwrap();
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let cells = (0..rows)
            .map(|row| {
                (0..cols)
                    .map(|col| {
                        screen
                            .cell(row, col)
                            .map(|cell| ScreenCell {
                                text: {
                                    let contents = cell.contents();
                                    if contents.is_empty() {
                                        " ".to_string()
                                    } else {
                                        contents
                                    }
                                },
                                fg: cell.fgcolor(),
                                bg: cell.bgcolor(),
                                bold: cell.bold(),
                                italic: cell.italic(),
                                underline: cell.underline(),
                                inverse: cell.inverse(),
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect();
        Some(ScreenSnapshot {
            cells,
            cursor: screen.cursor_position(),
            hide_cursor: screen.hide_cursor(),
        })
    }

    /// Resize a session's PTY and emulator to `cols` x `rows`.
    ///
    /// Both must move together: the child reflows to the PTY size, so an
    /// emulator of a different size would render a torn screen.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter().find(|s| s.row.id == id) else {
            return;
        };
        {
            let mut parser = session.screen.lock().unwrap();
            if parser.screen().size() == (rows, cols) {
                return; // already correct — skip the SIGWINCH storm
            }
            parser.set_size(rows, cols);
        }
        let _ = session.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Write raw bytes to a session's PTY — the focused pane's keystrokes.
    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return Err(format!("no session {id}"));
        };
        if !session.row.state.is_running() {
            return Err(format!("{id} has exited"));
        }
        use std::io::Write as _;
        session
            .writer
            .write_all(bytes)
            .and_then(|()| session.writer.flush())
            .map_err(|err| format!("{id}: {err}"))
    }

    /// Record the harness session id a tailer read back from the rollout.
    ///
    /// Codex cannot be told an id, so its own is only knowable once it has
    /// written line one of its rollout. Claude's is minted at spawn and never
    /// changes, so this is a no-op there.
    pub fn record_session_id(&self, id: &str, harness_session_id: impl Into<String>) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            if session.row.session_id.is_none() {
                session.row.session_id = Some(harness_session_id.into());
            }
        }
    }

    /// Ask a session's harness to exit, then reap it.
    ///
    /// Sends the child a kill rather than typing `/exit`: the harnesses disagree
    /// on the command, and a session the operator asked to close should not
    /// depend on the model cooperating.
    pub fn close(&self, id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return false;
        };
        if let Some(child) = session.child.as_mut() {
            let _ = child.kill();
        }
        session.row.state = PtyState::Exited { code: None };
        true
    }

    /// Drop an exited session's record and screen.
    ///
    /// Refuses while the child is alive, so a forgotten session can never leave
    /// an orphaned process holding a PTY.
    pub fn forget(&self, id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(index) = sessions
            .iter()
            .position(|s| s.row.id == id && !s.row.state.is_running())
        else {
            return false;
        };
        sessions.remove(index);
        true
    }

    /// Kill every child. Called on shutdown so no harness outlives the TUI.
    pub fn shutdown(&self) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        for session in sessions.iter_mut() {
            if let Some(child) = session.child.as_mut() {
                let _ = child.kill();
            }
        }
    }
}

/// One rendered terminal cell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenCell {
    /// The cell's text (a space when blank).
    pub text: String,
    /// Foreground color.
    pub fg: vt100::Color,
    /// Background color.
    pub bg: vt100::Color,
    /// Whether the cell is bold.
    pub bold: bool,
    /// Whether the cell is italic.
    pub italic: bool,
    /// Whether the cell is underlined.
    pub underline: bool,
    /// Whether foreground/background are swapped.
    pub inverse: bool,
}

/// An owned copy of a session's screen, safe to render without holding the
/// emulator's lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSnapshot {
    /// Rows of cells, top to bottom.
    pub cells: Vec<Vec<ScreenCell>>,
    /// The cursor's `(row, col)`.
    pub cursor: (u16, u16),
    /// Whether the harness has hidden its cursor.
    pub hide_cursor: bool,
}
