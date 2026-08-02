//! The stand-in for a session with no fleet behind it.
//!
//! Costs nothing and touches nothing: a harness on a remote worker, on a host
//! with `[mcp].fleetTools = false`, or on a platform without unix sockets uses
//! this and never learns there was an alternative — the `fleet_*` tools are not
//! advertised, so no call is made to fail.

use serde_json::Value;

use crate::control_socket::{ControlError, Hello};

use super::FleetBackend;

/// A backend that reaches no fleet.
pub struct OfflineFleet;

#[async_trait::async_trait]
impl FleetBackend for OfflineFleet {
    fn hello(&self) -> Option<&Hello> {
        None
    }

    async fn call(&self, _op: &str, _params: Value) -> Result<Value, ControlError> {
        Err(ControlError::NoInstance)
    }
}
