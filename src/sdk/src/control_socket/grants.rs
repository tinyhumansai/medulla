//! Grants: the capability a spawned harness presents to reach the fleet.
//!
//! This is what makes the control plane *exclusive* rather than merely
//! filesystem-private. Medulla mints a token immediately before spawning a
//! harness and hands it to that one child in its environment. Everything the
//! holder may do — how deep in a dispatch tree it sits, which tool families it
//! may call, how many tasks it may hold at once — is recorded here and looked up
//! by token.
//!
//! The consequence worth being explicit about: **the grant is the authority, and
//! nothing the caller says about itself is.** A model that rewrites its own
//! environment, or sends a request claiming a shallower depth, changes nothing —
//! the server never reads those. It can only present a token that means less.
//! That is the difference between a depth cap that holds and one that a single
//! confused turn can talk its way past.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::types::ToolFamilies;

/// What one spawned harness is permitted to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The task or session this was minted for, used to revoke it later.
    pub session: String,
    /// How deep in the dispatch tree its holder sits, counted from the
    /// operator's own turn.
    pub depth: u8,
    /// The depth at which dispatching is withheld.
    ///
    /// Carried on the grant rather than read from config at check time so a
    /// mid-session config edit cannot retroactively widen a running harness.
    pub max_depth: u8,
    /// Which tool families the holder may call.
    pub families: ToolFamilies,
    /// The most concurrent dispatches the holder may have in flight.
    pub max_in_flight: usize,
}

impl Grant {
    /// A grant for a harness spawned by `session` at `depth`.
    pub fn new(session: impl Into<String>, depth: u8, max_depth: u8) -> Self {
        Grant {
            session: session.into(),
            depth,
            max_depth,
            families: ToolFamilies::default(),
            max_in_flight: 4,
        }
    }

    /// Set which tool families this grant covers.
    pub fn with_families(mut self, families: ToolFamilies) -> Self {
        self.families = families;
        self
    }

    /// Set the concurrent-dispatch ceiling.
    pub fn with_max_in_flight(mut self, max_in_flight: usize) -> Self {
        self.max_in_flight = max_in_flight.max(1);
        self
    }

    /// Whether the holder may dispatch at all.
    ///
    /// False either because it was never granted the fleet family or because it
    /// has reached the depth ceiling. Both cases withhold `fleet_dispatch` from
    /// the advertised tool list rather than refusing it on call.
    pub fn may_dispatch(&self) -> bool {
        self.families.fleet && self.depth < self.max_depth
    }

    /// The depth a harness *this* grant's holder dispatches would sit at.
    pub fn child_depth(&self) -> u8 {
        self.depth.saturating_add(1)
    }
}

/// The live set of grants this instance has minted.
///
/// Cheap to clone; every clone shares one table.
#[derive(Clone, Default)]
pub struct GrantRegistry {
    inner: Arc<Mutex<HashMap<String, Grant>>>,
}

impl GrantRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a token for `grant` and return it.
    ///
    /// 32 random bytes rendered as hex, from the same CSPRNG that backs v4
    /// UUIDs. The token is the only copy: it is handed to exactly one child
    /// process and never written to disk or logged.
    pub fn mint(&self, grant: Grant) -> String {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        if let Ok(mut grants) = self.inner.lock() {
            grants.insert(token.clone(), grant);
        }
        token
    }

    /// What `token` is permitted to do, or `None` if it is not a live grant.
    ///
    /// The lookup is not constant-time. That is a deliberate non-goal: reaching
    /// this code at all requires connecting to a `0600` socket inside a `0700`
    /// directory, so an attacker positioned to measure it can already read the
    /// token from the environment of the process they would be attacking.
    pub fn redeem(&self, token: &str) -> Option<Grant> {
        self.inner.lock().ok()?.get(token).cloned()
    }

    /// Drop every grant minted for `session`.
    ///
    /// Called when an ACP session ends. Tasks the holder already dispatched keep
    /// running — killing a working agent because the turn that asked for it
    /// finished would discard real work — but nothing can poll or extend them
    /// through this grant any more; they settle into the operator's task view.
    pub fn revoke(&self, session: &str) {
        if let Ok(mut grants) = self.inner.lock() {
            grants.retain(|_, grant| grant.session != session);
        }
    }

    /// How many grants are live. For diagnostics and tests.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether no grants are live.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
