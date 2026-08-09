//! Data types for the wrapper run loop.

/// The wrapper's own termination-signal source: SIGINT/SIGTERM on Unix, Ctrl-C
/// elsewhere.
///
/// On the PTY path the terminal is in raw mode, so Ctrl-C reaches the child's
/// own line discipline instead of us — exactly as if the harness had been run
/// directly. This only fires for signals sent to the wrapper itself.
///
/// It is a *source*, not a future, because the run loop re-enters its `select!`
/// after a signal fires. A once-built `async` block would be polled again after
/// completing and panic with "`async fn` resumed after completion" — taking the
/// terminal restore, the `session_end` lifecycle event, and the child's kill
/// down with it, and leaving the harness orphaned. [`Signals::recv`] builds a
/// fresh, cancel-safe future on every poll instead; that behaviour lives in
/// [`super`], beside the run loop that drives it.
pub(super) enum Signals {
    /// Unix signal streams, held for the life of the run loop so a signal
    /// arriving between iterations is still delivered.
    #[cfg(unix)]
    Unix {
        /// SIGINT — an operator interrupt.
        sigint: tokio::signal::unix::Signal,
        /// SIGTERM — a supervisor or `kill` asking us to stop.
        sigterm: tokio::signal::unix::Signal,
    },
    /// Non-Unix hosts, where Ctrl-C is the only portable termination signal.
    #[cfg(not(unix))]
    CtrlC,
    /// No handler could be installed. Never fires, so the loop simply runs
    /// until the child exits — the same outcome as before, without a panic.
    Unavailable,
}