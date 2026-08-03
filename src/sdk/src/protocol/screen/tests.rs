//! Unit tests for the screen protocol: coalescing, diffing, the viewer's fold,
//! and the version-tagged envelope.
//!
//! The property that matters most is the last group's: a viewer that applies
//! every frame a sender emits ends up holding exactly the sender's screen. The
//! diff is only worth having if that holds across resizes and cursor-only
//! moves, so it is asserted as a sequence rather than frame by frame.

use super::*;

/// A style with just an attribute set, for making rows visibly differ.
fn styled(attrs: u8) -> RunStyle {
    RunStyle {
        attrs,
        ..RunStyle::default()
    }
}

/// A grid of `lines`, sized to fit them, cursor at the origin.
fn grid(cols: u16, lines: Vec<Vec<ScreenRun>>) -> ScreenGrid {
    ScreenGrid {
        cols,
        rows: lines.len() as u16,
        lines,
        cursor: (0, 0),
        hide_cursor: false,
    }
}

/// One row of one unstyled run.
fn row(text: &str) -> Vec<ScreenRun> {
    vec![ScreenRun::plain(text)]
}

fn cells(text: &str, style: RunStyle) -> Vec<(String, RunStyle)> {
    text.chars().map(|c| (c.to_string(), style)).collect()
}

// --- coalescing ------------------------------------------------------------

#[test]
fn adjacent_cells_of_one_style_become_a_single_run() {
    let runs = coalesce_runs(cells("hello", RunStyle::default()));
    assert_eq!(runs, vec![ScreenRun::plain("hello")]);
}

#[test]
fn a_style_change_splits_the_run() {
    let mut input = cells("ab", RunStyle::default());
    input.extend(cells("c", styled(ATTR_BOLD)));
    let runs = coalesce_runs(input);
    assert_eq!(
        runs,
        vec![
            ScreenRun::plain("ab"),
            ScreenRun::new("c", styled(ATTR_BOLD)),
        ]
    );
}

#[test]
fn unstyled_trailing_blanks_are_dropped() {
    // Most of a terminal row is trailing blanks; carrying them would dominate
    // every frame.
    let runs = coalesce_runs(cells("hi        ", RunStyle::default()));
    assert_eq!(runs, vec![ScreenRun::plain("hi")]);
}

#[test]
fn an_entirely_blank_row_coalesces_to_nothing() {
    assert!(coalesce_runs(cells("      ", RunStyle::default())).is_empty());
}

#[test]
fn styled_trailing_blanks_survive() {
    // A harness status bar is a run of styled spaces. Trimming it erases the bar.
    let runs = coalesce_runs(cells("    ", styled(ATTR_INVERSE)));
    assert_eq!(runs, vec![ScreenRun::new("    ", styled(ATTR_INVERSE))]);
}

// --- diffing ---------------------------------------------------------------

#[test]
fn the_first_frame_of_a_stream_is_full() {
    let next = grid(10, vec![row("a"), row("b")]);
    let FrameDecision::Send(frame) = build_frame(None, &next, "w_1", 1, 0) else {
        panic!("expected a frame");
    };
    assert!(frame.full);
    assert_eq!(frame.rows_changed.len(), 2);
}

#[test]
fn only_changed_rows_are_sent() {
    let previous = grid(10, vec![row("a"), row("b"), row("c")]);
    let next = grid(10, vec![row("a"), row("CHANGED"), row("c")]);
    let FrameDecision::Send(frame) = build_frame(Some(&previous), &next, "w_1", 2, 1) else {
        panic!("expected a frame");
    };
    assert!(!frame.full);
    assert_eq!(frame.base_seq, 1);
    assert_eq!(frame.rows_changed.len(), 1);
    assert_eq!(frame.rows_changed[0].y, 1);
    assert_eq!(frame.rows_changed[0].runs, row("CHANGED"));
}

#[test]
fn an_unchanged_screen_sends_nothing() {
    // The property that decouples wire cost from harness repainting.
    let previous = grid(10, vec![row("a"), row("b")]);
    let next = previous.clone();
    assert_eq!(
        build_frame(Some(&previous), &next, "w_1", 2, 1),
        FrameDecision::Unchanged
    );
}

#[test]
fn a_cursor_move_alone_is_still_worth_a_frame() {
    let previous = grid(10, vec![row("a")]);
    let mut next = previous.clone();
    next.cursor = (0, 4);
    let FrameDecision::Send(frame) = build_frame(Some(&previous), &next, "w_1", 2, 1) else {
        panic!("a cursor move is a visible change");
    };
    assert!(frame.rows_changed.is_empty());
    assert_eq!(frame.cursor, (0, 4));
}

#[test]
fn a_resize_forces_a_full_frame() {
    // Row indices do not survive a resize: a delta across one would address rows
    // that no longer mean the same thing.
    let previous = grid(10, vec![row("a"), row("b")]);
    let next = grid(20, vec![row("a"), row("b")]);
    let FrameDecision::Send(frame) = build_frame(Some(&previous), &next, "w_1", 2, 1) else {
        panic!("expected a frame");
    };
    assert!(frame.full, "a resize must not be sent as a delta");
    assert_eq!(frame.cols, 20);
}

#[test]
fn changed_rows_refuses_grids_of_different_sizes() {
    let a = grid(10, vec![row("x")]);
    let b = grid(10, vec![row("x"), row("y")]);
    assert!(changed_rows(&a, &b).is_none());
}

// --- applying --------------------------------------------------------------

#[test]
fn a_full_frame_applies_to_an_empty_view() {
    let next = grid(10, vec![row("a"), row("b")]);
    let FrameDecision::Send(frame) = build_frame(None, &next, "w_1", 7, 0) else {
        panic!("expected a frame");
    };
    let mut view = None;
    assert_eq!(apply_frame(&mut view, &frame), ApplyOutcome::Applied);
    let view = view.expect("a full frame always establishes a view");
    assert_eq!(view.seq, 7);
    assert_eq!(view.grid, next);
}

#[test]
fn a_delta_against_no_view_asks_to_resync() {
    let previous = grid(10, vec![row("a")]);
    let next = grid(10, vec![row("b")]);
    let FrameDecision::Send(frame) = build_frame(Some(&previous), &next, "w_1", 2, 1) else {
        panic!("expected a frame");
    };
    let mut view = None;
    assert_eq!(apply_frame(&mut view, &frame), ApplyOutcome::NeedsResync);
    assert!(view.is_none(), "a rejected frame must not half-apply");
}

#[test]
fn a_sequence_gap_asks_to_resync_without_touching_the_view() {
    let base = grid(10, vec![row("a")]);
    let FrameDecision::Send(full) = build_frame(None, &base, "w_1", 1, 0) else {
        panic!("expected a frame");
    };
    let mut view = None;
    apply_frame(&mut view, &full);

    // A delta claiming to follow seq 5 when the view holds seq 1.
    let next = grid(10, vec![row("b")]);
    let FrameDecision::Send(mut delta) = build_frame(Some(&base), &next, "w_1", 6, 5) else {
        panic!("expected a frame");
    };
    delta.base_seq = 5;
    assert_eq!(apply_frame(&mut view, &delta), ApplyOutcome::NeedsResync);
    assert_eq!(
        view.expect("view survives").grid,
        base,
        "the stale view must be left intact, not partly patched"
    );
}

#[test]
fn a_row_outside_the_grid_asks_to_resync_rather_than_panicking() {
    // Inbound frames are untrusted; a bad row index must not index out of bounds.
    let base = grid(10, vec![row("a")]);
    let FrameDecision::Send(full) = build_frame(None, &base, "w_1", 1, 0) else {
        panic!("expected a frame");
    };
    let mut view = None;
    apply_frame(&mut view, &full);

    let hostile = ScreenFrame {
        task_id: "w_1".into(),
        seq: 2,
        base_seq: 1,
        full: false,
        cols: 10,
        rows: 1,
        cursor: (0, 0),
        hide_cursor: false,
        rows_changed: vec![RowUpdate {
            y: 99,
            runs: row("boom"),
        }],
    };
    assert_eq!(apply_frame(&mut view, &hostile), ApplyOutcome::NeedsResync);
    assert_eq!(view.expect("view survives").grid, base);
}

// --- the round trip --------------------------------------------------------

#[test]
fn a_viewer_applying_every_frame_ends_up_holding_the_senders_screen() {
    // The whole point of the protocol, asserted across the cases that are easy
    // to get wrong: a plain edit, a cursor-only move, and a resize.
    let states = vec![
        grid(10, vec![row("one"), row("two"), row("three")]),
        grid(10, vec![row("one"), row("TWO!"), row("three")]),
        {
            let mut g = grid(10, vec![row("one"), row("TWO!"), row("three")]);
            g.cursor = (2, 5);
            g
        },
        grid(24, vec![row("wider"), row("now"), row("reflowed")]),
        {
            let mut g = grid(24, vec![row("wider"), row("now"), row("reflowed")]);
            g.hide_cursor = true;
            g
        },
    ];

    let mut view: Option<ScreenView> = None;
    let mut sent: Option<ScreenGrid> = None;
    let mut seq = 0i64;

    for state in &states {
        let base_seq = seq;
        seq += 1;
        match build_frame(sent.as_ref(), state, "w_1", seq, base_seq) {
            FrameDecision::Unchanged => {
                seq -= 1; // nothing went out, so the sequence did not advance
            }
            FrameDecision::Send(frame) => {
                assert_eq!(
                    apply_frame(&mut view, &frame),
                    ApplyOutcome::Applied,
                    "every frame the sender emits must apply in order"
                );
                sent = Some(state.clone());
            }
        }
        assert_eq!(
            &view.as_ref().expect("a view by now").grid,
            state,
            "viewer diverged from the sender"
        );
    }
}

#[test]
fn a_resync_recovers_a_viewer_that_missed_a_frame() {
    let first = grid(10, vec![row("a"), row("b")]);
    let second = grid(10, vec![row("a"), row("CHANGED")]);
    let third = grid(10, vec![row("a"), row("AGAIN")]);

    let mut view = None;
    let FrameDecision::Send(f1) = build_frame(None, &first, "w_1", 1, 0) else {
        panic!("expected a frame");
    };
    apply_frame(&mut view, &f1);

    // The frame carrying `second` is lost in the mail, so the viewer never
    // reaches seq 2 — and the next delta is unusable.
    let FrameDecision::Send(f3) = build_frame(Some(&second), &third, "w_1", 3, 2) else {
        panic!("expected a frame");
    };
    assert_eq!(apply_frame(&mut view, &f3), ApplyOutcome::NeedsResync);

    // The viewer asks to resync; the sender answers with a full frame and the
    // gap is closed without anything being retransmitted.
    let FrameDecision::Send(resync) = build_frame(None, &third, "w_1", 4, 0) else {
        panic!("expected a frame");
    };
    assert_eq!(apply_frame(&mut view, &resync), ApplyOutcome::Applied);
    assert_eq!(view.expect("recovered").grid, third);
}

// --- the envelope ----------------------------------------------------------

#[test]
fn messages_round_trip_through_the_envelope() {
    let cases = vec![
        ScreenMessage::Subscribe {
            task_id: "w_1".into(),
            max_fps: 1,
            resync: true,
        },
        ScreenMessage::Unsubscribe {
            task_id: "w_1".into(),
        },
        ScreenMessage::Kill {
            task_id: "w_1".into(),
            correlation_id: "cyc/w_1/0".into(),
        },
        ScreenMessage::Ack {
            task_id: "w_1".into(),
            seq: 418,
        },
    ];
    for message in cases {
        let body = encode_screen_message(&message);
        assert_eq!(parse_screen_message(&body), Some(message));
    }
}

#[test]
fn a_frame_round_trips_with_its_styles_intact() {
    let mut source = grid(
        10,
        vec![vec![
            ScreenRun::new("run", styled(ATTR_BOLD | ATTR_UNDERLINE)),
            ScreenRun::new(
                "ning",
                RunStyle {
                    fg: Color::Idx(4),
                    bg: Color::Rgb(1, 2, 3),
                    attrs: ATTR_INVERSE,
                },
            ),
        ]],
    );
    source.cursor = (0, 7);
    let FrameDecision::Send(frame) = build_frame(None, &source, "w_1", 1, 0) else {
        panic!("expected a frame");
    };
    let body = encode_screen_message(&ScreenMessage::Frame(frame));
    let Some(ScreenMessage::Frame(decoded)) = parse_screen_message(&body) else {
        panic!("a frame must survive the envelope");
    };
    let mut view = None;
    apply_frame(&mut view, &decoded);
    assert_eq!(view.expect("applied").grid, source);
}

#[test]
fn the_encoded_body_carries_the_version_tag() {
    let body = encode_screen_message(&ScreenMessage::Unsubscribe {
        task_id: "w_1".into(),
    });
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["screen_version"], SCREEN_PROTO);
    assert_eq!(value["kind"], "unsubscribe");
}

#[test]
fn parse_rejects_bodies_that_are_not_ours() {
    // This channel carries four protocols; a body that is not ours must fall
    // through to the next parser rather than being mangled into one.
    assert!(parse_screen_message("plain text dm").is_none());
    assert!(parse_screen_message("  not { json").is_none());
    assert!(parse_screen_message(r#"{"proto":"medulla-task/1","kind":"task"}"#).is_none());
    assert!(parse_screen_message(
        r#"{"control_version":"tinyplace.harness.control.v1","kind":"input","text":"x"}"#
    )
    .is_none());
    // Right tag, unknown kind.
    assert!(parse_screen_message(&format!(
        r#"{{"screen_version":"{SCREEN_PROTO}","kind":"teleport"}}"#
    ))
    .is_none());
    // Right tag, missing required fields.
    assert!(parse_screen_message(&format!(
        r#"{{"screen_version":"{SCREEN_PROTO}","kind":"ack"}}"#
    ))
    .is_none());
}

#[test]
fn default_colours_and_attributes_stay_off_the_wire() {
    // Most of a screen is unstyled; serializing those fields would dominate a
    // frame that is supposed to be a few hundred bytes.
    let body = encode_screen_message(&ScreenMessage::Frame(ScreenFrame {
        task_id: "w_1".into(),
        seq: 1,
        base_seq: 0,
        full: true,
        cols: 4,
        rows: 1,
        cursor: (0, 0),
        hide_cursor: false,
        rows_changed: vec![RowUpdate {
            y: 0,
            runs: row("bare"),
        }],
    }));
    assert!(!body.contains("\"fg\""), "got: {body}");
    assert!(!body.contains("\"bg\""), "got: {body}");
    assert!(!body.contains("\"attrs\""), "got: {body}");
    assert!(body.contains("\"t\":\"bare\""), "got: {body}");
}
