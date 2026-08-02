//! Unit tests for PTY manager bookkeeping rules.

use super::session::consumed_bell_count;

#[test]
fn a_stale_bell_sample_cannot_move_the_watermark_backwards() {
    // Release sampled one bell, then an in-flight classifier stored the second
    // before release reacquired the sessions lock.
    assert_eq!(consumed_bell_count(2, 1), 2);
}
