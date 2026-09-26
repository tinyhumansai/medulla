//! Typed models shared by the public worker-roster and task-program endpoints.

mod types;

pub use types::*;
pub(crate) use types::{TaskPayload, TaskSourcePayload};
