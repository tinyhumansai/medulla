//! Inbound pairing: what the hub identity does with a contact request it never
//! asked for.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::contacts::{ContactRelay, IncomingRequest};

/// A relay that serves one incoming request and records what was done to it.
#[derive(Default)]
struct FakeRelay {
    incoming: Mutex<Vec<String>>,
    accepted: Mutex<Vec<String>>,
    calls: Mutex<Vec<String>>,
}

impl FakeRelay {
    fn with_incoming(ids: &[&str]) -> Arc<Self> {
        Arc::new(FakeRelay {
            incoming: Mutex::new(ids.iter().map(|id| (*id).to_string()).collect()),
            ..FakeRelay::default()
        })
    }

    fn rows(ids: &[String]) -> Vec<IncomingRequest> {
        ids.iter()
            .map(|id| IncomingRequest {
                agent_id: id.clone(),
                handle: None,
            })
            .collect()
    }
}

impl ContactRelay for FakeRelay {
    fn incoming(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move { Ok(FakeRelay::rows(&self.incoming.lock().unwrap())) })
    }
    fn accepted(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move { Ok(FakeRelay::rows(&self.accepted.lock().unwrap())) })
    }
    fn accept(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("accept:{agent_id}"));
            self.incoming.lock().unwrap().retain(|id| *id != agent_id);
            self.accepted.lock().unwrap().push(agent_id);
            Ok(())
        })
    }
    fn decline(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("decline:{agent_id}"));
            Ok(())
        })
    }
    fn block(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(format!("block:{agent_id}"));
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_worker_that_names_this_hub_as_its_master_is_paired_and_narrated() {
    // The bug this prevents: the hub only ever *sent* contact requests, so a
    // worker pairing from its own screen left the edge pending forever. The
    // relay refuses a DM between non-contacts, so the worker showed its master
    // as offline and every message to it was refused — with nothing on the
    // hub side reading, or reporting, the request that would have fixed it.
    let relay = FakeRelay::with_incoming(&["worker-1"]);
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = {
        let said = said.clone();
        Arc::new(move |line: &str| said.lock().unwrap().push(line.to_string()))
    };

    let desk = super::super::pairing::pairing_desk(relay.clone(), log);
    desk.refresh().await;

    assert_eq!(relay.calls.lock().unwrap().clone(), vec!["accept:worker-1"]);
    assert!(
        desk.accepted().iter().any(|c| c.agent_id == "worker-1"),
        "the worker is a contact now: {:?}",
        desk.accepted()
    );
    let lines = said.lock().unwrap().clone();
    assert!(
        lines.iter().any(|line| line.contains("worker-1")),
        "an unasked-for pairing must be visible in the log: {lines:?}"
    );
}

#[tokio::test]
async fn pairing_does_not_re_accept_a_worker_the_relay_keeps_listing() {
    let relay = FakeRelay::with_incoming(&["worker-1"]);
    let desk = super::super::pairing::pairing_desk(relay.clone(), Arc::new(|_line: &str| {}));

    desk.refresh().await;
    desk.refresh().await;

    assert_eq!(
        relay.calls.lock().unwrap().len(),
        1,
        "one accept per pairing, however often it is listed"
    );
}
