//! Tests for the derived state the Agents surface reads — currently which rail
//! row counts as the operator's own conversation.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::agents::AgentRow;
use medulla::ui::events::{EventEnvelope, TuiEvent};

use super::rail::RailRow;
use super::types::App;

/// An app on the demo runtime with a busy agent, so the rail carries every row
/// kind at once: the orchestrator's lane, an agent, its sessions, the `+N more`
/// overflow control and the `+ new session` action.
fn app_with_a_busy_agent() -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    let base = app.snapshot.events.len() as u64;
    for i in 0..25 {
        let seq = base + i;
        app.snapshot.events.push(EventEnvelope {
            seq,
            at: seq as i64 * 1000,
            event: TuiEvent::TaskStart {
                task_id: format!("dev-t{i}"),
                instruction: "x".into(),
                depth: 2,
                agent_id: Some("dev".into()),
                contract: None,
            },
        });
    }
    app
}

#[test]
fn only_a_lanes_own_row_is_the_orchestrators_conversation() {
    // The composer's visibility hangs off this answer, so every row that is not
    // a lane must say no. The dangerous shape is a row that *wraps* a lane
    // without being one: `RailRow::Lane` also carries `AgentRow::More`, the
    // overflow control, and the `── functions ──` divider, which names no lane
    // at all and so used to fall through to the "no lanes yet ⇒ the
    // orchestrator is all there is" fallback.
    let mut app = app_with_a_busy_agent();
    let rows = app.rail_rows();
    assert!(
        rows.iter()
            .any(|row| matches!(row, RailRow::Lane(AgentRow::More { .. }))),
        "25 tasks overflow one page: {rows:?}"
    );

    for (index, row) in rows.iter().enumerate() {
        if matches!(row, RailRow::Lane(AgentRow::Lane { .. })) {
            continue;
        }
        app.agent_index = index;
        assert!(
            !app.on_orchestrator_lane(),
            "row {index} ({row:?}) is not a lane and must not read as the orchestrator"
        );
    }
}

#[test]
fn the_orchestrators_own_lane_row_still_is() {
    // The other half of the same match: narrowing it must not cost the row it
    // exists to recognise.
    let mut app = app_with_a_busy_agent();
    let index = app
        .orchestrator_row_index()
        .expect("the demo runtime folds an orchestrator lane");

    app.agent_index = index;
    assert!(app.on_orchestrator_lane());
}
