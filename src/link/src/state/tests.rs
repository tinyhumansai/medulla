//! Tests for the two synchronised state types (protocol §4.4, §4.5).

use super::*;

fn queue() -> MessageQueue {
    MessageQueue::new(QueueLimits::default())
}

#[test]
fn a_queue_diff_carries_exactly_the_messages_the_peer_is_missing() {
    let mut sender = queue();
    sender.push(b"one".to_vec()).unwrap();
    let seen = sender.clone();
    sender.push(b"two".to_vec()).unwrap();
    sender.push(b"three".to_vec()).unwrap();

    let mut receiver = seen.clone();
    receiver.apply_diff(&sender.diff_from(&seen)).unwrap();
    assert_eq!(receiver.num(), sender.num());
    assert_eq!(
        receiver.pending(),
        [b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
    );
}

#[test]
fn the_state_number_is_the_count_of_messages_ever_appended() {
    let mut queue = queue();
    assert_eq!(queue.num(), 0);
    queue.push(b"a".to_vec()).unwrap();
    queue.push(b"b".to_vec()).unwrap();
    assert_eq!(queue.num(), 2);
    // Draining hands the messages upward without renumbering the stream.
    assert_eq!(queue.drain().len(), 2);
    assert_eq!(queue.num(), 2);
    queue.push(b"c".to_vec()).unwrap();
    assert_eq!(queue.num(), 3);
}

#[test]
fn pruning_frees_delivered_messages_and_keeps_the_numbering() {
    let mut queue = queue();
    for index in 0..5 {
        queue.push(vec![index]).unwrap();
    }
    queue.prune_through(3);
    assert_eq!(queue.num(), 5);
    assert_eq!(queue.pending(), [vec![3], vec![4]]);
    // Below the base is a no-op, not a rewind.
    queue.prune_through(1);
    assert_eq!(queue.pending(), [vec![3], vec![4]]);
}

#[test]
fn a_pruned_sender_still_diffs_correctly_to_the_state_the_peer_holds() {
    let mut sender = queue();
    sender.push(b"one".to_vec()).unwrap();
    sender.push(b"two".to_vec()).unwrap();
    let acked = sender.clone();
    sender.push(b"three".to_vec()).unwrap();
    // The peer acknowledged two messages, so the sender frees them.
    sender.release_through(2);

    let mut receiver = acked.clone();
    receiver.apply_diff(&sender.diff_from(&acked)).unwrap();
    assert_eq!(receiver.num(), 3);
    assert_eq!(receiver.pending().last().unwrap(), b"three");
}

#[test]
fn the_queue_bound_is_enforced_and_leaves_the_state_unchanged() {
    let mut queue = MessageQueue::new(QueueLimits {
        max_messages: 2,
        max_bytes: 1024,
    });
    queue.push(b"a".to_vec()).unwrap();
    queue.push(b"b".to_vec()).unwrap();
    let error = queue.push(b"c".to_vec()).unwrap_err();
    assert!(matches!(error, StateError::Overflow(_)));
    assert_eq!(queue.num(), 2);
}

#[test]
fn the_byte_bound_is_enforced_too() {
    let mut queue = MessageQueue::new(QueueLimits {
        max_messages: 100,
        max_bytes: 8,
    });
    queue.push(vec![0u8; 8]).unwrap();
    assert!(matches!(
        queue.push(vec![0u8; 1]),
        Err(StateError::Overflow(_))
    ));
}

#[test]
fn applying_a_diff_that_would_overflow_leaves_the_state_unchanged() {
    let mut sender = queue();
    for index in 0..4 {
        sender.push(vec![index]).unwrap();
    }
    let mut receiver = MessageQueue::new(QueueLimits {
        max_messages: 2,
        max_bytes: 1024,
    });
    let error = receiver
        .apply_diff(&sender.diff_from(&queue()))
        .unwrap_err();
    assert!(matches!(error, StateError::Overflow(_)));
    assert_eq!(receiver.num(), 0);
}

#[test]
fn a_malformed_queue_diff_is_refused_and_changes_nothing() {
    let mut receiver = queue();
    receiver.push(b"kept".to_vec()).unwrap();
    for bad in [
        vec![0u8, 0, 0],                      // truncated count
        vec![0u8, 0, 0, 1],                   // a message is promised
        vec![0u8, 0, 0, 1, 0, 0, 0, 9, 1, 2], // over-declared length
        vec![0u8, 0, 0, 0, 7],                // trailing bytes
    ] {
        assert!(matches!(
            receiver.apply_diff(&bad),
            Err(StateError::Malformed(_))
        ));
    }
    assert_eq!(receiver.num(), 1);
    assert_eq!(receiver.pending(), [b"kept".to_vec()]);
}

#[test]
fn an_empty_diff_is_a_no_op() {
    let mut receiver = queue();
    let empty = queue().diff_from(&queue());
    receiver.apply_diff(&empty).unwrap();
    assert_eq!(receiver.num(), 0);
}

// ------------------------------------------------------------------- grid

#[test]
fn a_grid_diff_carries_only_the_rows_that_changed() {
    let mut previous = RowGrid::new();
    previous.set_rows(vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
    let mut current = previous.clone();
    current.set_row(1, b"TWO".to_vec());

    let diff = current.diff_from(&previous);
    let mut receiver = previous.clone();
    receiver.apply_diff(&diff).unwrap();
    assert_eq!(receiver, current);
    // One row plus the two counts and the row's index/length headers.
    assert_eq!(diff.len(), 4 + 4 + 4 + 4 + 3);
}

#[test]
fn a_grid_catches_up_in_one_diff_however_many_updates_it_missed() {
    // The reason channel 1 exists (§4.4): latest wins, no replay.
    let stale = RowGrid::new();
    let mut current = RowGrid::new();
    for generation in 0..500 {
        current.set_rows(vec![format!("frame {generation}").into_bytes()]);
    }
    let mut receiver = stale.clone();
    receiver.apply_diff(&current.diff_from(&stale)).unwrap();
    assert_eq!(receiver, current);
    assert_eq!(receiver.rows(), [b"frame 499".to_vec()]);
}

#[test]
fn a_shrinking_grid_drops_its_extra_rows() {
    let mut previous = RowGrid::new();
    previous.set_rows(vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    let mut current = RowGrid::new();
    current.set_rows(vec![b"a".to_vec()]);

    let mut receiver = previous.clone();
    receiver.apply_diff(&current.diff_from(&previous)).unwrap();
    assert_eq!(receiver, current);
}

#[test]
fn a_malformed_grid_diff_is_refused() {
    let mut receiver = RowGrid::new();
    receiver.set_rows(vec![b"kept".to_vec()]);
    for bad in [
        vec![0u8, 0, 0, 1],                                        // truncated
        vec![0u8, 0, 0, 1, 0, 0, 0, 1],                            // a row is promised
        vec![0u8, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0, 4, 1], // over-declared
    ] {
        assert!(matches!(
            receiver.apply_diff(&bad),
            Err(StateError::Malformed(_))
        ));
    }
    assert_eq!(receiver.rows(), [b"kept".to_vec()]);
}

#[test]
fn a_grid_ignores_the_throwaway_hint() {
    // Latest-wins holds no history, so there is nothing to release.
    let mut grid = RowGrid::new();
    grid.set_rows(vec![b"a".to_vec()]);
    grid.release_through(99);
    assert_eq!(grid.rows(), [b"a".to_vec()]);
}
