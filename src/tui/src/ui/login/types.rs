//! Login-screen data model: the terminal [`LoginOutcome`], the async
//! [`LoginCmd`]/[`LoginEvent`] messages exchanged with `main`, the internal
//! [`Phase`] state, and the [`LoginScreen`] struct with its trivial
//! constructor and accessors. The state machine lives in the sibling `state`
//! module and rendering in `draw`.

use medulla::auth::Provider;

/// The terminal outcome of the login screen, consumed by the `main` pre-app loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// A verified JWT — sign the embedded core in and proceed into the app.
    Token(String),
    /// Quit cleanly without starting the app.
    ///
    /// The only alternative to signing in. There is deliberately no "continue
    /// offline" outcome: the mock runtime is for tests and the explicit
    /// `--mock` demo, and offering it here would let a failed sign-in land the
    /// operator in a scripted runtime that looks like their own.
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
    /// Redeem/verify a pasted API key, JWT, or 64-hex one-time code. A 64-hex
    /// value is redeemed via `/auth/login-token/consume`; anything else is
    /// treated as a ready-made credential and verified directly.
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

/// How the browser half of a sign-in reaches this terminal.
///
/// The choice is made before the provider because it is the one the operator
/// cannot get wrong by guessing: [`SignInMethod::Browser`] needs a browser on
/// *this* machine, and [`SignInMethod::Code`] works from anywhere. Both end at
/// the same provider list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInMethod {
    /// RFC 8252 loopback: open the browser here and catch the callback on
    /// `127.0.0.1`.
    Browser,
    /// Open a URL on any device, then paste the one-time code it shows back
    /// into this terminal.
    Code,
}

impl SignInMethod {
    /// The heading shown above the provider list once the method is chosen.
    pub(super) fn provider_prompt(self) -> &'static str {
        match self {
            SignInMethod::Browser => "Sign in with a browser — choose a provider",
            SignInMethod::Code => "Sign in with a code — choose a provider",
        }
    }
}

/// One row of the Idle menu.
///
/// The two sign-in methods and the non-sign-in actions share a single list so
/// the whole screen is navigated one way — arrow keys and Enter — rather than by
/// remembering a letter per action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuItem {
    /// Pick a provider, then run that method's flow.
    Method(SignInMethod),
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
            MenuItem::Method(SignInMethod::Browser) => "Sign in with a browser",
            MenuItem::Method(SignInMethod::Code) => "Sign in with a code (SSH / no browser)",
            MenuItem::PasteKey => "Paste an API key",
            MenuItem::Docs => "Read the docs",
            MenuItem::Star => "Star us on GitHub",
            MenuItem::Quit => "Quit",
        }
    }
}

/// The Idle menu, in display order: the two sign-in methods first, then the
/// fallbacks and the exit.
pub(super) const MENU: [MenuItem; 6] = [
    MenuItem::Method(SignInMethod::Browser),
    MenuItem::Method(SignInMethod::Code),
    MenuItem::PasteKey,
    MenuItem::Docs,
    MenuItem::Star,
    MenuItem::Quit,
];

/// The providers offered on the provider step, in display order.
///
/// `Provider::Discord` exists in the wire enum but the backend has no Discord
/// login, so it is deliberately absent — offering a row that cannot succeed is
/// worse than not offering it.
pub(super) const PROVIDERS: [Provider; 3] = [Provider::Google, Provider::Github, Provider::Twitter];

/// The label for one provider row.
pub(super) fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Google => "Google",
        Provider::Github => "GitHub",
        Provider::Twitter => "X (Twitter)",
        Provider::Discord => "Discord",
    }
}

/// Where "Read the docs" points.
pub(super) const DOCS_URL: &str = "https://tinyhumans.gitbook.io/medulla";

/// Where "Star us on GitHub" points.
pub(super) const REPO_URL: &str = "https://github.com/tinyhumansai/medulla";

/// The index of the first non-provider row, where the menu draws a separator.
pub(super) const MENU_ACTIONS_START: usize = 3;

/// Where the screen currently is in the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// The sign-in-method / action menu.
    Idle,
    /// The provider list for the method chosen on [`Phase::Idle`].
    ProviderPick,
    /// A `StartLoopback` was issued; awaiting `LoopbackStarted`.
    Starting,
    /// The loopback listener is live; browser round-trip in progress.
    Waiting,
    /// The terminal flow: the verification URL is shown and the one-time code
    /// it produces is being typed or pasted in.
    CodeEntry,
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
    /// The method chosen on the Idle menu, which decides what the provider step
    /// starts. Retained after the flow begins so a retry after an error repeats
    /// the same one.
    pub(super) method: SignInMethod,
    /// The highlighted row of the Idle menu (index into [`MENU`]).
    pub(super) menu_index: usize,
    /// The highlighted row of the provider step (index into [`PROVIDERS`]).
    pub(super) provider_index: usize,
    pub(super) phase: Phase,
    /// Where a failed verification returns to — the phase that submitted the
    /// value. [`Phase::Idle`] when nothing did (a loopback callback).
    pub(super) resume_phase: Phase,
    /// The URL being shown: the loopback login URL while waiting, or the
    /// verification URL to open on another device during [`Phase::CodeEntry`].
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
            method: SignInMethod::Browser,
            menu_index: 0,
            provider_index: 0,
            phase: Phase::Idle,
            resume_phase: Phase::Idle,
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

    /// The sign-in method chosen on the menu.
    pub fn method(&self) -> SignInMethod {
        self.method
    }

    /// Advance the spinner (called on the pre-app loop tick).
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
}
