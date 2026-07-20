//! Runs a wrapped coding-agent CLI on a real pseudo-terminal.
//!
//! This is the app-side half of [`medulla::wrapper::PtySpawner`]. The SDK needs
//! a writable handle on the harness's input so it can inject messages arriving
//! from tiny.place, but a full-screen TUI refuses to run with a pipe on stdin
//! (Codex exits with `stdin is not a terminal`). Allocating a PTY satisfies
//! both: the child sees a tty and drives the screen normally, while we keep the
//! master side to write into.
//!
//! Keeping this in the app crate is deliberate — it is process and terminal
//! wiring, so the SDK stays free of `portable-pty` and `crossterm`, exactly as
//! it stays free of the onboarding UI.
//!
//! Responsibilities while the session runs:
//! - copy the master's output to our stdout (the child renders itself);
//! - forward our stdin to the master, so keystrokes reach the child untouched;
//! - put our own terminal in raw mode, so the child's line discipline — not
//!   ours — owns echo, Ctrl-C, and cursor keys;
//! - forward `SIGWINCH` to the PTY so the child reflows on window resize.

use std::io::{Read, Write};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::{mpsc, oneshot};

use medulla::wrapper::{PtyHarness, PtyRequest, PtySpawner};

/// Read buffer for both stdin and the PTY master. Large enough that a burst of
/// full-screen redraw output is copied in a handful of syscalls.
const BUF_LEN: usize = 8192;

/// Fallback geometry when the terminal size cannot be queried.
const FALLBACK_COLS: u16 = 80;
/// Fallback geometry when the terminal size cannot be queried.
const FALLBACK_ROWS: u16 = 24;

/// Build the [`PtySpawner`] handed to [`medulla::wrapper::run_wrapper`].
pub fn spawner() -> PtySpawner {
    Box::new(spawn)
}

/// Restores cooked mode on drop, so a panic between here and the wrapper's
/// teardown still leaves the operator with a usable terminal.
struct RawGuard;

impl RawGuard {
    /// Enter raw mode. Failure is not fatal: the PTY session still works, the
    /// operator just gets local echo, so we degrade rather than refuse to run.
    fn enter() -> Self {
        let _ = enable_raw_mode();
        RawGuard
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// The current terminal geometry, or a conventional 80x24 if it is unknown.
fn terminal_size() -> PtySize {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((FALLBACK_COLS, FALLBACK_ROWS));
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Launch `request` on a fresh pseudo-terminal and wire up its I/O.
///
/// Returns once the child is running; the returned [`PtyHarness`] carries the
/// handles the wrapper drives it with. Errors if the PTY cannot be allocated or
/// the binary cannot be executed, in which case the SDK falls back to inherited
/// stdio.
fn spawn(request: PtyRequest) -> anyhow::Result<PtyHarness> {
    let pair = native_pty_system().openpty(terminal_size())?;

    let mut cmd = CommandBuilder::new(&request.bin);
    for arg in &request.args {
        cmd.arg(arg);
    }
    cmd.cwd(&request.cwd);
    for (key, value) in &request.env {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd)?;
    // Drop our handle on the slave: once the child's own copy closes, the master
    // sees EOF and the reader thread can finish.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;

    // Raw mode and keystroke forwarding only make sense against a real terminal.
    // The wrapper only reaches for a PTY when stdin is one, so this is a guard
    // against direct callers (tests) rather than a case that arises in the app.
    // Entering raw mode only after the child is live also means an early spawn
    // failure never leaves the terminal altered.
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let guard = interactive.then(RawGuard::enter);

    // PTY master -> our stdout. `drained` lets the wrapper wait for the child's
    // final frame before restoring the terminal.
    let (drained_tx, drained_rx) = oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut buf = [0u8; BUF_LEN];
        let mut out = std::io::stdout();
        loop {
            // A closed master reads as EOF on Linux and EIO on macOS; both mean
            // the child is gone, so any error ends the copy.
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = out.flush();
                }
            }
        }
        let _ = drained_tx.send(());
    });

    // A single writer thread owns the master, so injected messages and local
    // keystrokes are serialized instead of interleaving mid-sequence.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Some(bytes) = input_rx.blocking_recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    // Our stdin -> the same channel. In raw mode this is an unbuffered byte
    // stream, so control characters and escape sequences pass through intact.
    if interactive {
        let keystrokes = input_tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; BUF_LEN];
            let mut stdin = std::io::stdin();
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if keystrokes.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    // Window resizes must reach the child or its layout stays stale. The task
    // owns the master handle and lives for the rest of the process; the reader
    // and writer hold their own duplicated descriptors, so this is the only
    // thing keeping `master` alive and dropping it at exit is harmless.
    spawn_resize_forwarder(pair.master);

    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    let mut killer = child.clone_killer();
    tokio::spawn(async move {
        if kill_rx.await.is_ok() {
            let _ = killer.kill();
        }
    });

    // `child.wait()` is blocking, so it gets its own thread rather than a
    // runtime worker.
    let (done_tx, done_rx) = oneshot::channel::<i32>();
    std::thread::spawn(move || {
        let code = match child.wait() {
            // `exit_code` reports 0 for signal deaths too, so trust `success()`
            // over the raw number when they disagree.
            Ok(status) => match (status.success(), status.exit_code() as i32) {
                (true, _) => 0,
                (false, 0) => 1,
                (false, code) => code,
            },
            Err(_) => 1,
        };
        let _ = done_tx.send(code);
    });

    Ok(PtyHarness {
        input: input_tx,
        done: done_rx,
        kill: kill_tx,
        drained: drained_rx,
        restore: Box::new(move || drop(guard)),
    })
}

/// Forward terminal resizes to `master` for the lifetime of the process.
#[cfg(unix)]
fn spawn_resize_forwarder(master: Box<dyn portable_pty::MasterPty + Send>) {
    use tokio::signal::unix::{signal, SignalKind};

    tokio::spawn(async move {
        let Ok(mut winch) = signal(SignalKind::window_change()) else {
            return;
        };
        while winch.recv().await.is_some() {
            let _ = master.resize(terminal_size());
        }
    });
}

/// No `SIGWINCH` equivalent off Unix; the child keeps its initial geometry.
#[cfg(not(unix))]
fn spawn_resize_forwarder(_master: Box<dyn portable_pty::MasterPty + Send>) {}
