//! Login plumbing: two OAuth flows against the Medulla backend, and the pure
//! URL/query helpers the CLI and tests share.
//!
//! **Loopback (RFC 8252)** — for a terminal on the same machine as a browser:
//! bind an ephemeral loopback port, point the browser at
//! `<baseUrl>/auth/<provider>/login?redirect=app&redirectUri=<loopback>`, and
//! wait for the backend to redirect the browser back to the loopback URI with a
//! ready-to-use JWT (`?token=<jwt>&key=auth`) or an error (`?error=<msg>`).
//!
//! **Code (terminal)** — for SSH sessions and anywhere else the browser is on a
//! different machine, where loopback cannot work at all: the redirect would
//! reach the *browser* host's `127.0.0.1`, never the listener. So there is no
//! listener. [`code_login_url`] points at
//! `<baseUrl>/auth/<provider>/login?redirect=cli`, which the operator opens on
//! any device; the backend ends that round-trip on a page showing a one-time
//! login token, and the operator pastes it back into the terminal, where
//! [`crate::client::MedullaClient::consume_login_token`] exchanges it for a JWT.
//! Both flows converge on the same verified JWT.
//!
//! # Where the session is kept
//!
//! In [`session`], under the account-scoped Medulla home. Both flows above end
//! at a verified JWT; `session::store` checks it against `/auth/me` once more
//! and writes it, and every reader takes it from there.
//!
//! This module used to defer that to the embedded OpenHuman core, because two
//! stores meant two answers to "am I signed in?" — `medulla login` could succeed
//! while the core, whose session drove the runtime, stayed signed out. The core
//! is gone; owning the store satisfies the same invariant directly.
//!
//! Split by responsibility: `types` holds the plain data model, `session` the
//! store and its `AuthState` projection, `migrate` the adoption of credential
//! files written before the store moved, `url` the pure URL/query helpers,
//! `token` backend bearer-token resolution, and `loopback` the socket-bound
//! OAuth flow and browser opener. All public items are re-exported here so
//! callers use `medulla::auth::*`.

mod loopback;
mod migrate;
mod session;
mod token;
mod types;
mod url;

#[cfg(test)]
mod tests;

pub use loopback::{open_browser, run_login_flow, start_loopback, LoopbackListener};
pub use migrate::{
    adopt_legacy_credentials, discard_legacy_credential, LegacyCredential,
    LEGACY_CONFIG_DIR_OVERRIDE,
};
pub use session::{
    adopt_legacy, clear, read, state, store, token as session_token, AuthState, StoredSession,
};
// Re-exported under a name that says what it is from outside this module: the
// owner-only atomic write the session store uses, reused by any surface
// persisting something that should not be world-readable.
pub use session::write_private as write_private_file;
pub use token::{external_token_wins, is_one_time_login_token, resolve_backend_token};
pub use types::{Credentials, LoginError, LoopbackConfig, Provider, DEFAULT_LOGIN_TIMEOUT};
pub use url::{
    code_login_url, describe_me, login_url, random_state_nonce, redirect_uri, user_id_from_me,
};
