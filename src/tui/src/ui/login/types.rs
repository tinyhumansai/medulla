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
    ///
    /// No longer offered on the login menu — signing in is the only way into the
    /// app — but kept because the pre-app loop still falls back to the mock when
    /// a verified token cannot reach the backend.
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
    /// Redeem/verify a pasted API key, JWT, or 64-hex one-time token.
    SubmitToken(String),
    /// Open `url` in the platform browser. Fire-and-forget: the screen stays put.
    OpenUrl(String),
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

/// One row of the Idle menu.
///
/// Sign-in providers and the non-provider actions share a single list so the
/// whole screen is navigated one way — arrow keys and Enter — rather than by
/// remembering a letter per action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuItem {
    /// Start the browser loopback flow with this provider.
    Provider(Provider),
    /// Switch to the key-entry phase.
    PasteKey,
    /// Open the documentation in the browser. Does not leave the screen.
    Docs,
    /// Open the GitHub repository in the browser. Does not leave the screen.
    Star,
    /// Leave with [`LoginOutcome::Quit`].
    Quit,
}

impl MenuItem {
    /// The row's label.
    pub(super) fn label(self) -> &'static str {
        match self {
            MenuItem::Provider(Provider::Google) => "Continue with Google",
            MenuItem::Provider(Provider::Github) => "Continue with GitHub",
            MenuItem::Provider(Provider::Twitter) => "Continue with X (Twitter)",
            // Not reachable from MENU; kept so the match stays exhaustive.
            MenuItem::Provider(Provider::Discord) => "Continue with Discord",
            MenuItem::PasteKey => "Paste an API key instead",
            MenuItem::Docs => "Read the docs",
            MenuItem::Star => "Star us on GitHub",
            MenuItem::Quit => "Quit",
        }
    }
}

/// The Idle menu, in display order: every sign-in provider first, then the
/// fallbacks and the exit.
///
/// `Provider::Discord` exists in the wire enum but the backend has no Discord
/// login, so it is deliberately absent — offering a row that cannot succeed is
/// worse than not offering it.
pub(super) const MENU: [MenuItem; 7] = [
    MenuItem::Provider(Provider::Google),
    MenuItem::Provider(Provider::Github),
    MenuItem::Provider(Provider::Twitter),
    MenuItem::PasteKey,
    MenuItem::Docs,
    MenuItem::Star,
    MenuItem::Quit,
];

/// Where "Read the docs" points.
pub(super) const DOCS_URL: &str = "https://tinyhumans.gitbook.io/medulla";

/// Where "Star us on GitHub" points.
pub(super) const REPO_URL: &str = "https://github.com/tinyhumansai/medulla";

/// The index of the first non-provider row, where the menu draws a separator.
pub(super) const MENU_ACTIONS_START: usize = 3;

/// Where the screen currently is in the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// The provider/action menu.
    Idle,
    /// A `StartLoopback` was issued; awaiting `LoopbackStarted`.
    Starting,
    /// The loopback listener is live; browser round-trip in progress.
    Waiting,
    /// A focused single-line API-key / token input.
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
    /// The highlighted row of the Idle menu (index into [`MENU`]).
    pub(super) menu_index: usize,
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
            menu_index: 0,
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
