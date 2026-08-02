//! Tests for node identity, the pair-key codec, and the sequence reservation.

use std::path::Path;

use super::*;

fn state() -> NodeState {
    NodeState {
        version: 1,
        node_id: NodeId([1u8; 16]),
        role: Role::Host,
        pair_key: PairKey::from_bytes([2u8; 16]),
        forwarder_key: ForwarderKey([3u8; 32]),
        forwarder_endpoint: "link.example:4600".to_string(),
        peer_node_id: NodeId([4u8; 16]),
        seq_reservation: 1,
    }
}

// ---------------------------------------------------------------- pair key

#[test]
fn a_pair_key_round_trips_through_its_display_form() {
    let key = PairKey::generate();
    let typed = key.encode();
    assert_eq!(PairKey::decode(&typed).unwrap(), key);
}

#[test]
fn the_display_form_is_seven_groups_of_four() {
    let encoded = PairKey::from_bytes([0u8; 16]).encode();
    let groups: Vec<&str> = encoded.split('-').collect();
    assert_eq!(groups.len(), 7);
    assert!(groups.iter().all(|group| group.len() == 4));
    assert_eq!(encoded.len(), 34);
}

#[test]
fn the_encoding_uses_only_crockford_characters() {
    for _ in 0..64 {
        let encoded = PairKey::generate().encode_compact();
        assert_eq!(encoded.len(), 28);
        assert!(
            encoded.chars().all(|ch| !"ILOU".contains(ch)),
            "{encoded} contains a character Crockford base32 excludes"
        );
    }
}

#[test]
fn a_mistyped_character_is_caught_by_the_checksum() {
    // The point of §7.1: a typo fails at entry, not later as an opaque decrypt
    // failure.
    let key = PairKey::from_bytes([9u8; 16]);
    let encoded = key.encode_compact();
    let mut rejected = 0;
    for index in 0..encoded.len() {
        let mut typo: Vec<char> = encoded.chars().collect();
        typo[index] = if typo[index] == '7' { '8' } else { '7' };
        let typed: String = typo.into_iter().collect();
        if typed != encoded {
            assert_eq!(
                PairKey::decode(&typed),
                Err(PairKeyError::Checksum),
                "a single-character typo at {index} was accepted"
            );
            rejected += 1;
        }
    }
    assert!(rejected > 20);
}

#[test]
fn confusable_characters_fold_instead_of_failing() {
    let key = PairKey::from_bytes([0u8; 16]);
    let canonical = key.encode();
    // 0/O and 1/I/L are exactly the transcription errors Crockford anticipates.
    let mistyped = canonical.replace('0', "O").replace('1', "I");
    assert_eq!(PairKey::decode(&mistyped).unwrap(), key);
    let mistyped_l = canonical.replace('1', "l");
    assert_eq!(PairKey::decode(&mistyped_l).unwrap(), key);
}

#[test]
fn separators_and_case_are_ignored() {
    let key = PairKey::generate();
    let typed = key.encode().to_lowercase().replace('-', " ");
    assert_eq!(PairKey::decode(&typed).unwrap(), key);
    assert_eq!(PairKey::decode(&key.encode_compact()).unwrap(), key);
}

#[test]
fn a_wrong_length_and_an_excluded_character_are_named_distinctly() {
    assert_eq!(PairKey::decode("K3M9"), Err(PairKeyError::Length(4)));
    // `U` is excluded from the alphabet and is not folded: guessing at what the
    // user meant would defeat the checksum.
    assert_eq!(
        PairKey::decode("UUUU-UUUU-UUUU-UUUU-UUUU-UUUU-UUUU"),
        Err(PairKeyError::InvalidCharacter('U'))
    );
}

#[test]
fn a_pair_key_never_prints_itself() {
    let rendered = format!("{:?}", PairKey::from_bytes([1u8; 16]));
    assert_eq!(rendered, "PairKey(<redacted>)");
    assert!(!format!("{:?}", state()).contains("pair_key"));
}

// ------------------------------------------------------------- identity file

#[test]
fn an_identity_is_created_then_loaded_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    let created = acquire_or_create(&path, state).unwrap();
    assert_eq!(created.state.node_id, state().node_id);
    drop(created);

    let loaded = acquire(&path).unwrap();
    assert_eq!(loaded.state.node_id, state().node_id);
    assert_eq!(loaded.state.pair_key, state().pair_key);
    assert_eq!(loaded.state.forwarder_endpoint, state().forwarder_endpoint);
}

#[test]
fn a_missing_identity_is_reported_rather_than_invented() {
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    assert!(matches!(acquire(&path), Err(KeyError::NotEnrolled(_))));
}

#[test]
fn a_malformed_identity_is_reported_not_replaced() {
    // It holds the only copy of a pair key, and there is no key recovery (§7.2).
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(node_path(&path), "{ not json").unwrap();
    assert!(matches!(
        acquire_or_create(&path, state),
        Err(KeyError::Malformed { .. })
    ));
    assert_eq!(
        std::fs::read_to_string(node_path(&path)).unwrap(),
        "{ not json"
    );
}

#[cfg(unix)]
#[test]
fn the_identity_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    acquire_or_create(&path, state).unwrap();
    let mode = std::fs::metadata(node_path(&path))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_second_process_cannot_hold_the_same_identity() {
    // Two holders would draw sequences from two counters under one AEAD key.
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    let held = acquire_or_create(&path, state).unwrap();
    assert!(matches!(acquire(&path), Err(KeyError::Busy(_))));
    drop(held);
    assert!(acquire(&path).is_ok());
}

// ------------------------------------------------------------- sequence

#[test]
fn sequences_start_at_one_and_increase() {
    let mut seq = MemorySeq::default();
    assert_eq!(seq.next_seq().unwrap(), 1);
    assert_eq!(seq.next_seq().unwrap(), 2);
    assert_eq!(MemorySeq::new(0).next_seq().unwrap(), 1);
}

#[test]
fn a_reservation_is_persisted_before_any_sequence_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    let mut node = acquire_or_create(&path, state).unwrap();
    assert_eq!(node.seq.next_seq().unwrap(), 1);
    assert_eq!(
        reservation_on_disk(&node_path(&path)),
        1 + RESERVATION_BLOCK
    );
}

#[test]
fn a_crash_mid_block_skips_forward_and_never_rewinds() {
    // The §3.1 requirement: a restart may waste sequences, never reuse one.
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());

    let mut node = acquire_or_create(&path, state).unwrap();
    let used: Vec<u64> = (0..5).map(|_| node.seq.next_seq().unwrap()).collect();
    assert_eq!(used, vec![1, 2, 3, 4, 5]);
    // Dropping without a clean shutdown is exactly the crash case.
    drop(node);

    let reopened = acquire(&path).unwrap();
    assert!(
        reopened.seq.peek() > *used.last().unwrap(),
        "reopening rewound the counter to {}",
        reopened.seq.peek()
    );
    assert_eq!(reopened.seq.peek(), 1 + RESERVATION_BLOCK);
}

#[test]
fn consuming_a_block_reserves_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = link_dir(dir.path());
    let mut node = acquire_or_create(&path, state).unwrap();
    for _ in 0..RESERVATION_BLOCK {
        node.seq.next_seq().unwrap();
    }
    assert_eq!(node.seq.next_seq().unwrap(), 1 + RESERVATION_BLOCK);
    assert_eq!(
        reservation_on_disk(&node_path(&path)),
        1 + 2 * RESERVATION_BLOCK
    );
}

/// The reservation recorded in the on-disk identity.
fn reservation_on_disk(path: &Path) -> u64 {
    let contents = std::fs::read_to_string(path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
    parsed["seq_reservation"].as_u64().unwrap()
}
