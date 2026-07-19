//! medulla-API fold leg: drive the encrypted owner → daemon chain, then fold the
//! decrypted frames into `TuiEvent`s and assert they fold, via
//! `medulla_tui::ui::agents`, into the expected agents-lane/task state.

use std::collections::HashMap;

use medulla::daemon::DaemonRuntime;
use medulla::runtime::AgentDescriptor;
use medulla::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind};
use medulla_tui::ui::agents::{derive_agent_lanes, TaskStatus};
use medulla_tui::ui::events::{EventEnvelope, TaskDigest, TuiEvent};

use crate::helpers::*;
use crate::mock_harness::{MockCli, MockDir, MockProvider};
use crate::mock_signal_server::MockSignalServer;

/// Fold the frames the owner decrypted off the wire into the TUI event stream
/// the way the owner's UI would surface a delegated task: dispatch → TaskStart,
/// each status → TaskEvent, the reply → TaskComplete.
fn frames_to_events(worker_id: &str, task_id: &str, frames: &[TaskFrame]) -> Vec<EventEnvelope> {
    let mut events = vec![EventEnvelope {
        seq: 0,
        at: 0,
        event: TuiEvent::TaskStart {
            task_id: task_id.to_string(),
            instruction: "do the thing".to_string(),
            depth: 2,
            agent_id: Some(worker_id.to_string()),
        },
    }];
    let mut seq = 1u64;
    for frame in frames {
        match frame.kind {
            TaskFrameKind::Status => {
                events.push(EventEnvelope {
                    seq,
                    at: seq as i64 * 1000,
                    event: TuiEvent::TaskEvent {
                        task_id: task_id.to_string(),
                        event_kind: "text".to_string(),
                        content: frame.text.clone(),
                        harness: frame.harness.map(|h| h.as_str().to_uppercase()),
                    },
                });
                seq += 1;
            }
            TaskFrameKind::Reply => {
                events.push(EventEnvelope {
                    seq,
                    at: seq as i64 * 1000,
                    event: TuiEvent::TaskComplete {
                        digest: TaskDigest {
                            task_id: task_id.to_string(),
                            status: "done".to_string(),
                            digest: frame.text.clone(),
                            result_ref: None,
                            usage: None,
                            depth: 2,
                        },
                    },
                });
                seq += 1;
            }
            _ => {}
        }
    }
    events
}

// Scenario 6 (medulla-API leg): drive the encrypted owner → daemon chain, then
// fold the decrypted frames into TuiEvents and assert they fold, via
// `medulla_tui::ui::agents`, into the expected agents-lane/task state — a worker lane
// for the delegated agent whose task lands Done carrying the reply digest.
#[tokio::test]
async fn decrypted_frames_fold_into_agent_lane_states() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-fold", &server.base_url);
    let worker = make_identity("worker-fold", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    let work = IdentityDir::new("work-fold");
    let mock = MockCli::new(MockProvider::Claude)
        .thinking("planning")
        .message("progress note")
        .claude_result("the final result");
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_CLAUDE_BIN".to_string(), bin);
    let runtime = DaemonRuntime::new(
        daemon_config(
            HarnessProvider::Claude,
            work.path.to_string_lossy().into_owned(),
            env,
        ),
        real_run_task(),
        transport_send(worker.transport.clone()),
    );

    owner
        .transport
        .send(
            &worker_id,
            &task_frame(
                TaskFrameKind::Task,
                "cyc-9",
                "do the thing",
                Some("corr-fold"),
            ),
        )
        .await
        .unwrap();

    let mut collected: Vec<TaskFrame> = Vec::new();
    let saw_reply = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| frames.iter().any(|f| f.kind == TaskFrameKind::Reply),
    )
    .await;
    assert!(saw_reply, "no reply frame to fold: {collected:?}");
    runtime.idle().await;
    pump_chain(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
    )
    .await;

    // Fold the decrypted frames through the public agents view-model.
    let roster = vec![AgentDescriptor {
        id: worker_id.clone(),
        name: "worker".to_string(),
        description: String::new(),
        availability: "online".to_string(),
        tags: vec![],
        metadata: serde_json::Map::new(),
    }];
    let events = frames_to_events(&worker_id, "cyc-9", &collected);
    let lanes = derive_agent_lanes(&events, "TINYPLACE", &roster);

    let lane = lanes
        .iter()
        .find(|l| l.key == format!("agent:{worker_id}"))
        .expect("a lane for the delegated worker agent");
    let task = lane
        .tasks
        .iter()
        .find(|t| t.task_id == "cyc-9")
        .expect("the delegated task folded into the lane");
    assert_eq!(task.status, TaskStatus::Done, "reply frame folds to Done");
    // The completion turn carries the reply as its digest content.
    assert!(
        task.turn_blocks
            .iter()
            .any(|b| b.content.as_deref() == Some("the final result")),
        "the reply digest folded into the task's turns"
    );
    // The lane closed out its active task on completion.
    assert_eq!(lane.active_tasks, 0);
}
