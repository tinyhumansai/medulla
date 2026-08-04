//! The animated workflow graph on the Overview tab.
//!
//! This module connects the graph model, simulation, character surface, and
//! renderer used by the Overview panel.

pub(crate) mod charmap;
pub(crate) mod layout;
pub(crate) mod mock;
mod renderer;
#[cfg(test)]
mod tests;
mod types;

#[allow(unused_imports)]
pub(crate) use types::{Edge, EdgeKind, Graph, Node, NodeKind};
