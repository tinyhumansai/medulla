//! Unit tests for the stream derivations.

use super::*;
use crate::runtime::ThreadSummary;
use crate::ui::events::{EventEnvelope, TaskDigest, Usage};

/// Build an envelope with a synthetic timestamp derived from `seq`.
fn env(seq: u64, event: TuiEvent) -> EventEnvelope {
    EventEnvelope {
        seq,
        at: seq as i64 * 1000,
        event,
    }
}

fn start(tier: &str, model: Option<&str>) -> TuiEvent {
    TuiEvent::InferenceStart {
        tier: tier.into(),
        op: "execute_step".into(),
        model: model.map(Into::into),
    }
}

fn end(tier: &str, model: Option<&str>, usage: Option<(i64, i64)>) -> TuiEvent {
    TuiEvent::InferenceEnd {
        tier: tier.into(),
        op: "execute_step".into(),
        model: model.map(Into::into),
        duration_ms: 1,
        usage: usage.map(|(i, o)| Usage {
            input_tokens: i,
            output_tokens: o,
        }),
        content: None,
        reasoning: None,
        tool_calls: None,
    }
}

fn thread(id: &str, parent: Option<&str>) -> ThreadSummary {
    ThreadSummary {
        id: id.into(),
        parent_id: parent.map(Into::into),
        name: id.into(),
        running: false,
        turns: 0,
        running_tasks: 0,
        attention: 0,
    }
}

#[test]
fn usage_fold_accumulates_tiers_and_tasks() {
    let events = vec![
        env(1, start("reasoning", Some("gpt"))),
        env(2, end("reasoning", Some("gpt"), Some((100, 20)))),
        env(3, start("orchestrator", None)),
        env(
            4,
            TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: "t1".into(),
                    status: "completed".into(),
                    digest: String::new(),
                    result_ref: None,
                    usage: Some(Usage {
                        input_tokens: 7,
                        output_tokens: 3,
                    }),
                    depth: 1,
                    contract: None,
                    evidence: None,
                },
            },
        ),
    ];
    let fold = usage_fold(&events);
    let reasoning = fold.tiers.get("reasoning").expect("reasoning tier");
    assert_eq!(reasoning.calls, 1);
    assert_eq!(reasoning.input_tokens, 100);
    assert_eq!(reasoning.output_tokens, 20);
    assert_eq!(fold.tiers.get("orchestrator").unwrap().calls, 1);
    assert_eq!(fold.subagent.calls, 1);
    assert_eq!(fold.subagent.input_tokens, 7);
    assert_eq!(fold.tasks, vec![("t1".to_string(), 7, 3)]);
}

#[test]
fn running_calls_clamps_at_zero() {
    // Two starts, three ends → never negative, lands at 0.
    let events = vec![
        env(1, start("reasoning", None)),
        env(2, start("reasoning", None)),
        env(3, end("reasoning", None, None)),
        env(4, end("reasoning", None, None)),
        env(5, end("reasoning", None, None)),
    ];
    assert_eq!(running_calls(&events), 0);

    let in_flight = vec![
        env(1, start("reasoning", None)),
        env(2, start("reasoning", None)),
        env(3, end("reasoning", None, None)),
    ];
    assert_eq!(running_calls(&in_flight), 1);
}

#[test]
fn observed_model_returns_most_recent_for_tier() {
    let events = vec![
        env(1, start("reasoning", Some("old"))),
        env(2, end("reasoning", Some("new"), None)),
        env(3, start("orchestrator", Some("other"))),
    ];
    assert_eq!(observed_model(&events, "reasoning"), Some("new"));
    assert_eq!(observed_model(&events, "orchestrator"), Some("other"));
    assert_eq!(observed_model(&events, "compress"), None);
}

#[test]
fn thread_depths_counts_hops_and_guards_cycles() {
    let threads = vec![
        thread("root", None),
        thread("a", Some("root")),
        thread("b", Some("a")),
    ];
    let depths = thread_depths(&threads);
    assert_eq!(depths["root"], 0);
    assert_eq!(depths["a"], 1);
    assert_eq!(depths["b"], 2);

    // A 2-node cycle must not loop forever; depth is bounded by the 32-hop guard.
    let cyclic = vec![thread("x", Some("y")), thread("y", Some("x"))];
    let d = thread_depths(&cyclic);
    assert_eq!(d["x"], 32);
    assert_eq!(d["y"], 32);
}
