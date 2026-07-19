//! Background poll loops layered on the tiny.place SDK client: destructive
//! mailbox reads, fail-closed contact auto-acceptance, and presence heartbeats.
//!
//! Each helper spawns a tokio task and returns its handle (and, for the mailbox,
//! a receiver). All loops are best-effort: transient SDK errors are ignored and
//! retried on the next tick.

use std::time::Duration;

use tinyplace::types::MessageEnvelope;
use tinyplace::TinyPlaceClient;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::super::frames::decode_task_frame;
use super::types::{MailboxItem, MailboxPoll};

/// Spawn a loop that polls `client.messages` for `agent_id` every `interval`,
/// **destructively reads** each message (acknowledges/deletes it after handing it
/// off), turns the opaque body into plaintext via `decode_body` (the caller's
/// Signal-decrypt hook; return `None` to skip a message), decodes any task frame,
/// and yields [`MailboxItem`]s over a channel.
///
/// Best-effort: transient list/ack errors are ignored and retried next tick. The
/// loop ends when the receiver is dropped.
pub fn spawn_mailbox_poll<F>(
    client: TinyPlaceClient,
    agent_id: String,
    interval: Duration,
    limit: i64,
    decode_body: F,
) -> MailboxPoll
where
    F: Fn(&MessageEnvelope) -> Option<String> + Send + 'static,
{
    let (tx, receiver) = mpsc::channel(256);
    let handle = tokio::spawn(async move {
        loop {
            if let Ok(resp) = client.messages.list(&agent_id, Some(limit)).await {
                for msg in resp.messages {
                    if let Some(body) = decode_body(&msg) {
                        let frame = decode_task_frame(&body);
                        let item = MailboxItem {
                            envelope: msg.clone(),
                            body,
                            frame,
                        };
                        if tx.send(item).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                    // Destructive read: delete the delivered message regardless of
                    // whether it decoded, so the relay does not redeliver it.
                    let _ = client.messages.acknowledge(&msg.id, &agent_id).await;
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
    MailboxPoll { handle, receiver }
}

/// Spawn a loop that polls incoming contact requests every `interval` and
/// accepts each one the fail-closed `allow` predicate approves (by cryptoId).
/// Requests `allow` rejects are left pending. Errors are ignored and retried.
///
/// A typical interval is ~1500ms.
pub fn spawn_contact_auto_accepter<F>(
    client: TinyPlaceClient,
    interval: Duration,
    allow: F,
) -> JoinHandle<()>
where
    F: Fn(&str) -> bool + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            if let Ok(resp) = client.contacts.requests(None).await {
                for view in resp.incoming {
                    if view.agent_id.is_empty() {
                        continue;
                    }
                    if allow(&view.agent_id) {
                        let _ = client.contacts.accept(&view.agent_id).await;
                    }
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
}

/// Spawn a loop that marks the acting agent online via `client.presence` every
/// `interval`. Errors are ignored and retried next tick.
pub fn spawn_presence_heartbeat(client: TinyPlaceClient, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let _ = client.presence.heartbeat().await;
            tokio::time::sleep(interval).await;
        }
    })
}
