//! Unit tests for the session handle's own helpers.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::types::release_queued;

/// Releasing what was reserved returns the budget to where it started.
#[test]
fn releasing_a_reservation_returns_the_budget() {
    let queued = AtomicUsize::new(4_096);
    release_queued(&queued, 1_024);
    assert_eq!(queued.load(Ordering::Acquire), 3_072);
}

/// Releasing more than is outstanding floors at zero rather than wrapping.
///
/// The race this guards: the writer thread ends by storing 0, while a concurrent
/// `write` may already have reserved bytes it has not sent. When that send then
/// fails, the write path releases a reservation the store has already cleared. On
/// an unsigned counter a bare `fetch_sub` wraps to about `usize::MAX`, which
/// reads as permanently over-quota and refuses every later write to the session.
#[test]
fn releasing_more_than_is_outstanding_floors_at_zero() {
    let queued = AtomicUsize::new(0);
    release_queued(&queued, 1_024);
    assert_eq!(
        queued.load(Ordering::Acquire),
        0,
        "a bare fetch_sub would have wrapped to usize::MAX here"
    );

    let queued = AtomicUsize::new(100);
    release_queued(&queued, 8_192);
    assert_eq!(queued.load(Ordering::Acquire), 0);
}

/// The exact race, played out in order: reserve, writer zeroes, send fails.
///
/// Single-threaded on purpose — the interleaving is what matters, not the
/// timing, and a scheduler-dependent test would only sometimes cover it.
#[test]
fn a_reservation_released_after_the_writer_zeroed_cannot_wrap() {
    let queued = AtomicUsize::new(0);

    // `write` reserves its bytes...
    queued.fetch_add(2_048, Ordering::AcqRel);
    // ...the writer thread gives up and clears the budget on its way out...
    queued.store(0, Ordering::Release);
    // ...and only then does the failed send release what it reserved.
    release_queued(&queued, 2_048);

    assert_eq!(
        queued.load(Ordering::Acquire),
        0,
        "the budget must stay usable for a session that outlives this race"
    );
}
