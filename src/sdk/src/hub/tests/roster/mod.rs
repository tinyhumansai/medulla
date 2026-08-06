//! Tests for the hub roster: how a worker is advertised, addressed, and kept
//! unique.
//!
//! The roster is the only thing standing between an orchestrator's `agentId`
//! and a link address, so these pin the resolution rules rather than the
//! transport — dispatch itself is covered in [`super::dispatch`].
//!
//! Split by the surface each group pins, so no file approaches the repo's
//! 500-line ceiling: [`strategy`] picks *which* worker a route lands on,
//! [`advert`] pins the payload that names it, [`dedupe`] keeps one peer to one
//! slot and withholds the unreachable, [`roles`] carries an agent's declared
//! roles and placement into the advert, and [`topology`] pins the host block
//! those agents hang off. Fixtures shared across them live in [`helpers`].

mod helpers;

mod advert;
mod dedupe;
mod roles;
mod strategy;
mod topology;
