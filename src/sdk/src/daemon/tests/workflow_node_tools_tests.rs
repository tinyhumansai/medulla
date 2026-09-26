//! What tool surface a workflow `agent` node's harness is launched with.
//!
//! The node is dispatched as an ordinary task *frame*: `LocalWorkflowHost`
//! stands up an embedded daemon in the same process and sends each node's
//! instruction to it over a loopback bridge. That made this the one launch door
//! for a workflow node the withholding in [`crate::harness_tools`] did not
//! cover, and the consequence was observable — a `pr-babysitter` `babysit` node
//! was served `workflow_run` and the skill describing its own graph, and
//! started that graph again on its own pull request.
//!
//! These tests pin the boundary from both sides: a trusted node loses the
//! workflow family and nothing else, and the three launches that must keep it
//! (an ordinary delegated task, a forged marker from a remote peer, and a
//! scoped evolution review) still have it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::protocol::TaskFrame;

use super::{base_config, recording_send, task_frame};

/// A runner that records the environment of every launch it is given.
fn env_runner(seen: Arc<StdMutex<Vec<HashMap<String, String>>>>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        seen.lock().unwrap().push(opts.env.clone());
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

/// The environment of the one launch `frame` produced, delivered with the
/// caller's own verdict on whether the sender is device-local.
async fn launch_env(frame: TaskFrame, sender_device_local: bool) -> HashMap<String, String> {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), env_runner(seen.clone()), send);
    runtime.handle_message_from(
        "ident-this-device".into(),
        String::new(),
        Some(frame),
        sender_device_local,
    );
    runtime.idle().await;
    let mut seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "expected exactly one launch");
    seen.pop().unwrap()
}

/// A node frame as the workflow host's loopback dispatch sends it.
fn node_frame(task_id: &str) -> TaskFrame {
    TaskFrame {
        workflow_node: true,
        ..task_frame(task_id, "run this node", None)
    }
}

#[tokio::test]
async fn a_workflow_node_is_launched_without_the_workflow_tools() {
    let env = launch_env(node_frame("node"), true).await;

    assert!(
        crate::harness_tools::workflows_withheld(&env),
        "a node's harness must be marked as getting no workflow tools: {env:?}"
    );
    assert!(
        !env.contains_key(crate::mcp::TOOL_MODE_ENV),
        "no slice of an absent family is left to select: {env:?}"
    );
}

/// Only the workflow surface goes. `fleet_*` is bounded by the grant's own
/// depth and in-flight limits, and a node that can see the fleet is not the
/// thing that recursed.
#[tokio::test]
async fn a_workflow_node_keeps_the_rest_of_the_medulla_surface() {
    let env = launch_env(node_frame("node"), true).await;

    assert!(
        !crate::harness_tools::withheld(&env),
        "the whole tool surface must not be withheld, only the workflow half: {env:?}"
    );
}

/// The marker is caller-controlled JSON. Read from a remote peer it would let
/// that peer choose the tool surface of a harness on this host — narrower here
/// rather than wider, but still not the sender's decision.
#[tokio::test]
async fn a_remote_peers_workflow_node_marker_withholds_nothing() {
    let env = launch_env(node_frame("forged"), false).await;

    assert!(
        !crate::harness_tools::workflows_withheld(&env),
        "a remote peer's marker must be read as ordinary delegated work: {env:?}"
    );
}

/// The ordinary case, asserted explicitly: this seam must fail open, or every
/// operator-dispatched task silently loses its tools.
#[tokio::test]
async fn an_ordinary_task_keeps_its_workflow_tools() {
    let env = launch_env(task_frame("plain", "do the work", None), true).await;

    assert!(
        !crate::harness_tools::workflows_withheld(&env),
        "an ordinary task must keep the workflow tools: {env:?}"
    );
}

/// An evolution review comes down the same loopback bridge with the same
/// marker, and it is not the recursion this guards against: `propose:<id>`
/// already withholds `workflow_run` and every editing verb, and the notes and
/// proposal verbs it keeps are the whole point of the turn.
#[tokio::test]
async fn a_scoped_review_turn_keeps_its_workflow_tools() {
    let frame = TaskFrame {
        tool_mode: Some("propose:nightly".to_string()),
        ..node_frame("review")
    };

    let env = launch_env(frame, true).await;

    assert!(
        !crate::harness_tools::workflows_withheld(&env),
        "a turn that asked for a scoped workflow surface must keep it: {env:?}"
    );
    assert_eq!(
        env.get(crate::mcp::TOOL_MODE_ENV).map(String::as_str),
        Some("propose:nightly"),
        "the review's mode must still reach its tool server: {env:?}"
    );
}

/// A copilot authoring turn as `CopilotSession` and `LocalCopilotDispatch`
/// build it: the same loopback bridge, the same `workflowNode` marker, and no
/// tool mode — because an authoring turn is deliberately unscoped. Gating the
/// withholding on the node marker alone therefore caught the copilot too, and
/// took away `workflow_create` and every editing verb from the one caller whose
/// entire purpose is to call them. Its caller reads the store back afterwards,
/// so the failure is silent: the turn talks, and nothing was written.
#[tokio::test]
async fn a_copilot_authoring_turn_keeps_its_workflow_tools() {
    let frame = TaskFrame {
        workflow_authoring: true,
        conversation: Some("workflows-pane-1".to_string()),
        ..node_frame("copilot-turn")
    };

    let env = launch_env(frame, true).await;

    assert!(
        !crate::harness_tools::workflows_withheld(&env),
        "an authoring turn must keep the workflow tools it exists to call: {env:?}"
    );
}

/// And it keeps the managed workflow *skills* with them, which is the same
/// question asked at the other seam: both the argv renderer and the skills
/// attachment read this one marker, so an authoring turn that kept the tools
/// but lost the skills would still be launched half-briefed.
#[tokio::test]
async fn a_copilot_authoring_turn_keeps_the_managed_workflow_skills() {
    let frame = TaskFrame {
        workflow_authoring: true,
        ..node_frame("copilot-skills")
    };

    let env = launch_env(frame, true).await;

    assert!(
        !crate::harness_tools::withheld(&env) && !crate::harness_tools::workflows_withheld(&env),
        "the skills renderer asks exactly this question: {env:?}"
    );
}

/// The authoring marker widens a launch's tool surface, so it is read on the
/// same terms as the node marker beside it: only from a peer this daemon
/// serves. A graph step cannot set it either — it is minted by
/// [`crate::hub::TaskRunner::run_workflow_authoring`], which only the copilot's
/// own dispatch handle reaches — but the frame is JSON, and this pins that a
/// node frame without the marker is still withheld.
#[tokio::test]
async fn a_node_frame_without_the_authoring_marker_is_still_withheld() {
    let env = launch_env(node_frame("node"), true).await;

    assert!(
        crate::harness_tools::workflows_withheld(&env),
        "a graph step must still lose the workflow family: {env:?}"
    );
}
