//! Tests for how an `agent` dispatch is addressed.
//!
//! The subject here is the task id, which is the whole of the correlation
//! between a run and the harness sessions it has out: the run inspector joins
//! on it, `fleet_abort` cancels by it, and a worker dedupes on it. A duplicate
//! is therefore not a cosmetic clash — it silently merges two sessions.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use async_trait::async_trait;
use tinyflows::caps::AgentRunner;

use super::{dispatch_harness, AgentRoute, HarnessAgentRunner};
use crate::flow_engine::agent_evidence::AgentEvidence;
use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::flow_engine::harness_choice::HarnessChoice;
use crate::flow_engine::settings::CapabilitySettings;
use crate::harness_transcript::TranscriptEntry;
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::protocol::{HarnessProvider, HarnessTransport};
use crate::workflows::RunStep;

/// A dispatch that is never actually reached: these tests stop at the request.
struct UnusedDispatch;

#[async_trait]
impl HarnessDispatch for UnusedDispatch {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        unreachable!("these tests build requests rather than dispatching them")
    }
}

/// A dispatch that substitutes the harness the node asked for, the way a worker
/// without the named provider does.
///
/// It reads the dispatch registry from *inside* the dispatch, because that is
/// the only moment the entry exists: the recording guard is dropped as the
/// await returns.
struct SubstitutingDispatch {
    /// The run whose registry entry is read.
    run_id: String,
    /// The harness this dispatch really runs on, whatever was requested.
    substitute: String,
    /// What the registry named while the dispatch was in flight.
    recorded: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl HarnessDispatch for SubstitutingDispatch {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        *self.recorded.lock().expect("recorded lock") =
            crate::workflows::run::dispatches::in_flight(&self.run_id)
                .into_iter()
                .next()
                .map(|dispatch| dispatch.harness);
        Ok(TaskOutcome {
            reply: "done".to_string(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: None,
            transcript: Vec::new(),
        })
    }

    fn effective_harness(&self, _request: &TaskRequest) -> Option<String> {
        Some(self.substitute.clone())
    }
}

/// A runner for `run`, sharing `sequence` when one is given.
fn runner(run: &str, sequence: Option<Arc<AtomicU64>>) -> HarnessAgentRunner {
    let root = std::env::temp_dir().join("medulla-agent-tests");
    let mut settings = CapabilitySettings::rooted_at(&root);
    settings.default_worker_address = "worker".to_string();
    let built = HarnessAgentRunner::new(Arc::new(UnusedDispatch), Arc::new(settings), run);
    match sequence {
        Some(sequence) => built.with_sequence(sequence),
        None => built,
    }
}

/// The route both runners take when a node names no `agent_ref`.
fn task_id_of(runner: &HarnessAgentRunner) -> String {
    runner
        .request(
            &AgentRoute::Default,
            "do the thing".to_string(),
            HarnessChoice::default(),
        )
        .task_id
}

#[test]
fn one_runner_numbers_its_dispatches_in_order() {
    let runner = runner("run-ordered", None);
    assert_eq!(task_id_of(&runner), "wf:run-ordered:default#0");
    assert_eq!(task_id_of(&runner), "wf:run-ordered:default#1");
}

#[test]
fn two_runners_sharing_a_sequence_never_mint_the_same_task_id() {
    // The shape the run actually builds: an agent runner and the LLM provider's
    // own runner, both tagged with one run id and both routing to `default`.
    let sequence = Arc::new(AtomicU64::new(0));
    let agent = runner("run-shared", Some(sequence.clone()));
    let llm = runner("run-shared", Some(sequence));

    let first = task_id_of(&agent);
    let second = task_id_of(&llm);

    assert_ne!(
        first, second,
        "a shared sequence must not hand the same id to both runners"
    );
    assert_eq!(first, "wf:run-shared:default#0");
    assert_eq!(second, "wf:run-shared:default#1");
}

#[test]
fn independent_sequences_are_what_the_sharing_prevents() {
    // Guards the premise rather than the fix: if two unshared runners ever stop
    // colliding, `with_sequence` is no longer load-bearing and this test says so
    // instead of the collision resurfacing somewhere subtler.
    let agent = runner("run-split", None);
    let llm = runner("run-split", None);
    assert_eq!(task_id_of(&agent), task_id_of(&llm));
}

/// The registry records the *flavor*, so a node that asked for `codex-server`
/// is not reported as a plain `codex` session — a reader chasing the process
/// would go looking at the wrong one.
#[test]
fn a_transport_specific_harness_is_recorded_under_its_flavor_name() {
    let runner = runner("run-flavor", None);
    let request = runner.request(
        &AgentRoute::Default,
        "do the thing".to_string(),
        HarnessChoice {
            provider: Some(HarnessProvider::Codex),
            transport: Some(HarnessTransport::AppServer),
            custom_harness: None,
            model: None,
        },
    );

    assert_eq!(dispatch_harness(&request), "codex-server");
}

/// The ordinary pair keeps the bare provider name: a default transport must not
/// grow a suffix nobody wrote.
#[test]
fn a_default_transport_is_recorded_under_the_bare_provider_name() {
    let runner = runner("run-plain", None);
    let request = runner.request(
        &AgentRoute::Default,
        "do the thing".to_string(),
        HarnessChoice {
            provider: Some(HarnessProvider::Codex),
            transport: Some(HarnessTransport::default()),
            custom_harness: None,
            model: None,
        },
    );

    assert_eq!(dispatch_harness(&request), "codex");
}

/// A custom preset names itself, and a node that named nothing records nothing
/// — this side genuinely does not know what the worker's own config will pick.
#[test]
fn a_custom_preset_names_itself_and_an_unresolved_choice_names_nothing() {
    let runner = runner("run-custom", None);
    let custom = runner.request(
        &AgentRoute::Default,
        "do the thing".to_string(),
        HarnessChoice {
            provider: None,
            transport: None,
            custom_harness: Some("house-style".to_string()),
            model: None,
        },
    );
    assert_eq!(dispatch_harness(&custom), "house-style");

    let unresolved = runner.request(
        &AgentRoute::Default,
        "do the thing".to_string(),
        HarnessChoice::default(),
    );
    assert_eq!(dispatch_harness(&unresolved), "");
}

/// The registry names the harness that is *executing*, not the one the node
/// asked for. A worker without the named provider substitutes its own, and a run
/// inspector reporting the request would name a harness nobody is running.
#[tokio::test]
async fn the_registry_records_the_harness_the_dispatch_substituted() {
    let dispatch = Arc::new(SubstitutingDispatch {
        run_id: "run-substituted".to_string(),
        substitute: "claude".to_string(),
        recorded: std::sync::Mutex::new(None),
    });
    let root = std::env::temp_dir().join("medulla-agent-tests");
    let mut settings = CapabilitySettings::rooted_at(&root);
    settings.default_worker_address = "worker".to_string();
    let runner = HarnessAgentRunner::new(dispatch.clone(), Arc::new(settings), "run-substituted");

    runner
        .run_agent(
            "codex-server",
            serde_json::json!({ "prompt": "do the thing" }),
            None,
        )
        .await
        .expect("the dispatch replies");

    assert_eq!(
        dispatch.recorded.lock().expect("recorded lock").as_deref(),
        Some("claude"),
        "the substituted harness is what an inspector sees"
    );
    assert!(
        crate::workflows::run::dispatches::in_flight("run-substituted").is_empty(),
        "the entry is withdrawn when the dispatch returns"
    );
}

/// A dispatch that fails the way a real harness failure does: by the time the
/// task returns an error, the collector has already folded some of the
/// harness's events into a transcript. The workflow dispatch carries that
/// account on the failure (`RunError::WorkerWithTranscript`), and the runner
/// must record it onto the failed step — the step is the one place a transcript
/// is worth keeping, since its prompt and error say what was asked and what went
/// wrong but not what happened in between.
struct TranscriptFailingDispatch {
    message: String,
    transcript: Vec<TranscriptEntry>,
}

#[async_trait]
impl HarnessDispatch for TranscriptFailingDispatch {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Err(RunError::WorkerWithTranscript {
            message: self.message.clone(),
            transcript: self.transcript.clone(),
        })
    }
}

#[tokio::test]
async fn a_failed_dispatch_keeps_the_transcript_it_collected() {
    let evidence = Arc::new(AgentEvidence::default());
    let dispatch = Arc::new(TranscriptFailingDispatch {
        message: "harness exited nonzero".to_string(),
        transcript: vec![
            TranscriptEntry {
                at_ms: 1,
                kind: "tool_call".to_string(),
                text: "Bash(npm test)".to_string(),
            },
            TranscriptEntry {
                at_ms: 2,
                kind: "error".to_string(),
                text: "tests failed".to_string(),
            },
        ],
    });
    let root = std::env::temp_dir().join("medulla-agent-tests");
    let mut settings = CapabilitySettings::rooted_at(&root);
    settings.default_worker_address = "worker".to_string();
    let runner = HarnessAgentRunner::recording(
        dispatch.clone(),
        Arc::new(settings),
        "run-failed-transcript",
        evidence.clone(),
    );

    let error = runner
        .run_on_harness(
            AgentRoute::Default,
            "do the thing".to_string(),
            HarnessChoice::default(),
            Some("work".to_string()),
        )
        .await
        .expect_err("the dispatch fails");

    assert!(
        error.to_string().contains("harness exited nonzero"),
        "the failure message still reaches the step error"
    );

    let mut step = RunStep {
        node_id: "work".to_string(),
        status: "error".to_string(),
        duration_ms: 4,
        input: None,
        output: None,
        diagnostics: Vec::new(),
        transcript: Vec::new(),
    };
    evidence.attach(std::slice::from_mut(&mut step));
    assert_eq!(
        step.transcript, dispatch.transcript,
        "the failed step keeps the transcript its harness collected"
    );
}
