//! Medulla’s in-process OpenHuman adapter. Runtime lifetime belongs to the host;
//! each worker/thread gets an agent and each run gets scoped tools and hooks.

mod budget;
mod environment;
mod hooks;
mod host;
mod model_selection;
#[cfg(feature = "workflows")]
mod native_session;
mod permissions;
mod routing;
mod run;
#[cfg(feature = "workflows")]
mod tools;

#[cfg(test)]
mod tests;

pub use host::EmbedHost;
pub use model_selection::effective_model;
pub use routing::embedded_route;
pub use run::{run_local_task, uses_local_harness};
