//! End-to-end task dispatch over the device-local bridge with no tiny.place
//! client, identity, contact graph, or network server.

use std::sync::Arc;
use std::time::Duration;

use medulla::bridge::{Bridge, LocalBridgeNetwork};
use medulla::hub::{TaskRequest, TaskRunner};
use medulla::tinyplace::{decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrameKind};

#[tokio::test]
async fn task_runner_round_trips_entirely_over_the_local_bridge() {
    let network = LocalBridgeNetwork::new();
    let owner = Arc::new(network.bind("owner").unwrap());
    let worker = network.bind("worker").unwrap();

    let responder = tokio::spawn(async move {
        loop {
            for message in worker.drain_inbox(10).await {
                let frame = decode_task_frame(&message.text).unwrap();
                if frame.kind != TaskFrameKind::Task {
                    continue;
                }
                for (kind, text) in [
                    (TaskFrameKind::Ack, "accepted"),
                    (TaskFrameKind::Reply, "local result"),
                ] {
                    worker
                        .send(
                            &message.from,
                            &encode_task_frame(EncodeFrameInput {
                                kind,
                                task_id: frame.task_id.clone(),
                                text: text.to_string(),
                                ts: "T".to_string(),
                                correlation_id: frame.correlation_id.clone(),
                                harness: None,
                                provider: None,
                                model: None,
                                workflow: None,
                                conversation: None,
                            }),
                        )
                        .await
                        .unwrap();
                }
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });

    let runner = TaskRunner::start(owner, Duration::from_millis(1));
    let outcome = runner
        .run(
            TaskRequest {
                task_id: "local-task".to_string(),
                abort_id: "local-task".to_string(),
                cycle_id: Some("local-cycle".to_string()),
                instruction: "stay on this device".to_string(),
                worker_address: "worker".to_string(),
                provider: None,
                model: None,
                workflow: None,
                conversation: None,
            },
            None,
        )
        .await
        .unwrap();

    responder.await.unwrap();
    assert_eq!(outcome.reply, "local result");
}
