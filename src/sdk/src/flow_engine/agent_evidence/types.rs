//! Prompt and transcript evidence storage for one workflow engine invocation.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde_json::Value;

use crate::harness_transcript::TranscriptEntry;
use crate::workflows::RunStep;

use super::NODE_ID_FIELD;

const MAX_PROMPT_BYTES_PER_NODE: usize = 64 * 1024;
const MAX_PROMPTS_PER_NODE: usize = 128;
const TRUNCATED: &str = "[additional prompt evidence truncated]";

/// Resolved agent prompts and harness transcripts waiting to be attached to
/// completed run steps.
///
/// Two queues rather than one keyed pair, because they are filled at different
/// moments: the prompt is known when the request is resolved, the transcript
/// only when the harness has finished. Both drain in the same completion order
/// on [`attach`](Self::attach), which is what keeps the Nth activation of a
/// fanned-out node matched with its own evidence.
#[derive(Debug, Default)]
pub(crate) struct AgentEvidence {
    prompts: Mutex<HashMap<String, VecDeque<String>>>,
    transcripts: Mutex<HashMap<String, VecDeque<Vec<TranscriptEntry>>>>,
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

    /// Record the transcript one dispatch of `node_id` produced.
    ///
    /// An empty transcript is still queued — as a placeholder rather than a
    /// dropped position. The queue is the Nth activation's slot:
    /// [`attach_transcripts`](Self::attach_transcripts) pops one entry onto the
    /// Nth step of the same node, so a dispatch that folded to nothing (only
    /// status events, say) must keep its slot or every later transcript would
    /// shift one step early and be misattributed to the wrong activation.
    pub(crate) fn record_transcript(&self, node_id: &str, transcript: Vec<TranscriptEntry>) {
        let mut transcripts = self.transcripts.lock().expect("agent evidence lock");
        let queue = transcripts.entry(node_id.to_string()).or_default();
        // The same ceiling the prompt queue uses, for the same reason: a node
        // in a loop can activate without bound, and this is held in memory for
        // the whole run. A placeholder counts like a real transcript — both are
        // one activation's slot.
        if queue.len() < MAX_PROMPTS_PER_NODE {
            queue.push_back(transcript);
        }
    }

    /// Attach prompts and transcripts to their corresponding persisted steps in
    /// completion order.
    pub(crate) fn attach(&self, steps: &mut [RunStep]) {
        self.attach_transcripts(steps);
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

    /// Drain each node's queued transcripts onto its steps, in order.
    ///
    /// Simpler than the prompt pass beside it, and deliberately so. A prompt
    /// queue may hold several entries for one step — a node that dispatched
    /// more than once inside a single activation — so that pass has to decide
    /// how many to fold together. A transcript is one whole harness turn, and
    /// the same can happen here: a node that dispatches more than once inside a
    /// single activation queues one transcript per dispatch, so the Nth step
    /// owns however many of its dispatches produced turns. Both passes use the
    /// same `remaining` bookkeeping — the last step of a node absorbs every
    /// entry still queued for it, and every step before it takes one — which is
    /// what keeps a multi-dispatch activation's transcripts on its own step
    /// instead of drifting onto a later activation of the same node.
    fn attach_transcripts(&self, steps: &mut [RunStep]) {
        let mut transcripts = self.transcripts.lock().expect("agent evidence lock");
        let mut remaining = HashMap::<String, usize>::new();
        for step in steps.iter() {
            *remaining.entry(step.node_id.clone()).or_default() += 1;
        }
        for step in steps {
            let Some(queue) = transcripts.get_mut(&step.node_id) else {
                continue;
            };
            let steps_left = remaining.get_mut(&step.node_id).expect("step was counted");
            let take = if *steps_left == 1 {
                queue.len()
            } else {
                1.min(queue.len())
            };
            let mut folded = Vec::new();
            for _ in 0..take {
                if let Some(transcript) = queue.pop_front() {
                    folded.extend(transcript);
                }
            }
            step.transcript = folded;
            *steps_left -= 1;
        }
    }
}
