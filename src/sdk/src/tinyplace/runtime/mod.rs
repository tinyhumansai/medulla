//! Agent-runtime helpers layered on the tinyplace SDK client.
//!
//! These are thin async orchestrations of SDK calls — no PTY/provider spawning,
//! no rendering. They give the TUI/daemon the pieces it needs to stay live on
//! tiny.place:
//!
//! - [`FileSessionStore`] — a filesystem [`SessionStore`](::tinyplace::signal::store::SessionStore)
//!   persisting Signal ratchet/pre-key state as JSON, laid out to coexist with
//!   the TS SDK's `FileSessionStore`.
//! - [`load_or_create_identity`] — load-or-mint a 32-byte Ed25519 seed via the
//!   SDK signer, persisted to the tinyplace CLI config file (`secretKey` hex).
//! - [`acquire_identity`] — the same bootstrap under an exclusive OS lock, over a
//!   pool of identity slots, so two daemons on one machine cannot end up sharing
//!   an address (and therefore an inbox and a Signal ratchet).
//! - [`spawn_mailbox_poll`] — poll + destructively read DMs, decoding task
//!   frames, over a tokio channel.
//! - [`spawn_contact_auto_accepter`] — poll contact requests and accept via a
//!   fail-closed allowlist.
//! - [`spawn_presence_heartbeat`] — keep the identity marked online.
//!
//! Split by responsibility: [`types`] holds the error/mailbox surface and the
//! on-disk JSON shapes, [`identity`] the seed bootstrap, [`identity_pool`] the
//! locked slot acquisition layered over it, [`session_store`] the
//! file-backed [`SessionStore`](::tinyplace::signal::store::SessionStore) adapter,
//! and [`poll`] the background poll loops. All public items are re-exported here
//! so callers use `medulla::tinyplace::runtime::*`.

mod identity;
mod identity_pool;
mod poll;
mod session_store;
mod types;

#[cfg(test)]
mod tests;

pub use identity::load_or_create_identity;
pub use identity_pool::{
    acquire_identity, acquire_identity_at, AcquiredIdentity, IdentityLock, IDENTITY_FILE,
};
pub use poll::{spawn_contact_auto_accepter, spawn_mailbox_poll, spawn_presence_heartbeat};
pub use session_store::FileSessionStore;
pub use types::{MailboxItem, MailboxPoll, RuntimeError, RuntimeResult};
