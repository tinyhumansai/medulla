//! `HarnessAttention` semantics: precedence between cue kinds, first-seen time
//! preservation, and how a cue's label reports how long it has waited.

use super::super::types::{AttentionKind, HarnessAttention};
#[test]
fn a_named_cue_outranks_a_bell() {
    let bell = HarnessAttention::new(AttentionKind::Bell, "rang", 0);
    let approval = HarnessAttention::new(AttentionKind::Approval, "asking", 10);
    assert!(approval.supersedes(&bell));
    assert!(!bell.supersedes(&approval));
}

#[test]
fn the_same_cue_keeps_its_first_seen_time() {
    let first = HarnessAttention::new(AttentionKind::Approval, "asking", 100);
    let again = HarnessAttention::new(AttentionKind::Approval, "asking", 900);
    // Equal kinds do not displace, which is what preserves `since` — a prompt
    // repainted every frame must not read as newly arrived every frame.
    assert!(!again.supersedes(&first));
}

#[test]
fn the_label_states_how_long_it_has_waited() {
    let cue = HarnessAttention::new(
        AttentionKind::Approval,
        "claude is asking permission",
        1_000,
    );
    assert_eq!(cue.label(1_400), "claude is asking permission");
    assert_eq!(cue.label(13_000), "claude is asking permission · 12s");
}
/// Precedence across the whole vocabulary, in one place: a cue that names a
/// harder fact must never be displaced by a vaguer one.
#[test]
fn the_cue_vocabulary_is_ordered_from_certain_to_vague() {
    let kinds = [
        AttentionKind::Failed,
        AttentionKind::Dialog,
        AttentionKind::Approval,
        AttentionKind::Error,
        AttentionKind::Choice,
        AttentionKind::Completed,
        AttentionKind::Bell,
    ];
    for window in kinds.windows(2) {
        let stronger = HarnessAttention::new(window[0], "a", 0);
        let weaker = HarnessAttention::new(window[1], "b", 0);
        assert!(
            stronger.supersedes(&weaker),
            "{:?} must outrank {:?}",
            window[0],
            window[1]
        );
        assert!(!weaker.supersedes(&stronger));
    }
}
