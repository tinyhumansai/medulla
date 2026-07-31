//! The Medulla home directory and the early `.env` loader.
//!
//! Everything Medulla persists — credentials, TUI state, the tiny.place
//! identity, and the layered config file — lives under a single home directory
//! resolved by [`medulla_home`].
//!
//! # Two levels, not one
//!
//! [`medulla_root`] is the install-wide directory (`~/.medulla`). It holds
//! nothing but one directory per account and the [`user`] marker that names the
//! active one. [`medulla_home`] is that account's directory — `<root>/<user
//! id>` — and is what every other module means by "the Medulla home". Two
//! accounts on one machine therefore share no config, no logs, no workflow
//! store, and no core state; nothing needs to be account-aware to get that,
//! because the scoping happens once, here.
//!
//! Before anyone signs in the id is [`user::PRE_LOGIN_USER_ID`], so a
//! signed-out install still has a complete, real home.
//!
//! # Layout
//!
//! - [`resolve`] — the root and home precedence chain, pure over an env map.
//! - [`user`] — the active-account marker that chooses between account
//!   directories, and the only input the environment cannot supply.
//! - [`dotenv`] — the `.env` loader that runs before any of the above reads the
//!   environment.

pub mod dotenv;
pub mod resolve;
pub mod user;

pub use dotenv::{apply_dotenv, load_dotenv_from_cwd, parse_dotenv};
pub use resolve::{is_truthy, medulla_home, medulla_root};

#[cfg(test)]
mod tests;
