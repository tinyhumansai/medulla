//! One dispatched leg: open the link, send a task frame, wait for the terminal
//! frame, and render it as the JSON line the harness asserts on.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use medulla::bridge::{Bridge, LinkBridge};
use medulla::protocol::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrame, TaskFrameKind,
};
use medulla_link::keys::NodeId;
use medulla_link::{Link, LinkConfig};

use crate::{Leg, POLL};

/// Bring the link up, retrying briefly while a previous driver releases the
/// identity lock (the lock is dropped when the old driver task stops, which is
/// shortly after its handle is).
pub async fn connect(state_dir: &Path, forwarder: Option<&str>) -> Result<LinkBridge, String> {
    let mut config = LinkConfig::new(state_dir);
    config.forwarder_endpoint = forwarder.map(str::to_string);
    let mut last = String::new();
    for _ in 0..25 {
        match Link::connect(config.clone()).await {
            Ok(link) => {
                let [peer] = link.peers() else {
                    return Err(format!(
                        "expected the coordination link to have one peer, found {}",
                        link.peers().len()
                    ));
                };
                let peer = *peer;
                return LinkBridge::single_peer(
                    Arc::new(link),
                    "coordination-owner",
                    peer.to_string(),
                );
            }
            Err(err) => {
                last = err.to_string();
                tokio::time::sleep(POLL).await;
            }
        }
    }
    Err(format!(
        "could not bring up the link at {}: {last}",
        state_dir.display()
    ))
}

/// Run one leg: send the frame, wait for the terminal frame, build the report.
///
/// Returns the leg's exit code and the JSON line describing it.
pub async fn run_leg(link: &LinkBridge, owner_id: NodeId, leg: &Leg) -> (i32, serde_json::Value) {
    let peers = link.link().peers();
    let Some(peer) = leg.to.or_else(|| peers.first().copied()) else {
        return (
            2,
            report_error(owner_id, leg, "missing --to <worker node id>"),
        );
    };
    if !peers.contains(&peer) {
        let known: Vec<String> = peers.iter().map(NodeId::to_string).collect();
        return (
            2,
            report_error(
                owner_id,
                leg,
                &format!(
                    "{peer} is not this link's peer (enrolled: {})",
                    known.join(", ")
                ),
            ),
        );
    }

    let frame = encode_task_frame(EncodeFrameInput {
        kind: leg.kind,
        task_id: leg.task_id.clone(),
        text: leg.task.clone(),
        ts: medulla::clock::iso_now(),
        correlation_id: Some(format!("{}-corr", leg.task_id)),
        harness: None,
        provider: leg.provider,
        custom_harness: None,
        model: leg.model.clone(),
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    if let Err(err) = link.send(&peer.to_string(), &frame).await {
        return (
            2,
            report_error(owner_id, leg, &format!("send failed: {err}")),
        );
    }
    eprintln!(
        "coordination_owner: {owner_id} → {peer} task {} sent, waiting for a terminal frame…",
        leg.task_id
    );

    let deadline = tokio::time::Instant::now() + Duration::from_millis(leg.timeout_ms);
    let mut collected: Vec<TaskFrame> = Vec::new();
    let mut terminal: Option<TaskFrame> = None;
    while terminal.is_none() {
        for message in link.drain_inbox(1024).await {
            let Some(frame) = decode_task_frame(&message.text) else {
                eprintln!("coordination_owner: ignoring a message that is not a task frame");
                continue;
            };
            // A leg only ends on its *own* task. A reply to an earlier, timed-out leg
            // can still be in flight on a link this process keeps across legs, and
            // letting it terminate this one would report the wrong answer.
            if frame.task_id != leg.task_id {
                eprintln!(
                    "coordination_owner: ignoring frame for task {} (waiting on {})",
                    frame.task_id, leg.task_id
                );
                continue;
            }
            eprintln!(
                "coordination_owner: frame kind={:?} text={:?}",
                frame.kind, frame.text
            );
            let is_terminal = matches!(
                frame.kind,
                TaskFrameKind::Reply | TaskFrameKind::Error | TaskFrameKind::CapabilitiesResult
            );
            collected.push(frame.clone());
            if is_terminal {
                terminal = Some(frame);
                break;
            }
        }
        if terminal.is_some() || tokio::time::Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        link.wait_for_inbox(remaining.min(POLL)).await;
    }

    match terminal {
        Some(frame) => {
            let code = i32::from(!matches!(
                frame.kind,
                TaskFrameKind::Reply | TaskFrameKind::CapabilitiesResult
            ));
            (code, report(owner_id, &frame, &collected))
        }
        None => {
            eprintln!(
                "coordination_owner: timed out with no terminal frame ({} frames seen)",
                collected.len()
            );
            (
                1,
                report_error(owner_id, leg, "timed out with no terminal frame"),
            )
        }
    }
}

/// The terminal frame as the JSON line the shell asserts on.
fn report(owner_id: NodeId, frame: &TaskFrame, collected: &[TaskFrame]) -> serde_json::Value {
    serde_json::json!({
        "kind": format!("{:?}", frame.kind),
        "text": frame.text,
        "taskId": frame.task_id,
        "correlationId": frame.correlation_id,
        "harness": frame.harness.map(|h| h.as_str().to_string()),
        "ownerId": owner_id.to_string(),
        "frames": collected.len(),
        "frameKinds": collected.iter().map(|f| f.kind.as_str().to_string()).collect::<Vec<_>>(),
        "usage": frame.usage.as_ref().map(|u| serde_json::json!({
            "inputTokens": u.input_tokens,
            "outputTokens": u.output_tokens,
        })),
    })
}

/// The same shape for a leg that never reached a terminal frame, so a scenario
/// asserting on the JSON sees a reason rather than an empty file.
fn report_error(owner_id: NodeId, leg: &Leg, reason: &str) -> serde_json::Value {
    eprintln!("coordination_owner: {reason}");
    serde_json::json!({
        "kind": "None",
        "text": reason,
        "taskId": leg.task_id,
        "ownerId": owner_id.to_string(),
        "frames": 0,
        "frameKinds": Vec::<String>::new(),
    })
}
