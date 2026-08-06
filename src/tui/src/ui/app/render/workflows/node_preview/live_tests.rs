//! Frame rendering, and the sanitizing it does on the way.

use super::{frame_line, live_lines};

/// Everything a rendered line puts on the screen, concatenated.
fn text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

/// An OSC 0 window-title rewrite and a CSI clear, the two payloads that make a
/// relayed harness frame worth sanitizing.
const ESCAPES: &str = "\u{1b}]0;pwned\u{7}\u{1b}[2J";

#[test]
fn a_tool_call_is_stripped_of_terminal_escapes() {
    let line = frame_line(&format!("running Read({ESCAPES}/etc/passwd)"));

    let rendered = text(&line);
    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(!rendered.contains('\u{7}'), "{rendered:?}");
    // The legible part survives: this sanitizes, it does not redact.
    assert!(rendered.contains("Read(]0;pwned[2J/etc/passwd)"), "{rendered:?}");
}

#[test]
fn a_tool_result_is_stripped_of_terminal_escapes() {
    for (frame, mark) in [
        (format!("tool completed · read 12 lines{ESCAPES}"), "✓"),
        (format!("tool failed · no such file{ESCAPES}"), "✗"),
    ] {
        let rendered = text(&frame_line(&frame));
        assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
        assert!(rendered.contains(mark), "{rendered:?}");
    }
}

#[test]
fn a_thinking_fragment_is_stripped_of_terminal_escapes() {
    let rendered = text(&frame_line(&format!("thinking · {ESCAPES}next I will")));

    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(rendered.contains("next I will"), "{rendered:?}");
}

#[test]
fn a_status_frame_is_stripped_of_terminal_escapes() {
    // Anything that is not a tool call or a thought falls through to `Status`,
    // which is the variant a harness's free-form chatter lands in.
    let rendered = text(&frame_line(&format!("{ESCAPES}compiling")));

    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(rendered.contains("compiling"), "{rendered:?}");
}

#[test]
fn no_frames_draws_no_header_at_all() {
    // An empty buffer means the step has said nothing yet; a bare "live" badge
    // over nothing is worse than the box simply not being there.
    assert!(live_lines(&[], true).is_empty());
}

#[test]
fn only_the_tail_is_drawn_and_the_rest_is_counted() {
    let frames: Vec<String> = (0..super::VISIBLE_FRAMES + 7)
        .map(|index| format!("frame {index}"))
        .collect();

    let lines = live_lines(&frames, true);
    let rendered: Vec<String> = lines.iter().map(text).collect();

    // Header, the "earlier frames" note, then exactly the visible tail.
    assert_eq!(rendered.len(), super::VISIBLE_FRAMES + 2);
    assert!(rendered[0].contains("live"));
    assert!(rendered[1].contains("… 7 earlier frames"), "{:?}", rendered[1]);
    assert!(rendered[2].contains("frame 7"), "{:?}", rendered[2]);
    assert!(
        rendered.last().is_some_and(|last| last
            .contains(&format!("frame {}", super::VISIBLE_FRAMES + 6))),
        "{:?}",
        rendered.last()
    );
}

#[test]
fn a_settled_run_is_labelled_last_rather_than_live() {
    let lines = live_lines(&["compiling".to_string()], false);

    let header = text(&lines[0]);
    assert!(header.contains("last"), "{header:?}");
    assert!(!header.contains("live"), "{header:?}");
    assert!(header.contains("when the run ended"), "{header:?}");
}
