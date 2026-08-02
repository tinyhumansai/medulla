//! Login plumbing: an RFC 8252 loopback OAuth flow against the Medulla backend
//! and the pure URL/query helpers the CLI and tests share.
//!
//! The flow: bind an ephemeral loopback port, point the browser at
//! `<baseUrl>/auth/<provider>/login?redirect=app&redirectUri=<loopback>`, and
//! wait for the backend to redirect the browser back to the loopback URI with a
//! ready-to-use JWT (`?token=<jwt>&key=auth`) or an error (`?error=<msg>`).
//!
//! # Where the session is kept
//!
//! Nowhere in this module. This produces a verified JWT; the embedded OpenHuman
//! core stores it (`crate::core_host::auth`) and every reader takes it from
//! there. There used to be a second, Medulla-owned credential file beside the
//! core's own store, which meant two answers to "am I signed in?" — `medulla
//! login` could succeed while the core, whose session actually drives the
//! runtime, stayed signed out.
//!
//! Split by responsibility: `types` holds the plain data model, `migrate`
//! the sweep of the retired credential files, `url` the
//! pure URL/query helpers, `token` backend bearer-token resolution, and
//! `loopback` the socket-bound OAuth flow and browser opener. All public items
//! are re-exported here so callers use `medulla::auth::*`.

mod loopback;
mod migrate;
mod token;
mod types;
mod url;

#[cfg(test)]
mod tests;

pub use loopback::{open_browser, run_login_flow, start_loopback, LoopbackListener};
pub use migrate::remove_retired_credentials;
pub use token::{is_one_time_login_token, resolve_backend_token};
pub use types::{Credentials, LoginError, LoopbackConfig, Provider, DEFAULT_LOGIN_TIMEOUT};
pub use url::{describe_me, login_url, random_state_nonce, redirect_uri, user_id_from_me};
