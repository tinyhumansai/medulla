//! Tests for the hub's [`ActivityLog`]: the in-memory record of what the hub's
//! workers are doing right now.
//!
//! Three behaviours matter and are pinned here. A frame must land on the worker
//! its *task* was dispatched to, not on whichever worker sorted first — the log
//! is useless if it attributes work to the wrong lane. A terminal frame
//! (`reply`/`error`) must end a running task, so the screen and the peer agree
//! on whether work is outstanding. And the ring must drop the *oldest* entry
//! when it fills, because a worker left running for a week must not accumulate
//! its whole past in memory.

use super::super::activity::{ActivityLog, WorkerActivity};

/// The ring's retention limit. Mirrors the private `CAPACITY` in the module
/// under test: if the product changes it, these bound tests should be revisited.
const CAPACITY: usize = 512;

/// The attribution map's retention limit. Mirrors `ATTRIBUTION_CAPACITY`.
const ATTRIBUTION_CAPACITY: usize = 512;

#[test]
fn a_frame_is_attributed_to_the_worker_its_task_was_dispatched_to() {
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.dispatched("task-2", "worker-b");

    log.observed("task-1", "status", "reading files", 10);
    log.observed("task-2", "status", "writing patch", 11);

    let snap = log.snapshot();
    let first = snap.iter().find(|e| e.task_id == "task-1").unwrap();
    let second = snap.iter().find(|e| e.task_id == "task-2").unwrap();
    assert_eq!(
        first.agent_id, "worker-a",
        "a frame must be attributed to the worker its task was dispatched to"
    );
    assert_eq!(second.agent_id, "worker-b");
}

#[test]
fn a_frame_for_a_task_never_dispatched_here_is_left_unattributed() {
    // The backend broadcasts to every harness, so a frame can arrive for a task
    // this hub never sent. It must be recorded, but attributed to no worker
    // rather than guessed onto an arbitrary lane.
    let log = ActivityLog::new();
    log.observed("stranger-task", "reply", "done elsewhere", 5);

    let snap = log.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(
        snap[0].agent_id, "",
        "an unattributed frame carries an empty agent id, not a fabricated one"
    );
}

#[test]
fn redispatching_a_task_moves_it_to_the_worker_that_now_owns_it() {
    // The same wire task id can be reused; the latest dispatch is the truth.
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.dispatched("task-1", "worker-b");

    log.observed("task-1", "status", "on it", 1);
    let snap = log.snapshot();
    assert_eq!(
        snap[0].agent_id, "worker-b",
        "after redispatch the frame belongs to the worker that now owns the task"
    );
}

#[test]
fn observed_records_the_frame_verbatim() {
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.observed("task-1", "ack", "picked it up", 42);

    assert_eq!(
        log.snapshot()[0],
        WorkerActivity {
            agent_id: "worker-a".to_string(),
            task_id: "task-1".to_string(),
            kind: "ack".to_string(),
            content: "picked it up".to_string(),
            at: 42,
        }
    );
}

#[test]
fn a_task_with_only_non_terminal_frames_counts_as_running() {
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.observed("task-1", "ack", "", 1);
    log.observed("task-1", "status", "working", 2);

    let running = log.running_by_agent();
    assert_eq!(
        running.get("worker-a").map(Vec::as_slice),
        Some(["task-1".to_string()].as_slice()),
        "a dispatched task with no terminal frame is still outstanding"
    );
}

#[test]
fn a_terminal_reply_frame_ends_a_running_task() {
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.observed("task-1", "status", "working", 1);
    assert!(
        log.running_by_agent().contains_key("worker-a"),
        "running before the reply"
    );

    log.observed("task-1", "reply", "shipped it", 2);
    assert!(
        !log.running_by_agent().contains_key("worker-a"),
        "a reply frame ends the task, so the worker has nothing outstanding"
    );
}

#[test]
fn an_error_frame_also_ends_a_running_task() {
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.observed("task-1", "status", "working", 1);
    log.observed("task-1", "error", "it blew up", 2);

    assert!(
        !log.running_by_agent().contains_key("worker-a"),
        "an error is terminal too — the task is no longer running"
    );
}

#[test]
fn a_terminal_frame_out_of_order_still_ends_the_task() {
    // Frames fold order-independently: a status arriving after the reply (a
    // late-delivered ack, say) must not resurrect a finished task.
    let log = ActivityLog::new();
    log.dispatched("task-1", "worker-a");
    log.observed("task-1", "reply", "done", 1);
    log.observed("task-1", "status", "late status", 2);

    assert!(
        !log.running_by_agent().contains_key("worker-a"),
        "once terminal, a task stays terminal regardless of later non-terminal frames"
    );
}

#[test]
fn running_tasks_are_grouped_by_their_worker() {
    let log = ActivityLog::new();
    log.dispatched("t1", "worker-a");
    log.dispatched("t2", "worker-a");
    log.dispatched("t3", "worker-b");
    log.observed("t1", "status", "", 1);
    log.observed("t2", "status", "", 2);
    log.observed("t3", "status", "", 3);
    // t2 finishes; t1 and t3 keep running.
    log.observed("t2", "reply", "done", 4);

    let running = log.running_by_agent();
    let mut a = running.get("worker-a").cloned().unwrap_or_default();
    a.sort();
    assert_eq!(
        a,
        vec!["t1".to_string()],
        "only t1 is still open for worker-a"
    );
    assert_eq!(
        running.get("worker-b").map(Vec::as_slice),
        Some(["t3".to_string()].as_slice())
    );
}

#[test]
fn snapshot_returns_entries_oldest_first() {
    let log = ActivityLog::new();
    log.dispatched("t", "w");
    for i in 0..5 {
        log.observed("t", "status", &format!("step {i}"), i);
    }
    let contents: Vec<String> = log.snapshot().into_iter().map(|e| e.content).collect();
    assert_eq!(
        contents,
        vec!["step 0", "step 1", "step 2", "step 3", "step 4"],
        "the snapshot preserves observation order, oldest first"
    );
}

#[test]
fn the_ring_drops_the_oldest_entry_at_capacity() {
    let log = ActivityLog::new();
    log.dispatched("t", "w");
    let overflow = 10;
    // Push CAPACITY + overflow frames, each tagged with its index in `content`.
    for i in 0..(CAPACITY + overflow) {
        log.observed("t", "status", &format!("{i}"), i as i64);
    }

    let snap = log.snapshot();
    assert_eq!(
        snap.len(),
        CAPACITY,
        "the ring is bounded and never grows past its capacity"
    );
    // The first `overflow` entries are the ones that must have been dropped.
    assert_eq!(
        snap.first().unwrap().content,
        overflow.to_string(),
        "the OLDEST entries are evicted, so the survivors begin at index {overflow}"
    );
    assert_eq!(
        snap.last().unwrap().content,
        (CAPACITY + overflow - 1).to_string(),
        "the newest entry is retained"
    );
}

#[test]
fn the_attribution_map_is_bounded_and_forgets_the_oldest_dispatch() {
    let log = ActivityLog::new();
    // The first task dispatched is the one that should fall out once the map is
    // overfull.
    log.dispatched("first-task", "worker-first");
    for i in 0..ATTRIBUTION_CAPACITY {
        log.dispatched(&format!("task-{i}"), "worker-later");
    }

    // The oldest attribution has been evicted, so a frame for it is orphaned.
    log.observed("first-task", "status", "", 1);
    let snap = log.snapshot();
    let orphan = snap.iter().find(|e| e.task_id == "first-task").unwrap();
    assert_eq!(
        orphan.agent_id, "",
        "the oldest attribution is dropped once the map overflows, orphaning its frames"
    );

    // A recent attribution is still remembered.
    log.observed("task-0", "status", "", 2);
    let recent = log
        .snapshot()
        .into_iter()
        .find(|e| e.task_id == "task-0")
        .unwrap();
    assert_eq!(recent.agent_id, "worker-later");
}

#[test]
fn redispatch_does_not_leave_a_stale_duplicate_attribution() {
    // `dispatched` retains-then-pushes, so re-dispatching the same id must not
    // grow the map with a stale pair that a later `retain` might race.
    let log = ActivityLog::new();
    for _ in 0..1000 {
        log.dispatched("same-task", "worker-a");
    }
    // If duplicates accumulated, this single distinct task would have evicted
    // its own attribution long ago. It must still resolve.
    log.observed("same-task", "status", "", 1);
    assert_eq!(log.snapshot()[0].agent_id, "worker-a");
}

#[test]
fn a_default_log_starts_empty() {
    let log = ActivityLog::default();
    assert!(log.snapshot().is_empty());
    assert!(log.running_by_agent().is_empty());
}

#[test]
fn clones_share_one_underlying_ring() {
    // The type is documented as cheap to clone with every clone reading and
    // writing the same ring; a write through one clone must be visible through
    // another.
    let log = ActivityLog::new();
    let other = log.clone();
    log.dispatched("t", "w");
    other.observed("t", "status", "via clone", 1);

    assert_eq!(
        log.snapshot().len(),
        1,
        "a write through one clone is seen through the other"
    );
    assert_eq!(log.snapshot()[0].agent_id, "w");
}
