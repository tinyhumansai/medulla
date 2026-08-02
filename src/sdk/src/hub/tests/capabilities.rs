//! Tests for the socket-plane capability advertisement: the hub probes a worker
//! for its budgets/readiness over the encrypted relay and maps them onto the
//! backend-shaped `capabilities` payload the backend's `sanitizeCapabilities`
//! reads (`harnessBudgets`, `ready`, `readyReason`).

use std::time::Duration;

use serde_json::json;

use crate::hub::probe::decorate_with_budgets;
use crate::hub::tests::dispatch::harness::{FakeWorker, Mode};
use crate::hub::TaskRunner;
use crate::tinyplace::{
    AgentCapabilities, BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider, HarnessReadiness,
};

fn caps() -> AgentCapabilities {
    AgentCapabilities {
        providers: vec![HarnessProvider::Claude],
        budgets: vec![HarnessBudget {
            provider: HarnessProvider::Claude,
            seat: Some("seat-1".to_string()),
            window: BudgetWindow::FiveHour,
            limit_tokens: Some(1_000),
            used_tokens: Some(250),
            remaining_tokens: Some(750),
            cooldown_until: None,
            source: BudgetSource::Estimate,
        }],
        readiness: vec![HarnessReadiness {
            provider: HarnessProvider::Claude,
            ready: true,
            reason: None,
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn capability_probe_returns_the_workers_budgets_and_readiness() {
    let worker = FakeWorker::new(Mode::Capabilities(Box::new(caps())));
    let runner = TaskRunner::start_with_ack_window(
        worker.clone(),
        Duration::from_millis(5),
        Duration::from_millis(100),
    );

    let result = runner
        .capabilities("worker")
        .await
        .expect("capability reply");

    assert_eq!(result, caps());
    assert_eq!(worker.sent_kinds().await, vec!["capabilities"]);
}

#[test]
fn decoration_maps_budgets_and_readiness_onto_the_backend_shape() {
    let mut payload = json!({ "providers": ["claude"], "summary": "claude daemon" });
    decorate_with_budgets(&mut payload, &caps());

    // The budget array rides under the backend's `harnessBudgets` key, camelCased
    // exactly as `parseHarnessBudget` accepts. The window/source enums are
    // snake_case; the seat is carried on the wire (the backend strips it into the
    // client-safe projection).
    let budgets = payload["harnessBudgets"]
        .as_array()
        .expect("harnessBudgets");
    assert_eq!(budgets.len(), 1);
    assert_eq!(budgets[0]["provider"], json!("claude"));
    assert_eq!(budgets[0]["window"], json!("five_hour"));
    assert_eq!(budgets[0]["remainingTokens"], json!(750));
    assert_eq!(budgets[0]["source"], json!("estimate"));
    // A fully-ready fleet advertises the advisory scalar as usable, with no reason.
    assert_eq!(payload["ready"], json!(true));
    assert!(payload.get("readyReason").is_none());
    // The static facts are preserved untouched.
    assert_eq!(payload["summary"], json!("claude daemon"));
}

#[test]
fn decoration_collapses_not_ready_readiness_to_a_scalar_reason() {
    let mut degraded = caps();
    degraded.readiness = vec![HarnessReadiness {
        provider: HarnessProvider::Claude,
        ready: false,
        reason: Some("not authenticated".to_string()),
    }];
    let mut payload = json!({ "providers": ["claude"], "summary": "" });
    decorate_with_budgets(&mut payload, &degraded);

    assert_eq!(payload["ready"], json!(false));
    assert_eq!(payload["readyReason"], json!("not authenticated"));
}

#[test]
fn decoration_omits_keys_when_the_worker_advertises_nothing() {
    let bare = AgentCapabilities {
        providers: vec![HarnessProvider::Claude],
        ..Default::default()
    };
    let mut payload = json!({ "providers": ["claude"], "summary": "" });
    decorate_with_budgets(&mut payload, &bare);

    assert!(payload.get("harnessBudgets").is_none());
    assert!(payload.get("ready").is_none());
    assert!(payload.get("readyReason").is_none());
}
