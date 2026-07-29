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

use super::script::{run_script, ScriptLanguage};

/// The prefix marking a slug this host implements natively.
pub const NATIVE_TOOL_PREFIX: &str = "medulla:";

/// The native operations a workflow may call.
///
/// Small on purpose: every native tool added here is surface a workflow author
/// can reach without an operator's say-so.
///
/// `medulla:shell` is the exception to that smallness and earns it. A workflow
/// step that runs a script in the operator's project used to be expressible only
/// as an `agent` node — a whole coding session, minutes of wall clock, and a
/// model deciding what to run — for work that is a deterministic command. It is
/// gated on `workflows.allowCode` for exactly the reason `code` nodes are.
pub const NATIVE_TOOLS: [&str; 3] = ["medulla:echo", "medulla:now", "medulla:shell"];

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
    async fn native(&self, slug: &str, args: Value) -> Result<Value> {
        match slug {
            // Echoes its arguments. The graph author's smoke test: it proves a
            // `tool_call` node is wired and its expressions resolve, without
            // reaching anything outside the process.
            "medulla:echo" => Ok(json!({ "echo": args })),
            // The host's clock, so a workflow does not have to trust a node's
            // idea of "now" or shell out for it.
            "medulla:now" => Ok(json!({ "epoch_ms": crate::clock::now_millis() })),
            "medulla:shell" => self.shell(args).await,
            _ => Err(EngineError::Capability(format!(
                "tool_call: unknown native tool '{slug}'; known: {}",
                NATIVE_TOOLS.join(", ")
            ))),
        }
    }

    /// Run a script in the operator's workspace.
    ///
    /// Gated on `allow_code` alongside `code` nodes, because it is the same
    /// decision: this host has no sandbox, so a workflow's script runs with the
    /// daemon's privileges. The difference from a `code` node is *where* — this
    /// one runs in the workspace, because a step that shells out almost always
    /// means to touch the project.
    async fn shell(&self, args: Value) -> Result<Value> {
        if !self.settings.allow_code {
            return Err(EngineError::Capability(
                "medulla:shell is disabled: this host has no sandbox, so a workflow's script \
                 would run with the daemon's full privileges. Enable `workflows.allowCode` only \
                 where that is acceptable."
                    .to_string(),
            ));
        }

        let source = args
            .get("script")
            .and_then(Value::as_str)
            .filter(|source| !source.trim().is_empty())
            .ok_or_else(|| {
                EngineError::Capability(
                    "medulla:shell: `args.script` is required and must be the script to run"
                        .to_string(),
                )
            })?;

        // Defaults to shell because that is what the slug says; naming another
        // language is how a step runs a short node or python program in the
        // workspace rather than in a `code` node's temporary directory.
        let language = match args.get("language").and_then(Value::as_str) {
            Some(name) => ScriptLanguage::parse(name).ok_or_else(|| {
                EngineError::Capability(format!(
                    "medulla:shell: unknown language '{name}'; known: {}",
                    ScriptLanguage::NAMES.join(", ")
                ))
            })?,
            None => ScriptLanguage::Shell,
        };

        let workspace = std::path::Path::new(&self.settings.workspace);
        let output = run_script(
            language,
            source,
            args.get("input").unwrap_or(&Value::Null),
            self.settings.script_timeout(),
            workspace.is_dir().then_some(workspace),
        )
        .await?;

        Ok(json!({
            "output": output.value,
            // Carried rather than dropped: a script that succeeds and warns
            // wrote that warning for a reader, and the run record is the only
            // place a reader will look.
            "stderr": output.stderr,
        }))
    }
}

#[async_trait]
impl ToolInvoker for MedullaToolInvoker {
    async fn invoke(&self, slug: &str, args: Value, _conn: Option<&str>) -> Result<Value> {
        if is_native(slug) {
            return self.native(slug, args).await;
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
