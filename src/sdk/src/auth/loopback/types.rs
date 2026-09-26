//! Data types for the `loopback` module.
#[allow(unused_imports)]
use super::*;
/// Outcome of classifying one HTTP request received by the loopback accept loop.
/// Extracted as a pure function so routing can be unit-tested without a socket.
#[derive(Debug, PartialEq, Eq)]
pub(in super::super) enum RequestOutcome {
    /// `GET /auth` with a matching `state=` nonce. The caller extracts the
    /// `token`/`error` params from `callback_url` and finishes.
    AuthCallback { callback_url: String },
    /// `/auth` matched but `state=` was missing or wrong. Caller sends 400 and
    /// keeps waiting.
    StateMismatch,
    /// Path is not `/auth`. Caller sends 404 and keeps waiting.
    NotFound,
    /// Method is not GET. Caller sends 405 and keeps waiting.
    MethodNotAllowed,
}
/// A bound loopback listener with its state nonce and the login URL to open.
/// Splitting "start" (immediate: port/url/state) from "await the callback" lets
/// the TUI open the browser and render the waiting screen before blocking, while
/// the CLI drives both back-to-back via [`run_login_flow`].
pub struct LoopbackListener {
    pub(super) listener: TcpListener,
    pub(super) port: u16,
    pub(super) state: String,
    pub(super) login_url: String,
}
