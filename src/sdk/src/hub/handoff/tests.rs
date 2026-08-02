//! The payload contract, exercised without a socket.

use super::*;

/// A brief with the fields a caller always knows, ready to be clamped.
fn brief() -> HarnessHandoff {
    HarnessHandoff {
        id: "w_3-1".to_string(),
        at: 1_753_420_600_000,
        session_id: "w_3".to_string(),
        harness_session_id: Some("0f9c".to_string()),
        provider: "claude".to_string(),
        workspace_path: "/repos/acme".to_string(),
        branch: Some("feat/login".to_string()),
        project: Some("acme".to_string()),
        note: Some("  stuck on the failing e2e  ".to_string()),
        transcript: String::new(),
        transcript_truncated: false,
    }
}

fn lines(of: &[&str]) -> Vec<String> {
    of.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_brief_serializes_every_key_in_camel_case() {
    let value = serde_json::to_value(normalize(brief(), &lines(&["one", "two"]))).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "at",
            "branch",
            "harnessSessionId",
            "id",
            "note",
            "project",
            "provider",
            "sessionId",
            "transcript",
            "transcriptTruncated",
            "workspacePath",
        ]
    );
}

#[test]
fn absent_optionals_are_omitted_rather_than_null() {
    // A key present with a null is a key a reader has to handle. Omitting it
    // means "this was never known", which is the truth.
    let brief = HarnessHandoff {
        harness_session_id: None,
        branch: None,
        project: None,
        note: None,
        ..brief()
    };
    let value = serde_json::to_value(normalize(brief, &lines(&["x"]))).unwrap();

    for absent in ["harnessSessionId", "branch", "project", "note"] {
        assert!(value.get(absent).is_none(), "{absent} should be omitted");
    }
    assert_eq!(value["transcript"], "x");
}

#[test]
fn a_note_is_trimmed_and_a_blank_one_is_dropped() {
    let kept = normalize(brief(), &lines(&["x"]));
    assert_eq!(kept.note.as_deref(), Some("stuck on the failing e2e"));

    let blank = normalize(
        HarnessHandoff {
            note: Some("   \n  ".to_string()),
            ..brief()
        },
        &lines(&["x"]),
    );
    assert_eq!(blank.note, None, "whitespace is not a note");
}

#[test]
fn a_long_note_is_clamped() {
    let long = normalize(
        HarnessHandoff {
            note: Some("n".repeat(NOTE_MAX + 500)),
            ..brief()
        },
        &lines(&["x"]),
    );
    assert_eq!(long.note.unwrap().chars().count(), NOTE_MAX);
}

#[test]
fn the_transcript_keeps_the_last_lines_and_says_it_truncated() {
    // The end is what matters: it is where the work stopped.
    let many: Vec<String> = (0..TRANSCRIPT_LINES + 20)
        .map(|n| format!("line {n}"))
        .collect();
    let out = normalize(brief(), &many);

    let kept: Vec<&str> = out.transcript.lines().collect();
    assert_eq!(kept.len(), TRANSCRIPT_LINES);
    assert_eq!(kept[0], format!("line {}", 20));
    assert_eq!(
        *kept.last().unwrap(),
        format!("line {}", TRANSCRIPT_LINES + 19)
    );
    assert!(out.transcript_truncated);
}

#[test]
fn a_short_transcript_is_not_marked_truncated() {
    let out = normalize(brief(), &lines(&["one", "two"]));
    assert_eq!(out.transcript, "one\ntwo");
    assert!(!out.transcript_truncated);
}

#[test]
fn trailing_blank_lines_are_dropped() {
    // A harness pane is mostly empty space below the prompt; without this the
    // brief is a screenful of nothing.
    let out = normalize(brief(), &lines(&["real output", "", "   ", ""]));
    assert_eq!(out.transcript, "real output");
    assert!(!out.transcript_truncated, "blank padding is not content");
}

#[test]
fn a_very_wide_line_is_clamped_and_flagged() {
    let out = normalize(brief(), &["w".repeat(LINE_MAX + 100)]);
    assert_eq!(out.transcript.chars().count(), LINE_MAX);
    assert!(out.transcript_truncated);
}

#[test]
fn a_multibyte_line_is_never_split_into_invalid_utf8() {
    // Terminal output is full of box-drawing and emoji; truncating by bytes
    // would produce a string serde cannot emit.
    let out = normalize(brief(), &[("é".repeat(LINE_MAX + 50)).to_string()]);
    assert_eq!(out.transcript.chars().count(), LINE_MAX);
    assert!(serde_json::to_value(out).is_ok());
}

#[test]
fn the_joined_transcript_is_capped() {
    // Per-line and line-count limits still allow 80 wide lines; the advert this
    // rides on re-emits whenever a worker is added, so there is a hard ceiling.
    let wide: Vec<String> = (0..TRANSCRIPT_LINES)
        .map(|_| "w".repeat(LINE_MAX))
        .collect();
    let out = normalize(brief(), &wide);

    assert!(out.transcript.chars().count() <= TRANSCRIPT_MAX);
    assert!(out.transcript_truncated);
}

#[test]
fn control_uses_the_shared_spelling() {
    // The orchestrator reasons about operators, not users. One word per
    // concept, wherever it is read.
    assert_eq!(HandoffControl::Operator.as_str(), "operator");
    assert_eq!(HandoffControl::Orchestrator.as_str(), "orchestrator");
    assert_eq!(
        serde_json::to_value(HandoffControl::Operator).unwrap(),
        "operator"
    );
    assert!(HandoffControl::Operator.is_operator());
    assert!(!HandoffControl::default().is_operator());
}
