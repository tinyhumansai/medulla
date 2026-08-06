use base64::Engine;

use super::osc52::Osc52Scanner;

/// Feed `bytes` through a fresh scanner, returning the last `Some` it produced.
fn scan(bytes: &[u8]) -> Option<String> {
    let mut scanner = Osc52Scanner::default();
    let mut last = None;
    for &byte in bytes {
        if let Some(text) = scanner.advance(byte) {
            last = Some(text);
        }
    }
    last
}

fn osc52_set(text: &str) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut seq = Vec::new();
    seq.extend_from_slice(b"\x1b]52;c;");
    seq.extend_from_slice(encoded.as_bytes());
    seq.push(0x07);
    seq
}

#[test]
fn captures_a_short_copy() {
    assert_eq!(scan(&osc52_set("hello")), Some("hello".to_string()));
}

#[test]
fn captures_a_copy_past_vtes_1024_byte_osc_buffer() {
    // Comfortably past the 1024-byte `ArrayVec` `vte`'s `no_std` build backs
    // its OSC accumulator with — the whole point of this scanner.
    let text = "x".repeat(4096);
    assert_eq!(scan(&osc52_set(&text)), Some(text));
}

#[test]
fn terminates_on_st_as_well_as_bel() {
    let mut seq = b"\x1b]52;c;aGVsbG8=".to_vec();
    seq.extend_from_slice(b"\x1b\\");
    assert_eq!(scan(&seq), Some("hello".to_string()));
}

#[test]
fn ignores_unrelated_osc_sequences() {
    // OSC 0 (set window title) — a different OSC entirely.
    assert_eq!(scan(b"\x1b]0;some title\x07"), None);
}

#[test]
fn ignores_an_osc52_query() {
    assert_eq!(scan(b"\x1b]52;c;?\x07"), None);
}

#[test]
fn resumes_scanning_after_an_aborted_osc() {
    // ESC not followed by `\` mid-OSC cancels that capture without wedging
    // the scanner shut for the next one.
    let mut bytes = b"\x1b]52;c;partial\x1bX".to_vec();
    bytes.extend_from_slice(&osc52_set("second"));
    assert_eq!(scan(&bytes), Some("second".to_string()));
}

#[test]
fn recovers_after_exceeding_the_capture_budget() {
    // A pathological OSC that never terminates must not wedge the scanner:
    // once it is dropped for size, a later well-formed write still lands.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x1b]52;c;");
    bytes.extend(std::iter::repeat_n(b'A', 9 * 1024 * 1024));
    bytes.push(0x07);
    bytes.extend_from_slice(&osc52_set("after"));
    assert_eq!(scan(&bytes), Some("after".to_string()));
}
