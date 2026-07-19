//! A scripted, self-contained [`Runtime`] used by `main` until the backend
//! runtime lands, and by tests. It fabricates a plausible event stream so every
//! tab has something to render.
//!
//! Split by responsibility: [`types`] holds the in-memory state model, the
//! [`MockRuntime`] handle, and its trivial construction/scripting seams;
//! [`runtime_impl`] carries the [`Runtime`] trait implementation; and
//! [`scenario`] scripts the populated demo world. The public surface is
//! re-exported here so callers use `medulla::runtime::mock::MockRuntime`.
//!
//! [`Runtime`]: crate::runtime::Runtime

mod runtime_impl;
mod scenario;
mod types;

#[cfg(test)]
mod tests;

pub use types::MockRuntime;
