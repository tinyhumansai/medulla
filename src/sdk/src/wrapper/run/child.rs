//! Spawning the harness child, and the uniform handle the run loop drives it
//! through.
//!
//! Two stdio strategies exist, and [`spawn_child`] picks between them:
//!
//! - **PTY** — used when input injection is active, a real terminal is attached,
//!   and the app supplied a [`PtySpawner`]. The child gets its own
//!   pseudo-terminal, so a full-screen TUI (Codex, Claude) sees a tty on stdin
//!   and runs normally while we still hold a writable handle for injected owner
//!   messages. The PTY itself is allocated app-side; see
//!   [`PtySpawner`](crate::wrapper::PtySpawner).
//! - **Inherited / piped stdio** — everything else. With no injection the child
//!   simply inherits our stdio; with injection but no tty (tests, headless runs)
//!   stdin is a plain pipe, which is all a non-interactive harness needs.
//!
//! Both paths surface the same [`ChildSession`], so the select loop in [`super`]
//! never branches on which one is live.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

use super::super::types::{PtyRequest, WrapperConfig};

/// A spawned harness child, uniform across the PTY and inherited-stdio paths.
///
/// The run loop destructures this rather than calling methods on it, so each
/// field can be borrowed independently inside `tokio::select!`.
pub(super) struct ChildSession {
    /// Sink for bytes typed into the child. `Some` only when input injection is
    /// active; [`drain_and_inject`](super::super::bridge::drain_and_inject)
    /// writes inbound owner messages here.
    pub(super) input: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Resolves with the shell-style exit code once the child has exited.
    pub(super) done: oneshot::Receiver<i32>,
    /// Sending on this kills the child; consumed on first use.
    pub(super) kill: Option<oneshot::Sender<()>>,
    /// Resolves once the PTY reader has copied the child's final output to our
    /// stdout. `None` on the inherited-stdio path, where the child wrote to the
    /// real stdout directly and nothing is buffered on our side.
    pub(super) drained: Option<oneshot::Receiver<()>>,
    /// Returns the terminal to cooked mode (PTY path only). Called by the run
    /// loop once the child's output has drained.
    pub(super) restore: Option<Box<dyn FnOnce() + Send>>,
}

/// Spawn `bin` with `args` under `config`, choosing the stdio strategy.
///
/// `inject` is the bridge's `receive_active`: whether inbound owner messages
/// must be typed into the child. When it is set, stdin is a terminal, and
/// `config.pty_spawner` is present, the child is given a PTY; if allocating one
/// fails we warn and fall back to inherited stdin rather than failing the
/// session outright — a running harness without remote input beats no harness.
pub(super) fn spawn_child(
    bin: &str,
    args: &[String],
    config: &mut WrapperConfig,
    inject: bool,
) -> anyhow::Result<ChildSession> {
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if inject && interactive {
        if let Some(spawn_pty) = config.pty_spawner.take() {
            let request = PtyRequest {
                bin: bin.to_string(),
                args: args.to_vec(),
                cwd: config.cwd.clone(),
                env: config.env.clone(),
            };
            match spawn_pty(request) {
                Ok(harness) => {
                    return Ok(ChildSession {
                        input: Some(harness.input),
                        done: harness.done,
                        kill: Some(harness.kill),
                        drained: Some(harness.drained),
                        restore: Some(harness.restore),
                    })
                }
                Err(err) => {
                    eprintln!(
                        "medulla wrapper: could not allocate a pty ({err}); \
                         falling back to inherited stdin (remote input is disabled)"
                    );
                    return spawn_stdio(bin, args, config, false);
                }
            }
        }
    }
    spawn_stdio(bin, args, config, inject)
}

/// Spawn the child on inherited stdio, piping stdin only when `inject` is set.
///
/// A piped stdin here means no terminal is attached, or no PTY spawner was
/// supplied — either way there is no local keystroke forwarding to do, so the
/// pipe carries injected messages and nothing else.
fn spawn_stdio(
    bin: &str,
    args: &[String],
    config: &WrapperConfig,
    inject: bool,
) -> anyhow::Result<ChildSession> {
    let mut command = Command::new(bin);
    command
        .args(args)
        .envs(&config.env)
        .current_dir(&config.cwd)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(if inject {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    let mut child = command
        .spawn()
        .map_err(|err| anyhow::anyhow!("failed to start {bin}: {err}"))?;

    // A single task owns the pipe so injected writes never interleave.
    let input = if inject {
        child.stdin.take().map(|mut child_stdin| {
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
            tokio::spawn(async move {
                while let Some(bytes) = rx.recv().await {
                    if child_stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = child_stdin.flush().await;
                }
            });
            tx
        })
    } else {
        None
    };

    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<i32>();
    tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status,
            _ = kill_rx => {
                let _ = child.start_kill();
                child.wait().await
            }
        };
        let _ = done_tx.send(status.map(exit_code).unwrap_or(1));
    });

    Ok(ChildSession {
        input,
        done: done_rx,
        kill: Some(kill_tx),
        drained: None,
        restore: None,
    })
}

/// Translate a child [`ExitStatus`](std::process::ExitStatus) into a shell-style
/// exit code (`128 + signal` for signal termination on Unix).
pub(super) fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}
