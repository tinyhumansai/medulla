//! Unit tests for button-event encoding: which modes take which motions, and
//! the literal bytes each wire format produces.
//!
//! Pinned to byte sequences for the same reason the key encoder is: a mistake
//! here is invisible until an operator is clicking inside a harness that either
//! ignores them or, worse, prints `ESC [ < 0 ; 4 ; 9 M` into its own composer.

use vt100::{MouseProtocolEncoding as Enc, MouseProtocolMode as Mode};

use super::super::mouse::{self, Button, Motion};

/// Every motion, for the exhaustiveness the mode table needs.
const MOTIONS: [Motion; 3] = [Motion::Press, Motion::Release, Motion::Drag];

#[test]
fn a_child_that_never_asked_for_the_mouse_gets_no_button_report() {
    // The important half of the feature, and the same rule the wheel follows:
    // a harness that never sent DECSET 1000/1002/1003 receives any report we
    // send as literal keystrokes.
    for motion in MOTIONS {
        assert!(
            mouse::button(Mode::None, Enc::Sgr, 0, 0, Button::Left, motion).is_none(),
            "{motion:?} must not be forwarded to a child that wants no mouse"
        );
        assert!(!mouse::reports(Mode::None, motion));
    }
}

#[test]
fn each_mode_takes_exactly_what_it_asked_for() {
    // The modes are cumulative, and overshooting is not harmless: an
    // application in press-only mode has no parser for a release.
    assert!(mouse::reports(Mode::Press, Motion::Press));
    assert!(!mouse::reports(Mode::Press, Motion::Release));
    assert!(!mouse::reports(Mode::Press, Motion::Drag));

    assert!(mouse::reports(Mode::PressRelease, Motion::Press));
    assert!(mouse::reports(Mode::PressRelease, Motion::Release));
    assert!(!mouse::reports(Mode::PressRelease, Motion::Drag));

    for motion in MOTIONS {
        assert!(mouse::reports(Mode::ButtonMotion, motion));
        assert!(mouse::reports(Mode::AnyMotion, motion));
    }
}

#[test]
fn sgr_names_the_button_and_distinguishes_press_from_release() {
    // Pane-relative (0, 0) is the child's own top-left, reported as 1;1.
    assert_eq!(
        mouse::button(
            Mode::PressRelease,
            Enc::Sgr,
            0,
            0,
            Button::Left,
            Motion::Press
        )
        .unwrap(),
        b"\x1b[<0;1;1M"
    );
    // The trailing letter is the *only* thing separating the two in SGR, and
    // the button code is repeated rather than replaced.
    assert_eq!(
        mouse::button(
            Mode::PressRelease,
            Enc::Sgr,
            11,
            4,
            Button::Left,
            Motion::Release
        )
        .unwrap(),
        b"\x1b[<0;12;5m"
    );
    assert_eq!(
        mouse::button(
            Mode::PressRelease,
            Enc::Sgr,
            0,
            0,
            Button::Middle,
            Motion::Press
        )
        .unwrap(),
        b"\x1b[<1;1;1M"
    );
    assert_eq!(
        mouse::button(
            Mode::PressRelease,
            Enc::Sgr,
            0,
            0,
            Button::Right,
            Motion::Press
        )
        .unwrap(),
        b"\x1b[<2;1;1M"
    );
}

#[test]
fn a_drag_sets_the_motion_bit_on_top_of_its_button() {
    // 32 is the motion bit; left-drag is 0 + 32, right-drag 2 + 32.
    assert_eq!(
        mouse::button(
            Mode::ButtonMotion,
            Enc::Sgr,
            2,
            3,
            Button::Left,
            Motion::Drag
        )
        .unwrap(),
        b"\x1b[<32;3;4M"
    );
    assert_eq!(
        mouse::button(
            Mode::ButtonMotion,
            Enc::Sgr,
            2,
            3,
            Button::Right,
            Motion::Drag
        )
        .unwrap(),
        b"\x1b[<34;3;4M"
    );
}

#[test]
fn the_normal_encoding_offsets_every_field_and_forgets_which_button_came_up() {
    let press = mouse::button(
        Mode::PressRelease,
        Enc::Default,
        0,
        0,
        Button::Left,
        Motion::Press,
    )
    .unwrap();
    assert_eq!(press, vec![0x1b, b'[', b'M', 32, 33, 33]);

    // The normal encoding has no room to name the released button, so every
    // release is code 3 (32 + 3 = 35) whichever button it was. An
    // xterm-compatible application pairs it with the press it is still holding.
    for button in [Button::Left, Button::Middle, Button::Right] {
        let release = mouse::button(
            Mode::PressRelease,
            Enc::Default,
            0,
            0,
            button,
            Motion::Release,
        )
        .unwrap();
        assert_eq!(release, vec![0x1b, b'[', b'M', 35, 33, 33], "{button:?}");
    }
}

#[test]
fn normal_coordinates_clamp_rather_than_wrap_past_their_ceiling() {
    // A pane can never be this wide, but the arithmetic reaching the clamp
    // must not overflow on the way there either.
    let bytes = mouse::button(
        Mode::PressRelease,
        Enc::Default,
        u16::MAX,
        u16::MAX,
        Button::Left,
        Motion::Press,
    )
    .unwrap();
    assert_eq!(bytes[4], 255, "column clamps to 223 + 32");
    assert_eq!(bytes[5], 255, "row clamps the same way");
}

#[test]
fn utf8_encoding_is_treated_as_the_normal_one() {
    // DECSET 1005 is a dead end whose multi-byte coordinates decode
    // ambiguously; xterm-compatible terminals fall back for the low
    // coordinates that fit in both, which is every coordinate a pane produces.
    assert_eq!(
        mouse::button(
            Mode::PressRelease,
            Enc::Utf8,
            3,
            1,
            Button::Left,
            Motion::Press
        )
        .unwrap(),
        mouse::button(
            Mode::PressRelease,
            Enc::Default,
            3,
            1,
            Button::Left,
            Motion::Press
        )
        .unwrap()
    );
}
