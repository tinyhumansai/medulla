//! Login/logout plumbing: an RFC 8252 loopback OAuth flow against the Medulla
//! backend, a small on-disk credential store, and the pure URL/query helpers the
//! CLI and tests share.
//!
//! The flow: bind an ephemeral loopback port, point the browser at
//! `<baseUrl>/auth/<provider>/login?redirect=app&redirectUri=<loopback>`, and
//! wait for the backend to redirect the browser back to the loopback URI with a
//! ready-to-use JWT (`?token=<jwt>&key=auth`) or an error (`?error=<msg>`).
//!
//! Split by responsibility: [`types`] holds the plain data model, [`store`] the
//! filesystem-backed credential store, [`url`] the pure URL/query helpers,
//! [`token`] backend bearer-token resolution, and [`loopback`] the socket-bound
//! OAuth flow and browser opener. All public items are re-exported here so
//! callers use `medulla::auth::*`.

mod loopback;
mod store;
mod token;
mod types;
mod url;

#[cfg(test)]
mod tests;

pub use loopback::{open_browser, run_login_flow, start_loopback, LoopbackListener};
pub use store::CredentialStore;
pub use token::{is_one_time_login_token, missing_token_note, resolve_backend_token};
pub use types::{Credentials, LoginError, LoopbackConfig, Provider, DEFAULT_LOGIN_TIMEOUT};
pub use url::{describe_me, login_url, random_state_nonce, redirect_uri};
