//! Unit tests for the embedded-core runtime.
//!
//! These exercise the pure fold and the snapshot/notify contract, neither of
//! which needs a core. Anything that genuinely needs one belongs in an
//! integration test: the core uses process globals (a `OnceLock` context, a
//! singleton event bus), so it cannot be stood up and torn down per test.

use crate::runtime::types::StreamState;
use openhuman_core::embed::{RosterWorker, SessionSummary};

use super::cell::SnapshotCell;
use super::fold;
use crate::runtime::types::{AgentDescriptor, ThreadSummary};

fn thread(id: &str) -> ThreadSummary {
    ThreadSummary {
        id: id.to_string(),
        name: String::new(),
        running: false,
        turns: 0,
        running_tasks: 0,
        attention: 0,
    }
}

fn worker(id: &str, label: &str) -> RosterWorker {
    RosterWorker {
        registry_id: id.to_string(),
        label: label.to_string(),
        description: "desc".to_string(),
        availability: "available".to_string(),
        ..Default::default()
    }
}

#[test]
fn roster_fold_preserves_identity_and_order() {
    // Order is the render order, so a reordering fold would shuffle the UI.
    let folded = fold::roster(vec![worker("w1", "First"), worker("w2", "Second")]);
    assert_eq!(
        folded,
        vec![
            AgentDescriptor {
                id: "w1".into(),
                name: "First".into(),
                description: "desc".into(),
                availability: "available".into(),
                ..AgentDescriptor::default()
            },
            AgentDescriptor {
                id: "w2".into(),
                name: "Second".into(),
                description: "desc".into(),
                availability: "available".into(),
                ..AgentDescriptor::default()
            },
        ]
    );
}

#[test]
fn roster_fold_handles_an_empty_roster() {
    assert!(fold::roster(Vec::new()).is_empty());
}

#[test]
fn thread_fold_defaults_a_missing_title_to_empty_not_a_placeholder() {
    // A synthesized name like "(untitled)" would render as if the backend sent
    // it; empty lets the UI apply its own placeholder.
    let folded = fold::threads(vec![SessionSummary {
        session_id: "s1".into(),
        title: None,
        ..Default::default()
    }]);
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].id, "s1");
    assert_eq!(folded[0].name, "");
}

#[test]
fn thread_fold_leaves_activity_counters_at_zero() {
    // The session list carries no per-thread activity. Inventing a count would
    // render as real data and quietly mislead.
    let folded = fold::threads(vec![SessionSummary {
        session_id: "s1".into(),
        title: Some("Work".into()),
        ..Default::default()
    }]);
    assert_eq!(
        (
            folded[0].turns,
            folded[0].running_tasks,
            folded[0].attention
        ),
        (0, 0, 0)
    );
    assert!(!folded[0].running);
}

#[test]
fn cell_starts_empty() {
    let cell = SnapshotCell::new();
    let snap = cell.snapshot();
    assert!(snap.roster.is_empty());
    assert!(snap.threads.is_empty());
}

#[test]
fn apply_replaces_rather_than_appends() {
    // Two refreshes must not accumulate: a roster that only grows would show
    // workers that have since disconnected.
    let cell = SnapshotCell::new();
    let one = vec![AgentDescriptor {
        id: "w1".into(),
        name: "First".into(),
        description: String::new(),
        availability: "available".into(),
        ..AgentDescriptor::default()
    }];
    cell.apply(one.clone(), Vec::new());
    cell.apply(one, Vec::new());
    assert_eq!(cell.snapshot().roster.len(), 1);
}

#[tokio::test]
async fn apply_notifies_subscribers() {
    let cell = SnapshotCell::new();
    let mut rx = cell.subscribe();
    cell.apply(Vec::new(), vec![thread("t1")]);
    assert!(rx.recv().await.is_ok(), "subscriber must be pinged");
    assert_eq!(cell.snapshot().threads.len(), 1);
}

#[test]
fn apply_without_subscribers_is_not_an_error() {
    // The ping is advisory; nobody listening is the normal case at startup.
    let cell = SnapshotCell::new();
    cell.apply(Vec::new(), Vec::new());
    assert!(cell.snapshot().roster.is_empty());
}

#[test]
fn active_thread_survives_a_refresh() {
    // The operator's selection is theirs, not the backend's — a refresh that
    // reset it would move the UI out from under them.
    let cell = SnapshotCell::new();
    cell.set_active_thread("thread-7".into());
    cell.apply(Vec::new(), vec![thread("t1")]);
    assert_eq!(cell.snapshot().active_thread_id, "thread-7");
}

#[test]
fn switching_threads_drops_the_previous_transcript() {
    // Per-thread transcripts: keeping the old rows would render two
    // conversations as one once the incoming thread replays from its start.
    let cell = SnapshotCell::new();
    cell.append_events(vec![render_event(1, "assistant")], Some(true));
    cell.switch_thread("thread-2".into());
    let snap = cell.snapshot();
    assert_eq!(snap.active_thread_id, "thread-2");
    assert!(snap.events.is_empty(), "the old thread's trace is gone");
    assert!(snap.chat_events.is_empty(), "and its transcript with it");
    assert!(
        !snap.running,
        "the old thread's liveness does not carry over"
    );
}

#[test]
fn a_new_thread_starts_from_an_empty_transcript() {
    // Ctrl-N from a populated conversation: the old rows must not sit under the
    // new session, and the liveness of the old turn says nothing about it.
    let cell = SnapshotCell::new();
    cell.append_events(vec![render_event(1, "assistant")], Some(true));
    cell.set_active_thread("thread-1".into());
    cell.switch_thread(String::new());
    let snap = cell.snapshot();
    assert!(snap.events.is_empty(), "the old transcript is gone");
    assert!(snap.chat_events.is_empty());
    assert!(!snap.running);
    assert_eq!(snap.active_thread_id, "", "and nothing is selected yet");
}

#[test]
fn the_cell_reports_whether_a_thread_is_selected() {
    // Startup reads this to decide whether to adopt the first thread: an empty
    // id means the UI is rendering index 0 as active with nothing behind it.
    let cell = SnapshotCell::new();
    assert_eq!(cell.active_thread_id(), "");
    cell.set_active_thread("thread-3".into());
    assert_eq!(cell.active_thread_id(), "thread-3");
}

// ── event translation ────────────────────────────────────────────────────────

use openhuman_core::embed::EventEnvelope as CoreEnvelope;

fn core_event(seq: Option<u64>, kind: &str, body: &str) -> CoreEnvelope {
    CoreEnvelope {
        seq,
        at: 1_700_000_000,
        session_id: "s1".into(),
        cycle_id: Some("c1".into()),
        event: serde_json::json!({ "kind": kind, "body": body }),
    }
}

#[test]
fn events_decode_and_preserve_seq_and_time() {
    let folded = fold::events(vec![core_event(Some(7), "assistant", "hi")]);
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].seq, 7);
    assert_eq!(folded[0].at, 1_700_000_000);
}

#[test]
fn events_without_a_seq_are_dropped_not_zeroed() {
    // Mapping `None` to 0 would make the event sort to the top of the stream
    // and defeat the replay cursor, so a reconnect would re-show it forever.
    let folded = fold::events(vec![
        core_event(None, "assistant", "no seq"),
        core_event(Some(3), "assistant", "has seq"),
    ]);
    assert_eq!(folded.len(), 1, "the seqless envelope must be dropped");
    assert_eq!(folded[0].seq, 3);
}

#[test]
fn an_unrecognized_kind_survives_as_unknown() {
    // A newer backend must not cause an older host to drop rows.
    let folded = fold::events(vec![core_event(Some(1), "some_future_kind", "x")]);
    assert_eq!(folded.len(), 1);
    assert!(matches!(
        folded[0].event,
        crate::ui::events::TuiEvent::Unknown { .. }
    ));
}

#[test]
fn max_seq_is_none_for_an_empty_batch() {
    // The caller reads `None` as "cursor unchanged", never "reset to start".
    assert_eq!(fold::max_seq(&[]), None);
}

#[test]
fn max_seq_takes_the_highest_not_the_last() {
    // Out-of-order delivery must not rewind the cursor.
    let folded = fold::events(vec![
        core_event(Some(9), "assistant", "a"),
        core_event(Some(4), "assistant", "b"),
    ]);
    assert_eq!(fold::max_seq(&folded), Some(9));
}

// ── snapshot event application ───────────────────────────────────────────────

fn render_event(seq: u64, kind: &str) -> crate::ui::events::EventEnvelope {
    fold::events(vec![core_event(Some(seq), kind, "x")])
        .pop()
        .expect("one envelope")
}

#[test]
fn appended_events_accumulate_rather_than_replace() {
    // Events are a growing log; the caller only fetches past its cursor.
    let cell = SnapshotCell::new();
    cell.append_events(vec![render_event(1, "assistant")], Some(true));
    cell.append_events(vec![render_event(2, "assistant")], Some(true));
    assert_eq!(cell.snapshot().events.len(), 2);
}

#[test]
fn a_long_session_trims_the_log_to_its_retention_caps() {
    // A growing log is also a growing deep clone on every `snapshot()`, so the
    // live cell honours the same caps the other runtimes apply.
    use crate::runtime::event_log::{CHAT_CAP, EVENT_CAP};
    let cell = SnapshotCell::new();
    let overflow = 100u64;
    let batch: Vec<_> = (1..=EVENT_CAP as u64 + overflow)
        .map(|seq| render_event(seq, "assistant"))
        .collect();
    cell.append_events(batch, Some(true));
    let snap = cell.snapshot();
    assert_eq!(snap.events.len(), EVENT_CAP);
    assert_eq!(snap.chat_events.len(), CHAT_CAP);
    assert_eq!(
        snap.events[0].seq,
        overflow + 1,
        "the oldest rows are the ones dropped"
    );
}

#[test]
fn only_conversational_rows_reach_the_chat_view() {
    // Trace rows in the transcript would read as something that was said.
    let cell = SnapshotCell::new();
    cell.append_events(
        vec![
            render_event(1, "assistant"),
            render_event(2, "tool_call_start"),
            render_event(3, "user"),
        ],
        Some(true),
    );
    let snap = cell.snapshot();
    assert_eq!(snap.events.len(), 3, "the trace keeps everything");
    assert_eq!(snap.chat_events.len(), 2, "the transcript keeps only turns");
}

#[test]
fn an_empty_batch_still_records_a_settled_turn() {
    // A turn can settle without emitting a final event; the spinner has to stop.
    let cell = SnapshotCell::new();
    cell.append_events(vec![render_event(1, "assistant")], Some(true));
    assert!(cell.snapshot().running);
    cell.append_events(Vec::new(), Some(false));
    assert!(!cell.snapshot().running);
}

#[test]
fn a_batch_with_no_liveness_answer_leaves_the_flag_alone() {
    // Most batches are mid-turn output. Re-asserting `running` on each one is
    // what keeps the spinner turning after the turn has already ended.
    let cell = SnapshotCell::new();
    cell.append_events(vec![render_event(1, "assistant")], Some(false));
    cell.append_events(vec![render_event(2, "assistant")], None);
    assert!(
        !cell.snapshot().running,
        "a straggler must not revive a turn"
    );
    assert_eq!(cell.snapshot().events.len(), 2, "it is still recorded");
}

// ── locally echoed turns ─────────────────────────────────────────────────────

/// The backend's own copy of a turn, as the replay would deliver it.
fn confirmed_user(seq: u64, body: &str) -> crate::ui::events::EventEnvelope {
    fold::events(vec![core_event(Some(seq), "user", body)])
        .pop()
        .expect("one envelope")
}

#[test]
fn a_submitted_turn_is_visible_before_the_backend_confirms_it() {
    // The whole point: the transcript must not wait on a round trip plus a
    // poll interval before showing what the operator just typed.
    let cell = SnapshotCell::new();
    cell.echo_user("hello");
    let snap = cell.snapshot();
    assert_eq!(snap.chat_events.len(), 1, "the turn is drawn immediately");
    assert!(snap.running, "and reads as in flight");
}

#[test]
fn the_confirmed_copy_retires_the_echo_rather_than_doubling_it() {
    // The replay carries the same turn back. Two rows would read as the
    // operator having said it twice.
    let cell = SnapshotCell::new();
    cell.echo_user("hello");
    cell.append_events(
        vec![cycle_event(4, "cycle_start"), confirmed_user(5, "hello")],
        Some(true),
    );
    let snap = cell.snapshot();
    assert_eq!(snap.chat_events.len(), 1, "one turn, not two");
    assert_eq!(snap.chat_events[0].seq, 5, "and it is the confirmed copy");
}

#[test]
fn retiring_an_echo_spares_a_real_event_sharing_its_guessed_seq() {
    // The provisional `seq` is a guess at what the backend will assign, not a
    // reservation, and a batch that carries no turn can land a real event on
    // it. Retiring the echo by `seq` alone takes that event down with it —
    // and a lost `cycle_start` costs the view both its liveness and its turn
    // count.
    let cell = SnapshotCell::new();
    // On an empty cell the guessed `seq` is 1.
    cell.echo_user("hello");
    // Nothing to reconcile in this batch, so the boundary is appended and ends
    // up sharing seq 1 with the provisional row.
    cell.append_events(vec![cycle_event(1, "cycle_start")], Some(true));
    // The confirmation only arrives in the next batch, at its real seq.
    cell.append_events(vec![confirmed_user(2, "hello")], Some(true));

    let snap = cell.snapshot();
    assert!(
        snap.events
            .iter()
            .any(|e| matches!(e.event, crate::ui::events::TuiEvent::CycleStart { .. })),
        "the cycle boundary is not collateral damage"
    );
    assert_eq!(snap.chat_events.len(), 1, "one turn, not two");
    assert_eq!(snap.chat_events[0].seq, 2, "and it is the confirmed copy");
}

#[test]
fn a_confirmed_turn_nobody_echoed_is_kept() {
    // A turn submitted from another client is not this host's echo, and
    // dropping it would hide half the conversation.
    let cell = SnapshotCell::new();
    cell.append_events(vec![confirmed_user(1, "from elsewhere")], Some(true));
    assert_eq!(cell.snapshot().chat_events.len(), 1);
}

#[test]
fn two_echoes_retire_one_confirmation_at_a_time() {
    // Submitting the same text twice is ordinary. Matching by body alone must
    // still leave one provisional row standing after the first confirmation.
    let cell = SnapshotCell::new();
    cell.echo_user("again");
    cell.echo_user("again");
    cell.append_events(vec![confirmed_user(9, "again")], Some(true));
    assert_eq!(
        cell.snapshot().chat_events.len(),
        2,
        "one confirmed, one still awaited"
    );
}

#[test]
fn an_echo_the_backend_never_took_settles_the_turn_but_keeps_the_text() {
    // The send failed. Nothing is running, so nothing should say it is — but
    // the operator wrote that line and it must not vanish with the error.
    let cell = SnapshotCell::new();
    let seq = cell.echo_user("undeliverable");
    cell.abandon_echo(seq);
    let snap = cell.snapshot();
    assert!(!snap.running, "the spinner stops");
    assert_eq!(
        snap.chat_events.len(),
        1,
        "the turn stays in the transcript"
    );
}

#[test]
fn silence_past_the_deadline_settles_the_turn_but_keeps_the_text() {
    // The backend accepted the turn and then said nothing at all. Spinning on
    // claims work is in flight that this host has no evidence of.
    let cell = SnapshotCell::new();
    cell.echo_user("accepted then silence");
    assert!(cell.expire_stalled_echo(std::time::Duration::ZERO));
    let snap = cell.snapshot();
    assert!(!snap.running, "the spinner stops");
    assert_eq!(
        snap.chat_events.len(),
        1,
        "the turn stays in the transcript"
    );
}

#[test]
fn a_run_of_unconfirmed_echoes_still_respects_the_retention_caps() {
    // Each echo files a provisional row, and only the backend's confirmed copy
    // retires it. A dead or stalled backend never sends one, so the echo path
    // alone must enforce the same caps `append_events` applies to batches —
    // otherwise its rows grow without bound.
    use crate::runtime::event_log::{CHAT_CAP, EVENT_CAP};
    let cell = SnapshotCell::new();
    let overflow = 100u64;
    for _ in 0..EVENT_CAP as u64 + overflow {
        cell.echo_user("hello");
    }
    let snap = cell.snapshot();
    assert_eq!(snap.events.len(), EVENT_CAP, "the full log honours the cap");
    assert_eq!(
        snap.chat_events.len(),
        CHAT_CAP,
        "the chat view honours its own (smaller) cap"
    );
    assert_eq!(
        snap.events[0].seq,
        overflow as u64 + 1,
        "the oldest rows are the ones dropped"
    );
}

#[test]
fn the_watchdog_is_disarmed_by_the_first_batch_to_arrive() {
    // Once the backend is talking, liveness is the batch's to report — a
    // watchdog still armed would settle a turn that is genuinely producing.
    let cell = SnapshotCell::new();
    cell.echo_user("working");
    cell.append_events(vec![cycle_event(1, "cycle_start")], Some(true));
    assert!(!cell.expire_stalled_echo(std::time::Duration::ZERO));
    assert!(cell.snapshot().running, "a live turn keeps its spinner");
}

#[test]
fn the_watchdog_fires_once_rather_than_every_tick() {
    // It runs on the poll loop, which wakes far more often than the deadline.
    let cell = SnapshotCell::new();
    cell.echo_user("quiet");
    assert!(cell.expire_stalled_echo(std::time::Duration::ZERO));
    assert!(!cell.expire_stalled_echo(std::time::Duration::ZERO));
}

#[test]
fn an_unconfirmed_echo_does_not_outlive_its_thread() {
    // Left pending, it would match the incoming thread's replay by body and
    // retire a row this host never drew.
    let cell = SnapshotCell::new();
    cell.echo_user("shared text");
    cell.switch_thread("other".into());
    cell.append_events(vec![confirmed_user(1, "shared text")], Some(true));
    assert_eq!(
        cell.snapshot().chat_events.len(),
        1,
        "the other thread's turn survives"
    );
}

// ── liveness from cycle boundaries ───────────────────────────────────────────

fn cycle_event(seq: u64, kind: &str) -> crate::ui::events::EventEnvelope {
    fold::events(vec![CoreEnvelope {
        seq: Some(seq),
        at: 1_700_000_000,
        session_id: "s1".into(),
        cycle_id: Some("c1".into()),
        event: serde_json::json!({ "kind": kind, "cycleId": "c1" }),
    }])
    .pop()
    .expect("one envelope")
}

#[test]
fn a_batch_without_a_cycle_boundary_says_nothing_about_liveness() {
    assert_eq!(
        fold::running_after(&[render_event(1, "assistant")]),
        None,
        "output alone is not a liveness signal"
    );
    assert_eq!(fold::running_after(&[]), None);
}

#[test]
fn a_cycle_end_settles_the_turn() {
    // The finding this guards: a batch containing the terminal event used to
    // report `running = true`, so the orchestrator rendered busy forever.
    assert_eq!(
        fold::running_after(&[render_event(1, "assistant"), cycle_event(2, "cycle_end")]),
        Some(false)
    );
}

#[test]
fn a_cycle_start_marks_the_turn_running() {
    assert_eq!(
        fold::running_after(&[cycle_event(1, "cycle_start")]),
        Some(true)
    );
}

#[test]
fn the_highest_seq_boundary_wins_not_the_last_in_the_vec() {
    // Out-of-order delivery must not settle a turn that has since restarted.
    let out_of_order = vec![cycle_event(9, "cycle_start"), cycle_event(4, "cycle_end")];
    assert_eq!(fold::running_after(&out_of_order), Some(true));
}

#[tokio::test]
async fn appending_events_notifies_subscribers() {
    let cell = SnapshotCell::new();
    let mut rx = cell.subscribe();
    cell.append_events(vec![render_event(1, "assistant")], Some(true));
    assert!(rx.recv().await.is_ok());
}

// ── stream health ────────────────────────────────────────────────────────────

#[test]
fn a_succeeding_poll_reads_as_live() {
    assert_eq!(fold::stream_state(0, 5), StreamState::Live);
}

#[test]
fn a_single_blip_resyncs_rather_than_stalling() {
    // One failed poll is a restart or a dropped connection, not an outage;
    // flipping straight to `Stalled` would flap the header on every hiccup.
    assert_eq!(fold::stream_state(1, 5), StreamState::Resyncing);
    assert_eq!(fold::stream_state(5, 5), StreamState::Resyncing);
}

#[test]
fn sustained_failure_reports_stalled() {
    // Past the threshold the screen is stale and must say so.
    assert_eq!(fold::stream_state(6, 5), StreamState::Stalled);
}

#[test]
fn the_roster_mapping_carries_a_workers_roles_to_the_ui() {
    // The Hosts preview reads roles off `WorkerInfo`, and a worker that reaches
    // it roleless is indistinguishable from one the operator never assigned —
    // so a dropped field here reads as "the toggles do nothing", not as a bug.
    let worker = crate::hub::HubWorker {
        id: "w1".into(),
        address: "@w1".into(),
        harness: "claude".into(),
        label: None,
        selected: false,
        roles: vec!["code-reviewer".into(), "test-writer".into()],
        workspace: Some(crate::runtime::WorkspaceRef::checkout("/work")),
        ..Default::default()
    };

    let info = super::worker_ops::hub_worker_to_info(worker, None);
    assert_eq!(info.roles, vec!["code-reviewer", "test-writer"]);
    assert_eq!(info.handle.as_deref(), Some("@w1"));
}
