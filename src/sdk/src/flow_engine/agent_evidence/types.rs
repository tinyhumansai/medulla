//! Prompt evidence storage for one workflow engine invocation.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde_json::Value;

use crate::workflows::RunStep;

use super::NODE_ID_FIELD;

const MAX_PROMPT_BYTES_PER_NODE: usize = 64 * 1024;
const MAX_PROMPTS_PER_NODE: usize = 128;
const TRUNCATED: &str = "[additional prompt evidence truncated]";

/// Resolved agent prompts waiting to be attached to completed run steps.
#[derive(Debug, Default)]
pub(crate) struct AgentEvidence {
    prompts: Mutex<HashMap<String, VecDeque<String>>>,
}

impl AgentEvidence {
    /// Record the prompt from one resolved engine request, when it carries a tag.
    pub(crate) fn record(&self, request: &Value, prompt: &str) {
        let Some(node_id) = request.get(NODE_ID_FIELD).and_then(Value::as_str) else {
            return;
        };
        let mut prompts = self.prompts.lock().expect("agent evidence lock");
        let queue = prompts.entry(node_id.to_string()).or_default();
        if queue.back().is_some_and(|value| value == TRUNCATED) {
            return;
        }
        let used = queue.iter().map(String::len).sum::<usize>();
        if queue.len() >= MAX_PROMPTS_PER_NODE - 1 || used >= MAX_PROMPT_BYTES_PER_NODE {
            queue.push_back(TRUNCATED.to_string());
            return;
        }
        let available = MAX_PROMPT_BYTES_PER_NODE
            .saturating_sub(used)
            .saturating_sub(TRUNCATED.len());
        if prompt.len() <= available {
            queue.push_back(prompt.to_string());
            return;
        }
        let end = prompt
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= available)
            .last()
            .unwrap_or(0);
        if end > 0 {
            queue.push_back(prompt[..end].to_string());
        }
        queue.push_back(TRUNCATED.to_string());
    }

    /// Attach prompts to their corresponding persisted steps in completion order.
    pub(crate) fn attach(&self, steps: &mut [RunStep]) {
        let mut prompts = self.prompts.lock().expect("agent evidence lock");
        let mut remaining = HashMap::<String, usize>::new();
        for step in steps.iter() {
            *remaining.entry(step.node_id.clone()).or_default() += 1;
        }
        for step in steps {
            let Some(node_prompts) = prompts.get_mut(&step.node_id) else {
                continue;
            };
            let steps_left = remaining.get_mut(&step.node_id).expect("step was counted");
            let take = if *steps_left == 1 {
                node_prompts.len()
            } else {
                1.min(node_prompts.len())
            };
            let values = node_prompts
                .drain(..take)
                .map(Value::String)
                .collect::<Vec<_>>();
            step.input = match values.len() {
                0 => None,
                1 => values
                    .into_iter()
                    .next()
                    .map(|value| crate::workflows::bounded_evidence(&value)),
                _ => Some(crate::workflows::bounded_evidence(&Value::Array(values))),
            };
            *steps_left -= 1;
        }
    }
}
