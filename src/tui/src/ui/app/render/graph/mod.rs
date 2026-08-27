//! The animated signal graph on the Subconscious tab.
//!
//! This module connects the graph model, simulation, character surface, and
//! renderer used by the Subconscious signal field.

pub(crate) mod charmap;
pub(crate) mod layout;
pub(crate) mod mock;
mod renderer;
#[cfg(test)]
mod tests;
mod types;

#[allow(unused_imports)]
pub(crate) use types::{Edge, EdgeKind, Graph, Node, NodeKind};
