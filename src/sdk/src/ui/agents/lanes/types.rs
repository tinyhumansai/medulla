//! Data types for the `lanes` module.
#[allow(unused_imports)]
use super::*;
/// Insertion-ordered lane collection.
pub(super) struct Lanes {
    pub(super) order: Vec<String>,
    pub(super) map: HashMap<String, AgentLane>,
}
