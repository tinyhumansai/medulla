//! View data shared by workflow node preview renderers.

use medulla::config::WorkflowsConfig;
use medulla::flow_engine::{HarnessChoice, HarnessPreference, HarnessSelector};
use medulla::workflows::WorkflowDefaults;
use serde_json::Value;

/// What is shown when nothing at any layer names a harness or a model: the
/// worker decides, and this pane cannot know what it will pick.
const WORKER_DECIDES: &str = "worker default";

/// Effective routing defaults used to explain an authored agent node.
///
/// Holds the two layers *beneath* a node — the workflow's own `defaults` block
/// and the host's configuration — rather than one flattened answer, because
/// which layer a step's harness came from is the thing an operator is reading
/// this pane to find out.
#[derive(Debug, Default)]
pub(super) struct AgentDefaults {
    /// Worker used when a node does not name an `agent_ref`.
    pub(super) worker: String,
    /// The workflow's own choice, as a preference layer.
    workflow: HarnessPreference,
    /// The host's configured choice.
    host: HarnessPreference,
}

/// The harness and model a step will actually run on, and where that came from.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct EffectiveHarness {
    /// Display name of the harness — a product name for a built-in, the preset
    /// id for a custom harness.
    pub(super) harness: String,
    /// The model hint, or [`WORKER_DECIDES`].
    pub(super) model: String,
    /// Which layer decided the harness, phrased for an operator.
    pub(super) source: &'static str,
    /// Set when the node's own `harness` cannot be read as one. Authoring is
    /// refused, so this only shows for a file edited by hand — which is exactly
    /// when saying so is worth a line.
    pub(super) unreadable: Option<String>,
}

/// A readable rendering of a prompt assembled by a workflow expression.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PromptTemplate {
    /// Literal prompt text with escaped newlines decoded and bindings inserted.
    pub(super) text: String,
    /// Short descriptions of the workflow values appended to the prompt.
    pub(super) inputs: Vec<String>,
}

impl AgentDefaults {
    /// Build the layers beneath a node from the host's config and the workflow's
    /// own `defaults` block.
    pub(super) fn new(config: &WorkflowsConfig, defaults: &WorkflowDefaults) -> Self {
        Self {
            worker: config.default_worker.clone(),
            // A `defaults` block the store would have refused cannot be
            // displayed as a choice, so an unreadable one is shown as no choice
            // at all rather than as the host's.
            workflow: medulla::workflows::defaults_preference(defaults).unwrap_or_default(),
            host: HarnessPreference {
                harness: config
                    .default_provider
                    .map(|provider| HarnessSelector::Builtin {
                        provider,
                        // Host config pins a provider, not a flavor — matching
                        // `CapabilitySettings::harness_preference`, which this
                        // preview stands in for.
                        transport: medulla::protocol::HarnessTransport::Cli,
                    }),
                model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
            },
        }
    }

    /// Resolve `config` — one `agent` node's own configuration — against these
    /// layers, exactly the way a dispatch does.
    pub(super) fn resolve(&self, config: &Value) -> EffectiveHarness {
        let (node, unreadable) = match HarnessPreference::from_config(config) {
            Ok(node) => (node, None),
            // The node named something that is not a harness. Its *model* is
            // still worth resolving, so only the harness half is dropped.
            Err(message) => (
                HarnessPreference {
                    harness: None,
                    model: config
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
                Some(message),
            ),
        };
        let source = if unreadable.is_some() || node.harness.is_some() {
            "this step"
        } else if self.workflow.harness.is_some() {
            "workflow default"
        } else if self.host.harness.is_some() {
            "host default"
        } else {
            WORKER_DECIDES
        };
        let choice = HarnessChoice::resolve(&[node, self.workflow.clone(), self.host.clone()]);
        EffectiveHarness {
            harness: choice
                .provider
                .map(|provider| provider.display_name().to_string())
                .or(choice.custom_harness)
                .unwrap_or_else(|| WORKER_DECIDES.to_string()),
            model: choice.model.unwrap_or_else(|| WORKER_DECIDES.to_string()),
            source,
            unreadable,
        }
    }
}
