//! Data returned while preparing hook delivery through ACP.

use serde_json::{Map, Value};

/// What an ACP-dispatched spawn carries so the operator's hooks run, and what it
/// could not carry.
///
/// Only one delivery channel is modelled because only one exists: the session
/// `_meta`. The rest of the type is the honest half — a spawn where nothing
/// could be installed still produces [`Self::notes`], which is what keeps
/// "your hook is not running here" from being a silence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpDelivery {
    /// The `_meta` object for `session/new` and `session/load`, when the
    /// provider takes its configuration that way.
    ///
    /// A `serde_json` map rather than an `agent_client_protocol` type on
    /// purpose — that crate's `Meta` *is* this type, and spelling it this way
    /// keeps the hook vocabulary free of a transport dependency it otherwise
    /// has no use for.
    pub session_meta: Option<Map<String, Value>>,
    /// Operator-facing notes: hooks this transport could not install, and why.
    pub notes: Vec<String>,
}

impl AcpDelivery {
    /// Whether this delivery changes anything about the spawn.
    ///
    /// Notes alone do not count: a delivery that only explains why nothing was
    /// installed has nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.session_meta.is_none()
    }
}
