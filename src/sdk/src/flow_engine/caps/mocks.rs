//! Capability stand-ins for dry runs.
//!
//! The engine ships mocks that echo their request back. That is enough to prove
//! a graph *executes*, but not that it is correct: a node declaring an
//! `output_parser.schema` will have its echoed response fail validation, so a
//! perfectly good graph fails a simulation for a reason that has nothing to do
//! with the graph. The sibling `openhuman` host hit exactly that and answered it
//! with schema-aware mocks; these are the same idea.
//!
//! A dry run therefore means: every expression resolved, every node's declared
//! output shape was satisfiable, and nothing left the process.

use async_trait::async_trait;
use serde_json::{json, Value};
use tinyflows::caps::{AgentRunner, LlmProvider};
use tinyflows::error::Result;

/// Synthesize a value satisfying a JSON Schema well enough to pass validation.
///
/// Deliberately shallow — it honours `type`, `properties`, `required`, and
/// `enum`, which is what node schemas in practice use. Anything it does not
/// understand becomes null, and a schema strict enough to reject that is a
/// schema whose graph deserves a real run before being trusted.
pub fn sample_for_schema(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return Value::Null;
    };
    if let Some(first) = object
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
    {
        return first.clone();
    }
    match object.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut out = serde_json::Map::new();
            if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                // Every declared property, not only the required ones: a graph
                // binding `=item.json.optional_field` should still resolve.
                for (name, property) in properties {
                    out.insert(name.clone(), sample_for_schema(property));
                }
            }
            Value::Object(out)
        }
        Some("array") => match object.get("items") {
            // One element, so a downstream `per_item` node has something to map
            // over and a `[0]` expression resolves.
            Some(items) => json!([sample_for_schema(items)]),
            None => json!([]),
        },
        Some("string") => json!("sample"),
        Some("integer") | Some("number") => json!(0),
        Some("boolean") => json!(false),
        _ => Value::Null,
    }
}

/// The `output_parser.schema` a request declares, if any.
fn declared_schema(request: &Value) -> Option<&Value> {
    request.get("output_parser")?.get("schema")
}

/// The response a schema-aware mock returns for `request`.
fn mock_response(request: &Value, source: &str) -> Value {
    match declared_schema(request) {
        Some(schema) => {
            let sample = sample_for_schema(schema);
            json!({
                "text": serde_json::to_string(&sample).unwrap_or_default(),
                "json": sample,
                "mock": source,
            })
        }
        None => json!({
            "text": format!("[{source} dry run]"),
            "json": Value::Null,
            "mock": source,
        }),
    }
}

/// An [`LlmProvider`] whose response satisfies the node's declared schema.
pub struct SchemaAwareMockLlm;

#[async_trait]
impl LlmProvider for SchemaAwareMockLlm {
    async fn complete(&self, request: Value, _conn: Option<&str>) -> Result<Value> {
        Ok(mock_response(&request, "llm"))
    }
}

/// An [`AgentRunner`] whose response satisfies the node's declared schema.
///
/// Dispatches nothing: the whole point of a dry run is that no harness session
/// is started and no repository is touched.
pub struct SchemaAwareMockAgentRunner;

#[async_trait]
impl AgentRunner for SchemaAwareMockAgentRunner {
    async fn run_agent(
        &self,
        agent_ref: &str,
        request: Value,
        _conn: Option<&str>,
    ) -> Result<Value> {
        let mut response = mock_response(&request, "agent");
        if let Some(object) = response.as_object_mut() {
            // Recorded so a dry run's output shows *which* worker each node
            // would have gone to — the thing an author most often gets wrong.
            object.insert("agent_ref".into(), Value::String(agent_ref.to_string()));
        }
        Ok(response)
    }
}
