//! Stable cursor identity tests for the Sessions rail.

use super::{rail_anchor, resolve_rail_cursor, RailAnchor, RailRow, SessionRailRow};
use crate::ui::agents::{AgentLane, AgentRole, TaskState, TaskStatus};

/// A local session row, keyed by the PTY id its anchor is built from.
fn session(id: &str) -> RailRow {
    RailRow::Session(Box::new(SessionRailRow {
        agent_id: None,
        lane_index: None,
        task: None,
        local: Some(super::tests::stub_session(id)),
        last: true,
    }))
}

fn lane(key: &str) -> AgentLane {
    AgentLane {
        key: key.to_string(),
        label: String::new(),
        role: AgentRole::Agent,
        turns: Vec::new(),
        last_at: 0,
        tasks: Vec::new(),
        context_tokens: None,
        usage: Default::default(),
        harness_label: None,
        agent_id: None,
        session_id: None,
        parent_agent_id: None,
        descriptor: None,
        active_tasks: 0,
        work: None,
    }
}

#[test]
fn an_anchored_session_follows_rows_inserted_ahead_of_it() {
    let anchor = RailAnchor::Session("w_2".to_string());
    assert_eq!(
        resolve_rail_cursor(
            &[RailRow::NewSession, session("w_1"), session("w_2")],
            &[],
            Some(&anchor),
            0
        ),
        2
    );
}

#[test]
fn a_missing_anchor_uses_the_clamped_previous_offset() {
    let rows = vec![RailRow::NewSession, session("w_1")];
    assert_eq!(
        resolve_rail_cursor(
            &rows,
            &[],
            Some(&RailAnchor::Session("removed".to_string())),
            99
        ),
        1
    );
}

#[test]
fn an_overflow_anchor_uses_its_lanes_stable_key() {
    let lanes = vec![lane("builder")];
    let overflow = RailRow::Overflow {
        lane_index: 0,
        hidden: 3,
    };
    let anchor = rail_anchor(&overflow, &lanes);
    assert_eq!(anchor, Some(RailAnchor::Overflow("builder".to_string())));
    assert_eq!(
        resolve_rail_cursor(
            &[RailRow::NewSession, session("w_1"), overflow],
            &lanes,
            anchor.as_ref(),
            0
        ),
        2
    );
}

#[test]
fn a_removed_overflow_anchor_relocates_to_its_lanes_first_session() {
    let lanes = vec![lane("builder")];
    let task = |id: &str| {
        RailRow::Session(Box::new(SessionRailRow {
            agent_id: Some("builder".to_string()),
            lane_index: Some(0),
            task: Some(TaskState {
                task_id: id.to_string(),
                status: TaskStatus::Running,
                turns: 0,
                last_at: 0,
                turn_blocks: Vec::new(),
                attention: None,
                question_id: None,
                work: None,
            }),
            local: None,
            last: false,
        }))
    };

    assert_eq!(
        resolve_rail_cursor(
            &[RailRow::NewSession, task("first"), task("retained")],
            &lanes,
            Some(&RailAnchor::Overflow("builder".to_string())),
            2,
        ),
        1,
    );
}

#[test]
fn a_task_anchor_survives_local_pty_enrichment() {
    let lanes = vec![lane("builder")];
    let task = TaskState {
        task_id: "t-1".to_string(),
        status: TaskStatus::Running,
        turns: 0,
        last_at: 0,
        turn_blocks: Vec::new(),
        attention: None,
        question_id: None,
        work: None,
    };
    let before = RailRow::Session(Box::new(SessionRailRow {
        agent_id: Some("builder".to_string()),
        lane_index: Some(0),
        task: Some(task),
        local: None,
        last: true,
    }));
    let anchor = rail_anchor(&before, &lanes);
    let after = RailRow::Session(Box::new(SessionRailRow {
        local: Some(super::tests::stub_session("w_1")),
        ..match before {
            RailRow::Session(session) => *session,
            _ => unreachable!(),
        }
    }));
    assert_eq!(rail_anchor(&after, &lanes), anchor);
    assert_eq!(
        resolve_rail_cursor(&[RailRow::NewSession, after], &lanes, anchor.as_ref(), 0),
        1
    );
}
