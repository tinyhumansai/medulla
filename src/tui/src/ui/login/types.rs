//! Login-screen data model: the terminal [`LoginOutcome`], the async
//! [`LoginCmd`]/[`LoginEvent`] messages exchanged with `main`, the internal
//! [`Phase`] state, and the [`LoginScreen`] struct with its trivial
//! constructor and accessors. The state machine lives in the sibling `state`
//! module and rendering in `draw`.

use medulla::auth::Provider;

/// The terminal outcome of the login screen, consumed by the `main` pre-app loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// A verified JWT — proceed into the main app with a backend runtime.
    Token(String),
    /// Continue offline with the mock runtime.
    Mock,
    /// Quit cleanly without starting the app.
    Quit,
}

/// An async action the pre-app loop must run on the screen's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginCmd {
    /// Bind the loopback listener, open the browser, and await the callback.
    StartLoopback {
        base_url: String,
        provider: Provider,
    },
    /// Abort a running loopback task (Esc while waiting).
    CancelLoopback,
    /// Redeem/verify a pasted JWT or 64-hex one-time token.
    SubmitToken(String),
}

/// An event fed back from a spawned async task into [`LoginScreen::apply`].
#[derive(Debug, Clone)]
pub enum LoginEvent {
    /// The loopback listener is bound; show the URL and waiting spinner.
    LoopbackStarted { url: String, port: u16 },
    /// A JWT was captured from the loopback callback (verification pending).
    CallbackToken(String),
    /// The loopback flow failed (backend error, state-mismatch timeout, …).
    CallbackError(String),
    /// A JWT was verified via `me()`; `who` is the `describe_me` summary.
    Verified { jwt: String, who: String },
    /// Verification (or token redemption) failed.
    VerifyFailed(String),
}

/// Where the screen currently is in the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// The provider/action menu.
    Idle,
    /// A `StartLoopback` was issued; awaiting `LoopbackStarted`.
    Starting,
    /// The loopback listener is live; browser round-trip in progress.
    Waiting,
    /// A focused single-line token input.
    TokenEntry,
    /// A captured/pasted token is being verified.
    Verifying,
}

/// The pure login-screen state machine.
///
/// Fields are `pub(super)` so the sibling `state` and `draw` modules (which hold
/// the behaviour-heavy `impl` blocks) can read and mutate them; nothing outside
/// the `login` module tree sees them.
pub struct LoginScreen {
    pub(super) base_url: String,
    pub(super) provider: Provider,
    pub(super) phase: Phase,
    pub(super) url: Option<String>,
    pub(super) port: Option<u16>,
    pub(super) input: String,
    pub(super) error: Option<String>,
    pub(super) flash: Option<String>,
    pub(super) frame: usize,
    pub(super) outcome: Option<LoginOutcome>,
}

impl LoginScreen {
    /// A fresh screen for `base_url`, starting on the provider menu.
    pub fn new(base_url: impl Into<String>) -> Self {
        LoginScreen {
            base_url: base_url.into(),
            provider: Provider::default(),
            phase: Phase::Idle,
            url: None,
            port: None,
            input: String::new(),
            error: None,
            flash: None,
            frame: 0,
            outcome: None,
        }
    }

    /// The terminal outcome, once the screen has reached one.
    pub fn outcome(&self) -> Option<LoginOutcome> {
        self.outcome.clone()
    }

    /// The currently-selected provider.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Advance the spinner (called on the pre-app loop tick).
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
}
