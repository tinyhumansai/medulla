//! `tool_call` dispatch across two namespaces.
//!
//! A slug beginning with [`NATIVE_TOOL_PREFIX`] names an operation this host
//! implements itself; anything else is a third-party integration, permitted only
//! when an operator has listed it. The prefix is what lets one `ToolInvoker`
//! multiplex both without a slug ever being ambiguous — the same shape the
//! sibling `openhuman` host uses for its own native tools.
//!
//! The engine's node-kind set is closed, so a Medulla-specific step *is* a
//! `tool_call` with a `medulla:` slug. That is the extension point.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tinyflows::caps::ToolInvoker;
use tinyflows::error::{EngineError, Result};

use crate::flow_engine::settings::CapabilitySettings;

/// The prefix marking a slug this host implements natively.
pub const NATIVE_TOOL_PREFIX: &str = "medulla:";

/// The native operations a workflow may call.
///
/// Small on purpose: a workflow's real work happens in `agent` nodes dispatched
/// to a harness, and every native tool added here is surface a workflow author
/// can reach without an operator's say-so.
pub const NATIVE_TOOLS: [&str; 2] = ["medulla:echo", "medulla:now"];

/// Whether a slug names a native operation.
pub fn is_native(slug: &str) -> bool {
    slug.starts_with(NATIVE_TOOL_PREFIX)
}

/// A [`ToolInvoker`] serving native operations and refusing everything not
/// explicitly allowed.
pub struct MedullaToolInvoker {
    settings: Arc<CapabilitySettings>,
}

impl MedullaToolInvoker {
    /// An invoker bound to `settings`.
    pub fn new(settings: Arc<CapabilitySettings>) -> Self {
        Self { settings }
    }

    /// Run one native operation.
    fn native(&self, slug: &str, args: Value) -> Result<Value> {
        match slug {
            // Echoes its arguments. The graph author's smoke test: it proves a
            // `tool_call` node is wired and its expressions resolve, without
            // reaching anything outside the process.
            "medulla:echo" => Ok(json!({ "echo": args })),
            // The host's clock, so a workflow does not have to trust a node's
            // idea of "now" or shell out for it.
            "medulla:now" => Ok(json!({ "epoch_ms": crate::clock::now_millis() })),
            _ => Err(EngineError::Capability(format!(
                "tool_call: unknown native tool '{slug}'; known: {}",
                NATIVE_TOOLS.join(", ")
            ))),
        }
    }
}

#[async_trait]
impl ToolInvoker for MedullaToolInvoker {
    async fn invoke(&self, slug: &str, args: Value, _conn: Option<&str>) -> Result<Value> {
        if is_native(slug) {
            return self.native(slug, args);
        }
        if !self.settings.tool_allowed(slug) {
            return Err(EngineError::Capability(format!(
                "tool_call: '{slug}' is not in the configured tool allowlist"
            )));
        }
        // A slug can be allowlisted before this host knows how to run it. Say so
        // plainly rather than failing as though the allowlist rejected it — the
        // operator did their part.
        Err(EngineError::Capability(format!(
            "tool_call: '{slug}' is allowed but this host has no integration registry to run it"
        )))
    }
}

/// A [`ToolInvoker`] decorator that checks arguments before delegating.
///
/// Wrapping the engine's *mock* invoker with this is what makes a dry run worth
/// anything: the mock will happily echo a call whose arguments never resolved,
/// so a graph with a broken `=nodes.x.…` expression would pass a simulation and
/// fail in production. Checking here catches it at authoring time.
pub struct PreflightToolInvoker {
    inner: Arc<dyn ToolInvoker>,
}

impl PreflightToolInvoker {
    /// Wrap `inner` with argument preflighting.
    pub fn new(inner: Arc<dyn ToolInvoker>) -> Self {
        Self { inner }
    }
}

/// Field paths whose value is null, which in a resolved config means an
/// expression pointed at something that was not there.
fn unresolved_fields(args: &Value) -> Vec<String> {
    let Some(object) = args.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .filter(|(_, value)| value.is_null())
        .map(|(name, _)| name.clone())
        .collect()
}

#[async_trait]
impl ToolInvoker for PreflightToolInvoker {
    async fn invoke(&self, slug: &str, args: Value, conn: Option<&str>) -> Result<Value> {
        let unresolved = unresolved_fields(&args);
        if !unresolved.is_empty() {
            return Err(EngineError::Capability(format!(
                "tool_call '{slug}': {} resolved to null — check the expressions binding them",
                unresolved.join(", ")
            )));
        }
        self.inner.invoke(slug, args, conn).await
    }
}
