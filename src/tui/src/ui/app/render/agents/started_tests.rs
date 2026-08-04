//! Attribution of spawned sessions to the conversation turn that caused them.

use crate::ui::events::{EventEnvelope, TuiEvent};

use super::super::super::session_focus::StartedSession;
use super::started::chat_lines_with_sessions;

/// One event in the stream, at a monotonic timestamp.
fn env(seq: u64, event: TuiEvent) -> EventEnvelope {
    EventEnvelope {
        seq,
        at: seq as i64,
        event,
    }
}

/// A user turn.
fn user(seq: u64, body: &str) -> EventEnvelope {
    env(
        seq,
        TuiEvent::User {
            body: body.to_string(),
        },
    )
}

/// A dispatch announced by the orchestrator.
fn task_start(seq: u64, task_id: &str) -> EventEnvelope {
    env(
        seq,
        TuiEvent::TaskStart {
            task_id: task_id.to_string(),
            instruction: String::new(),
            depth: 1,
            agent_id: None,
            contract: None,
        },
    )
}

/// A rail entry for a session serving `task_id`.
fn started(task_id: &str, agent: &str) -> StartedSession {
    StartedSession {
        agent: agent.to_string(),
        harness: Some("claude".into()),
        workspace: Some("/work/api".into()),
        task_id: task_id.to_string(),
        status: "running",
        row_index: 0,
    }
}

/// The rendered text of each line, for order assertions.
fn texts(lines: &[crate::ui::agents::Line]) -> Vec<String> {
    lines.iter().map(|line| line.text.clone()).collect()
}

/// Where a substring first appears in the rendered lines.
fn position(lines: &[crate::ui::agents::Line], needle: &str) -> usize {
    texts(lines)
        .iter()
        .position(|text| text.contains(needle))
        .unwrap_or_else(|| panic!("{needle} is missing from {:?}", texts(lines)))
}

#[test]
fn each_session_renders_under_the_query_that_started_it() {
    // Two queries, two dispatches. The old block listed both at the top of the
    // pane, which said *that* five sessions exist and never *which query* asked
    // for one — the fact an operator reading the conversation is after.
    let events = vec![
        user(1, "ship the auth fix"),
        task_start(2, "t_auth"),
        user(3, "now run the tests"),
        task_start(4, "t_tests"),
    ];
    let sessions = vec![
        started("t_auth", "api-claude"),
        started("t_tests", "api-claude"),
    ];

    let (lines, hits) = chat_lines_with_sessions(&events, 80, &sessions);

    assert!(
        position(&lines, "ship the auth fix") < position(&lines, "t_auth"),
        "the entry follows its query"
    );
    assert!(
        position(&lines, "t_auth") < position(&lines, "now run the tests"),
        "and precedes the next one: {:?}",
        texts(&lines)
    );
    assert!(
        position(&lines, "now run the tests") < position(&lines, "t_tests"),
        "the second turn keeps its own"
    );
    // Every entry stays addressable by task, line for line with what was drawn.
    assert_eq!(hits.len(), lines.len());
    assert_eq!(hits[position(&lines, "t_auth")].as_deref(), Some("t_auth"));
    assert_eq!(
        hits[position(&lines, "t_tests")].as_deref(),
        Some("t_tests")
    );
    assert_eq!(
        hits[position(&lines, "ship the auth fix")],
        None,
        "an ordinary transcript line opens nothing"
    );
}

#[test]
fn an_entry_names_the_agent_its_harness_and_its_workspace() {
    let events = vec![user(1, "go"), task_start(2, "t_1")];
    let (lines, _) = chat_lines_with_sessions(&events, 100, &[started("t_1", "api-claude")]);
    let entry = &texts(&lines)[position(&lines, "t_1")];
    assert!(entry.contains("api-claude"), "{entry}");
    assert!(entry.contains("claude × /work/api"), "{entry}");
    assert!(entry.contains("running"), "{entry}");
}

#[test]
fn a_session_this_conversation_cannot_account_for_still_appears() {
    // Dispatched in another thread, or folded in from a state older than the
    // visible events. A session that is running, costing tokens and unreachable
    // is the failure the block existed to prevent, so it is filed under a
    // heading that admits it rather than dropped.
    let events = vec![user(1, "go"), task_start(2, "t_known")];
    let sessions = vec![started("t_known", "api"), started("t_orphan", "web")];

    let (lines, hits) = chat_lines_with_sessions(&events, 80, &sessions);

    let at = position(&lines, "t_orphan");
    assert_eq!(hits[at].as_deref(), Some("t_orphan"), "and stays clickable");
    assert!(
        position(&lines, "sessions started outside this conversation") < at,
        "under a heading that says where it came from: {:?}",
        texts(&lines)
    );
    assert!(
        position(&lines, "t_known") < at,
        "after the turns that can be accounted for"
    );
}

#[test]
fn a_session_started_before_the_first_query_leads_the_conversation() {
    // Turn 0 is everything before the first user message. Chronologically that
    // is where it happened, so that is where it is drawn.
    let events = vec![task_start(1, "t_early"), user(2, "go")];
    let (lines, _) = chat_lines_with_sessions(&events, 80, &[started("t_early", "api")]);
    assert!(
        position(&lines, "t_early") < position(&lines, "go"),
        "{:?}",
        texts(&lines)
    );
}

#[test]
fn splitting_the_stream_at_its_turns_does_not_change_the_transcript() {
    // The grouping is a *relocation* of the entries, not a re-render of the
    // conversation: with no sessions to place, the output has to be exactly
    // what the unsplit fold produces, tool calls and all.
    let events = vec![
        user(1, "first"),
        env(
            2,
            TuiEvent::ToolCallStart {
                index: 0,
                name: "read".into(),
            },
        ),
        env(
            3,
            TuiEvent::ToolCallDelta {
                index: 0,
                args_delta: "{\"path\":\"a\"}".into(),
            },
        ),
        env(
            4,
            TuiEvent::Assistant {
                body: "done".into(),
            },
        ),
        user(5, "second"),
        env(
            6,
            TuiEvent::Assistant {
                body: "also done".into(),
            },
        ),
    ];

    let (grouped, hits) = chat_lines_with_sessions(&events, 80, &[]);
    assert_eq!(
        texts(&grouped),
        texts(&super::super::chat_lines(&events, 80))
    );
    assert!(hits.iter().all(Option::is_none));
}
