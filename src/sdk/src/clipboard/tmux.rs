//! tmux awareness: getting a copy out of the multiplexers between this process
//! and the terminal a person is actually looking at.
//!
//! OSC 52 is how a program hands text to its terminal, and it is the only
//! mechanism that survives SSH. A tmux in the middle changes that: tmux *parses*
//! OSC 52 rather than passing it on, and what it then does depends on its
//! `set-clipboard` setting and on whether its own outer terminal advertises the
//! `Ms` capability. Nested tmux is where this usually breaks — the inner
//! server's outer "terminal" is a `screen*`/`tmux*` pane, which does not
//! advertise `Ms`, so the escape is parsed and dropped and the copy silently
//! reaches nobody. That is exactly the shape of a session driven from a laptop
//! tmux, over SSH, into a second tmux on the box.
//!
//! So three things are done at once, because none of them is reliable alone and
//! together they are additive rather than conflicting — every route carries the
//! same text:
//!
//! - the plain [`osc52`](super::osc52) escape, for the no-tmux case and for a
//!   tmux configured with `set-clipboard on`;
//! - the same escape wrapped in tmux's DCS passthrough ([`passthrough`]), which
//!   a tmux with `allow-passthrough` on forwards *verbatim* to whatever is
//!   above it — one hop out of the inner tmux, where an outer tmux then sees a
//!   normal OSC 52 and handles it as it would from any pane;
//! - [`Tmux::load_buffer`], which puts the text in the local server's paste
//!   buffer over the tmux CLI. That needs no terminal capability and no
//!   configuration at all — only a reachable server on the socket in `$TMUX`,
//!   which can still be dead or missing — so it is the route least likely to
//!   need anything else to be set up. Worst case the operator pastes with
//!   tmux's own prefix-`]`. With `-w` tmux also makes its own attempt at the
//!   outer terminal.

/// The tmux server this process is running inside, discovered from `$TMUX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tmux {
    /// Path to the server socket.
    ///
    /// Passed to every `tmux` invocation as `-S`, so a copy lands in the server
    /// this process is actually inside rather than whichever one the default
    /// socket path happens to name — they differ whenever a session was started
    /// with `-L`/`-S`, and a worker reached over SSH is not guaranteed to be in
    /// the default one.
    ///
    /// Private: `$TMUX` is untrusted configuration, so every `Tmux` in
    /// existence has already passed [`Tmux::parse`]'s validation that this
    /// names a real Unix socket this process owns. A public setter would let a
    /// caller build one straight from a tainted value and skip that check.
    socket: String,
}

impl Tmux {
    /// Detect the surrounding tmux from the environment.
    ///
    /// Returns `None` when `$TMUX` is unset or empty, which is the plain
    /// terminal case. `$TMUX` is set by the server for every process it starts,
    /// so it is present for a harness launched inside a pane as well as for the
    /// shell the operator typed in.
    pub fn from_env() -> Option<Self> {
        Self::parse(std::env::var("TMUX").ok().as_deref())
    }

    /// [`Tmux::from_env`] over an explicit value, so the parse is testable
    /// without mutating the process environment.
    ///
    /// `$TMUX` is `<socket>,<server-pid>,<session-index>`; only the socket is
    /// needed, and a value carrying just the socket is accepted too.
    ///
    /// `$TMUX` is inherited environment, so it is untrusted: a child process,
    /// or anything else able to set the environment this process launches
    /// with, could point it at a socket that is not the real controlling
    /// server to redirect where a copy lands. This is validated at that
    /// boundary — accepted only when the path names a Unix socket that
    /// actually exists and is owned by the user running this process — rather
    /// than left to whatever `tmux -S` happens to do with it.
    pub fn parse(value: Option<&str>) -> Option<Self> {
        let socket = value?.split(',').next().unwrap_or_default().trim();
        Self::validated(socket)
    }

    /// Accept `socket` only when it names a real Unix socket owned by us.
    ///
    /// Rejects anything that is not an absolute path, does not exist, is not
    /// a socket (a regular file, say), or belongs to another user — each of
    /// those is a shape a tainted `$TMUX` could take, and none of them is a
    /// server this process should be handing clipboard text to.
    fn validated(socket: &str) -> Option<Self> {
        if socket.is_empty() || !socket.starts_with('/') {
            return None;
        }
        let meta = std::fs::symlink_metadata(socket).ok()?;
        if !is_socket_we_own(&meta) {
            return None;
        }
        Some(Tmux {
            socket: socket.to_string(),
        })
    }

    /// The validated socket path, for tests that need to assert what
    /// [`Tmux::parse`] kept without reaching into the private field.
    #[cfg(test)]
    pub(crate) fn socket(&self) -> &str {
        &self.socket
    }

    /// Build a `Tmux` naming `socket` without the existence/ownership checks
    /// in [`Tmux::validated`].
    ///
    /// Test-only: exercising [`Tmux::load_buffer`] against a socket nothing is
    /// listening on (to assert the "dead server" fallback) requires a path
    /// that predictably fails to validate, which is exactly what production
    /// code must never construct a `Tmux` from.
    #[cfg(test)]
    pub(crate) fn from_raw(socket: impl Into<String>) -> Self {
        Tmux {
            socket: socket.into(),
        }
    }

    /// Put `text` in this server's paste buffer, returning whether tmux took it.
    ///
    /// Tries `-w` first, which additionally asks tmux to hand the buffer to its
    /// own outer terminal, and falls back to a plain load for tmux older than
    /// 3.2 where `-w` is not a valid flag. Never errors: a missing `tmux`
    /// binary or a dead server is just "the buffer did not take it", and the
    /// caller still has the escape and the local writers.
    pub fn load_buffer(&self, text: &str) -> bool {
        super::pipe_to("tmux", &load_buffer_args(&self.socket, true), text)
            || super::pipe_to("tmux", &load_buffer_args(&self.socket, false), text)
    }
}

/// Whether `meta` names a Unix socket owned by the user running this process.
///
/// Both checks matter: the type check rules out a tainted `$TMUX` pointing at
/// an ordinary file, and the ownership check rules out one pointing at a real
/// socket that just happens to belong to someone else on a shared box.
#[cfg(unix)]
fn is_socket_we_own(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    // SAFETY: `getuid` takes no arguments and always succeeds.
    let uid = unsafe { libc::getuid() };
    meta.file_type().is_socket() && meta.uid() == uid
}

/// tmux, and therefore `$TMUX`, does not exist on Windows: there is no socket
/// concept to validate, so nothing this crate is handed there can be a real
/// tmux server.
#[cfg(not(unix))]
fn is_socket_we_own(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Arguments for `tmux load-buffer`, reading the text from stdin.
///
/// `hand_off` adds `-w`, which asks tmux to also send the buffer on to its own
/// outer terminal as an OSC 52. Split out from [`Tmux::load_buffer`] so the
/// command line is asserted in a test rather than only in a live tmux.
pub fn load_buffer_args(socket: &str, hand_off: bool) -> Vec<&str> {
    let mut args = vec!["-S", socket, "load-buffer"];
    if hand_off {
        args.push("-w");
    }
    args.push("-");
    args
}

/// Wrap `seq` so a surrounding tmux forwards it untouched to whatever is above.
///
/// tmux's passthrough is a DCS string: `ESC P tmux ; <payload> ESC \`, with
/// every `ESC` inside the payload doubled so it cannot terminate the DCS early.
/// A tmux with `allow-passthrough` on strips the wrapper and writes the payload
/// straight out; a tmux without it discards the whole DCS; a terminal that is
/// not tmux ignores an unrecognised DCS. So this is safe to emit unconditionally
/// — it either helps or is dropped, and it is never printed as text.
pub fn passthrough(seq: &str) -> String {
    let mut out = String::with_capacity(seq.len() + 16);
    out.push_str("\x1bPtmux;");
    for ch in seq.chars() {
        if ch == '\x1b' {
            out.push('\x1b');
        }
        out.push(ch);
    }
    out.push_str("\x1b\\");
    out
}

/// Everything to write to our own terminal to give `text` to the operator.
///
/// Inside tmux that is the plain escape *and* the passthrough-wrapped copy of
/// it; outside tmux the wrapper buys nothing, so it is left off. See the module
/// docs for why both are sent rather than one being chosen.
pub fn operator_sequence(text: &str, tmux: Option<&Tmux>) -> String {
    let escape = super::osc52(text);
    match tmux {
        Some(_) => format!("{escape}{}", passthrough(&escape)),
        None => escape,
    }
}

/// The text a child's OSC 52 asks to put on the clipboard, if it is a set.
///
/// `params` is the OSC parameter list as the terminal parser splits it —
/// `["52", "c", "<base64>"]`. Returns `None` for anything that is not a
/// clipboard *write* we can forward:
///
/// - a different OSC (not `52`);
/// - a query (`?` as the payload), which asks the terminal to *report* the
///   clipboard back and cannot be answered by passing it upwards;
/// - an empty payload, which is a clear — forwarding it would wipe whatever the
///   operator had rather than hand them anything;
/// - a payload that is not valid base64 of valid UTF-8.
pub fn osc52_from_params(params: &[&[u8]]) -> Option<String> {
    if params.first()? != b"52" {
        return None;
    }
    // The selection field ("c", "p", "s0" …) sits between the number and the
    // payload, but not every emitter sends one; the payload is always last.
    let payload = params.last()?;
    if params.len() < 2 || payload.is_empty() || payload == b"?" {
        return None;
    }
    decode_base64(payload).and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Decode base64 that may or may not carry its padding.
///
/// Terminals accept both, and emitters differ, so a strict engine alone would
/// drop real clipboard writes.
fn decode_base64(payload: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))
        .ok()
}
