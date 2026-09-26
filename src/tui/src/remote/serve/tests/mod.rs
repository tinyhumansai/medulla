//! Tests for the serving loop, driven over an in-process bridge.
//!
//! Split by responsibility rather than left as one file, per this repo's
//! per-file line ceiling (`AGENTS.md`):
//!
//! - [`support`] — the fixtures the rest share: a `LocalSessions` that can start
//!   a real shell, a server-and-client bridge pair, and the settle helpers.
//! - [`sessions`] — what the server does for a connected client. Pty-backed.
//! - [`config`] — that a host launches with its own configuration.
//! - [`bootstrap`] — address families, endpoint formatting, and the bootstrap
//!   lock. Pure.

mod bootstrap;
mod config;
mod sessions;
mod support;
