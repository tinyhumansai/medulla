//! Which harness and model an `agent` node runs on.
//!
//! An `agent` node is a task dispatched to a coding harness, and until now the
//! harness and the model were whatever the *host* pinned in `workflows.config`.
//! That is the wrong grain for a real workflow: a plan may want a cheap model to
//! triage and an expensive one to implement, or Codex for one step and Claude
//! Code for the next, and neither is an operator-wide setting.
//!
//! So a preference can be stated at three layers, and this module is the one
//! place that says how they combine:
//!
//! 1. **The node** — `config.harness` and `config.model` on an `agent` node.
//! 2. **The workflow** — the document's `defaults` block.
//! 3. **The host** — `workflows.defaultProvider` / `workflows.defaultModel`.
//!
//! Resolution is *paired*, not field-by-field: whichever layer names the harness
//! also supplies the model, unless a layer above it named one explicitly. A node
//! that says `harness: codex` and nothing else therefore runs Codex on Codex's
//! own default model, rather than inheriting a Claude model id from the host and
//! handing it to the wrong CLI — which is the mistake field-by-field precedence
//! would make on every mixed-harness workflow.
//!
//! Harness names are read from *config*, never from model output, for the same
//! reason `agent_ref` is: choosing the harness is choosing which binary and
//! which credentials run the next instruction.

use serde_json::Value;

use crate::protocol::{dispatchable_flavors, HarnessProvider, HarnessTransport};

/// The config keys an `agent` node states a harness under, in the order they
/// are consulted. `provider` is accepted because that is what the wire frame
/// and the host config call the same thing.
const HARNESS_KEYS: [&str; 2] = ["harness", "provider"];

/// The longest custom-harness id this will accept.
///
/// Custom harness ids cross the fleet protocol and land in process environments,
/// so a bound belongs here as much as in the config layer that mints them.
const MAX_CUSTOM_HARNESS_LEN: usize = 64;

/// A harness named by an author: one of the built-in coding CLIs, or a custom
/// preset the worker exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessSelector {
    /// A built-in coding harness: one of the CLIs, or a non-default flavor of
    /// one such as `codex-server`.
    ///
    /// The transport rides here rather than in a separate field because the two
    /// are named together — an author writes one word, `codex` or
    /// `codex-server`, and it resolves to one pair.
    Builtin {
        /// The provider the flavor runs.
        provider: HarnessProvider,
        /// The transport it runs over.
        transport: HarnessTransport,
    },
    /// A named custom harness preset configured on the worker that runs the
    /// task. Whether the preset exists is the worker's to answer: presets are
    /// per-host config, and a graph authored on one machine may legitimately
    /// name one that only another machine has.
    Custom(String),
}

impl HarnessSelector {
    /// Parse a harness name.
    ///
    /// A name that matches a built-in CLI is that CLI; anything else is taken
    /// as a custom preset id, provided it is short and made of the wire-safe
    /// characters the config layer requires of preset ids.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the correction when the value is empty or
    /// cannot be a preset id. The reader is usually an agent with one round trip
    /// to spend, so the message lists what *is* accepted.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!(
                "`harness` is empty — name one of {}, or the id of a custom harness preset this \
                 host exposes. Omit the field entirely to use the host default.",
                Self::builtin_names().join(", ")
            ));
        }
        if let Some((provider, transport)) =
            HarnessProvider::flavor_from_wire(&value.to_ascii_lowercase())
        {
            if provider.is_dispatchable() {
                return Ok(Self::Builtin {
                    provider,
                    transport,
                });
            }
            return Err(format!(
                "`harness` is `{value}`, which is a built-in harness but cannot run a workflow \
                 agent node — use {}, or a custom harness preset id.",
                Self::builtin_names().join(", ")
            ));
        }
        if value.len() > MAX_CUSTOM_HARNESS_LEN {
            return Err(format!(
                "`harness` is {} characters — a custom harness id may be at most \
                 {MAX_CUSTOM_HARNESS_LEN}.",
                value.len()
            ));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "`harness` is `{value}`, which is neither a built-in harness ({}) nor a usable \
                 custom harness id — preset ids are ASCII letters, digits, `-`, `_`, and `.` \
                 only.",
                Self::builtin_names().join(", ")
            ));
        }
        Ok(Self::Custom(value.to_string()))
    }

    /// The built-in harness names, for error messages and host facts.
    ///
    /// Every dispatchable flavor, not merely every provider, so `codex-server`
    /// appears wherever an author is told what they may write.
    pub fn builtin_names() -> Vec<&'static str> {
        dispatchable_flavors()
            .into_iter()
            .map(|(provider, transport)| provider.flavor_name(transport))
            .collect()
    }

    /// The name this selector was written as.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Builtin {
                provider,
                transport,
            } => provider.flavor_name(*transport),
            Self::Custom(id) => id.as_str(),
        }
    }
}

/// One layer's stated preference. Both halves are optional: a layer may pin a
/// model without caring which harness runs it, or the reverse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessPreference {
    /// The harness this layer asks for.
    pub harness: Option<HarnessSelector>,
    /// The model hint this layer asks for.
    pub model: Option<String>,
}

impl HarnessPreference {
    /// A preference stated as JSON config — an `agent` node's `config`, or a
    /// workflow document's `defaults` block. Both use the same key names, so an
    /// author moving between them is not caught out.
    ///
    /// Absent keys, JSON `null`, and blank strings all mean "this layer says
    /// nothing", so an author can clear a field by blanking it rather than
    /// having to delete the key.
    ///
    /// # Errors
    ///
    /// Returns a sentence when `harness` is present but is not a string, or
    /// names something that cannot be a harness.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let harness = match harness_field(config) {
            Some((key, value)) => match value.as_str() {
                Some(text) if text.trim().is_empty() => None,
                Some(text) => Some(HarnessSelector::parse(text)?),
                None => {
                    return Err(format!(
                        "`{key}` must be a string naming a harness, not {}.",
                        json_type_name(value)
                    ))
                }
            },
            None => None,
        };
        let model = config
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string);
        Ok(Self { harness, model })
    }

    /// Whether this layer says nothing at all.
    pub fn is_empty(&self) -> bool {
        self.harness.is_none() && self.model.is_none()
    }
}

/// A resolved choice, in the shape a task frame carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessChoice {
    /// The built-in harness to run on, when one was chosen.
    pub provider: Option<HarnessProvider>,
    /// The flavor of `provider` to run it over, when a built-in was chosen.
    ///
    /// Always `Some` beside a `provider` and always `None` without one, because
    /// the two are resolved from the same selector. A custom preset states its
    /// own transport through its base harness, so this stays `None` there.
    pub transport: Option<HarnessTransport>,
    /// The custom harness preset to run on, when one was named.
    pub custom_harness: Option<String>,
    /// The model hint to send with the dispatch.
    pub model: Option<String>,
}

impl HarnessChoice {
    /// Resolve `layers` — most specific first — into one choice.
    ///
    /// The harness comes from the first layer that names one. The model comes
    /// from the first layer that names one *at or above* that layer, so a model
    /// pinned below the chosen harness is left behind: it was chosen for a
    /// different harness and would be meaningless, or wrong, on this one.
    ///
    /// With no layer naming a harness, every layer may still supply the model —
    /// there is no harness switch to invalidate it.
    pub fn resolve(layers: &[HarnessPreference]) -> Self {
        let chosen = layers.iter().position(|layer| layer.harness.is_some());
        let model_depth = chosen.unwrap_or(layers.len().saturating_sub(1));
        let model = layers
            .iter()
            .take(model_depth + 1)
            .find_map(|layer| layer.model.clone());
        let harness = chosen.and_then(|index| layers[index].harness.clone());
        let (provider, transport, custom_harness) = match harness {
            Some(HarnessSelector::Builtin {
                provider,
                transport,
            }) => (Some(provider), Some(transport), None),
            Some(HarnessSelector::Custom(id)) => (None, None, Some(id)),
            None => (None, None, None),
        };
        Self {
            provider,
            transport,
            custom_harness,
            model,
        }
    }
}

/// The harness key a config states, with the value it states.
fn harness_field(config: &Value) -> Option<(&'static str, &Value)> {
    HARNESS_KEYS
        .iter()
        .find_map(|key| config.get(*key).map(|value| (*key, value)))
        .filter(|(_, value)| !value.is_null())
}

/// A JSON value's kind, for an error message that names what was written.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
#[path = "harness_choice_tests.rs"]
mod tests;
