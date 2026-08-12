//! How much of a run record a reader gets back.
//!
//! A run record inlines every step's whole input and output, because that is
//! the evidence a failure is diagnosed from. For the caller *asking* whether a
//! run finished it is the wrong answer entirely: one `pr-babysitter` history
//! listing is 433KB of prompts and transcripts, which is more than the model
//! reading it can hold.
//!
//! So the shape is chosen by the reader rather than fixed by the record. Three
//! levels, and the difference between them is only ever how much of each step
//! survives — status, timing, diagnosis, and everything else about the run are
//! identical in all three.

use serde_json::{json, Map, Value};

use crate::workflows::types::bounded_within;
use crate::workflows::{RunRecord, RunStep, WorkflowError};

/// Bytes of one step's output kept by [`StepDetail::Summary`].
///
/// Small enough that a hundred steps still fit a reply, large enough that a
/// script's stdout or an agent's verdict is usually readable in full.
const SUMMARY_OUTPUT_BYTES: usize = 2048;

/// How much of each step a caller wants back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepDetail {
    /// Every step with its whole input and output, as recorded.
    Full,
    /// Every step with its node, status, duration, diagnostics, and a bounded
    /// preview of its output. What "what happened" needs.
    #[default]
    Summary,
    /// No steps at all, only how many there were. What a history listing needs:
    /// which runs exist, and how each of them ended.
    Counts,
}

impl StepDetail {
    /// Read the `steps` argument, falling back to `default` when it is absent.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::Malformed`] for a value that is not one of the
    /// three levels, naming them — a caller that guessed "brief" should be told
    /// what to say instead rather than silently handed the default.
    pub fn parse(value: Option<&str>, default: Self) -> Result<Self, WorkflowError> {
        match value.map(str::trim) {
            None | Some("") => Ok(default),
            Some("full") => Ok(Self::Full),
            Some("summary") => Ok(Self::Summary),
            Some("counts") | Some("none") => Ok(Self::Counts),
            Some(other) => Err(WorkflowError::Malformed(format!(
                "'steps' must be one of 'full', 'summary', or 'counts'; got '{other}'"
            ))),
        }
    }

    /// This level's name, as it appears in a projected record.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Summary => "summary",
            Self::Counts => "counts",
        }
    }
}

/// A run record as `detail` asks to see it.
///
/// `stepCount` and `stepDetail` are added at every level, including `full`: a
/// reader must be able to tell a run with no steps from a projection that
/// dropped them, and that distinction cannot depend on which level was asked
/// for.
///
/// # Errors
///
/// Returns [`WorkflowError::Engine`] if the record cannot be serialized, which
/// in practice means a step output that is not representable as JSON.
pub fn project(record: &RunRecord, detail: StepDetail) -> Result<Value, WorkflowError> {
    let mut value = serde_json::to_value(record).map_err(|err| {
        WorkflowError::Engine(format!("could not serialize run '{}': {err}", record.id))
    })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| WorkflowError::Engine("a run record must serialize to an object".into()))?;
    object.insert("stepCount".to_string(), json!(record.steps.len()));
    object.insert("stepDetail".to_string(), json!(detail.as_str()));

    match detail {
        StepDetail::Full => {}
        StepDetail::Summary => {
            let steps: Vec<Value> = record.steps.iter().map(summarize).collect();
            object.insert("steps".to_string(), Value::Array(steps));
            object.insert("stepsElidedHint".to_string(), json!(FULL_HINT));
        }
        StepDetail::Counts => {
            object.remove("steps");
            object.insert("stepsElidedHint".to_string(), json!(FULL_HINT));
        }
    }
    Ok(value)
}

/// What to call when the elided halves of a step are the thing wanted.
const FULL_HINT: &str = "step inputs, outputs and harness transcripts are elided here; call \
                         workflow_run_get with { runId, \"steps\": \"full\" } for the whole record";

/// One step reduced to what "what happened" needs.
///
/// The input is dropped rather than bounded. For an `agent` node it is the
/// whole prompt, which the caller usually wrote and always can re-read from the
/// graph — it is the largest field in a run record and the least informative
/// about what the run *did*.
fn summarize(step: &RunStep) -> Value {
    let mut summary = Map::new();
    summary.insert("nodeId".to_string(), json!(step.node_id));
    summary.insert("status".to_string(), json!(step.status));
    summary.insert("durationMs".to_string(), json!(step.duration_ms));
    if let Some(output) = &step.output {
        summary.insert(
            "output".to_string(),
            bounded_within(output, SUMMARY_OUTPUT_BYTES),
        );
    }
    if !step.diagnostics.is_empty() {
        summary.insert("diagnostics".to_string(), json!(step.diagnostics));
    }
    // The transcript itself is elided — it is the largest field on an agent
    // step — but its *size* is not, because that is what tells a reader there
    // is an account of this step to ask for. Omitted entirely when there is
    // none, so a step with no transcript is not reported as having an empty
    // one: those mean different things (a non-agent node, versus a harness
    // that said nothing).
    if !step.transcript.is_empty() {
        summary.insert(
            "transcriptEntries".to_string(),
            json!(step.transcript.len()),
        );
    }
    Value::Object(summary)
}
