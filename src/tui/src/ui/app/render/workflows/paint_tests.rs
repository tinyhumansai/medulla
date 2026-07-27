//! Tests for the graph canvas grid.

use super::*;

/// The canvas serialised to plain strings, one per row.
fn rows(canvas: Canvas) -> Vec<String> {
    canvas
        .into_lines()
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn an_empty_canvas_is_blank_rows_of_the_asked_for_size() {
    let out = rows(Canvas::new(4, 2));

    assert_eq!(out, vec!["    ", "    "]);
}

#[test]
fn text_is_written_where_it_is_placed() {
    let mut canvas = Canvas::new(6, 1);

    canvas.text(1, 0, "abc", CellStyle::default());

    assert_eq!(rows(canvas), vec![" abc  "]);
}

#[test]
fn text_running_off_the_edge_is_clipped_rather_than_wrapping() {
    let mut canvas = Canvas::new(4, 2);

    canvas.text(2, 0, "abcdef", CellStyle::default());

    // Wrapping would put the tail inside the next row, which on a real canvas
    // is the next column's node.
    assert_eq!(rows(canvas), vec!["  ab", "    "]);
}

#[test]
fn a_horizontal_run_is_drawn_as_a_line() {
    let mut canvas = Canvas::new(5, 1);

    canvas.horizontal(0, 4, 0, CellStyle::default());

    assert_eq!(rows(canvas), vec!["─────"]);
}

#[test]
fn a_run_drawn_right_to_left_is_the_same_line() {
    let mut canvas = Canvas::new(5, 1);

    canvas.horizontal(4, 0, 0, CellStyle::default());

    assert_eq!(rows(canvas), vec!["─────"]);
}

#[test]
fn a_vertical_run_is_drawn_as_a_line() {
    let mut canvas = Canvas::new(1, 3);

    canvas.vertical(0, 0, 2, CellStyle::default());

    assert_eq!(rows(canvas), vec!["│", "│", "│"]);
}

#[test]
fn a_run_that_turns_gets_a_corner_where_it_meets() {
    let mut canvas = Canvas::new(3, 3);

    // Right along the top, then down the last column.
    canvas.horizontal(0, 2, 0, CellStyle::default());
    canvas.vertical(2, 0, 2, CellStyle::default());

    assert_eq!(rows(canvas), vec!["──╮", "  │", "  │"]);
}

#[test]
fn every_corner_orientation_gets_its_own_character() {
    let cases = [
        (DOWN | RIGHT, '╭'),
        (DOWN | LEFT, '╮'),
        (UP | RIGHT, '╰'),
        (UP | LEFT, '╯'),
    ];

    for (sides, expected) in cases {
        assert_eq!(wire_char(sides), expected, "{sides:b}");
    }
}

#[test]
fn two_wires_crossing_merge_into_one_junction() {
    let mut canvas = Canvas::new(3, 3);

    canvas.horizontal(0, 2, 1, CellStyle::default());
    canvas.vertical(1, 0, 2, CellStyle::default());

    assert_eq!(rows(canvas)[1], "─┼─");
}

#[test]
fn a_wire_branching_off_another_becomes_a_tee() {
    let mut canvas = Canvas::new(3, 2);

    canvas.horizontal(0, 2, 0, CellStyle::default());
    canvas.vertical(1, 0, 1, CellStyle::default());

    assert_eq!(rows(canvas)[0], "─┬─");
}

#[test]
fn a_wire_never_paints_over_a_node() {
    let mut canvas = Canvas::new(5, 1);
    canvas.text(1, 0, "box", CellStyle::default());

    canvas.horizontal(0, 4, 0, CellStyle::default());

    assert_eq!(
        rows(canvas),
        vec!["─box─"],
        "a long edge tunnels behind a box rather than through its label"
    );
}

#[test]
fn an_arrowhead_replaces_the_wire_it_lands_on() {
    let mut canvas = Canvas::new(3, 1);
    canvas.horizontal(0, 2, 0, CellStyle::default());

    canvas.arrow(2, 0, '▶', CellStyle::default());

    assert_eq!(rows(canvas), vec!["──▶"]);
}

#[test]
fn an_arrowhead_does_not_land_on_a_node() {
    let mut canvas = Canvas::new(2, 1);
    canvas.text(1, 0, "x", CellStyle::default());

    canvas.arrow(1, 0, '▶', CellStyle::default());

    assert_eq!(rows(canvas), vec![" x"]);
}

#[test]
fn painting_off_the_canvas_is_ignored_rather_than_panicking() {
    let mut canvas = Canvas::new(2, 2);

    canvas.horizontal(0, 99, 0, CellStyle::default());
    canvas.vertical(0, 0, 99, CellStyle::default());
    canvas.text(99, 99, "x", CellStyle::default());
    canvas.arrow(99, 99, '▶', CellStyle::default());

    assert_eq!(rows(canvas).len(), 2);
}

#[test]
fn a_row_of_one_style_serialises_as_one_span() {
    let mut canvas = Canvas::new(4, 1);
    canvas.text(0, 0, "abcd", CellStyle::colored(Color::Green));

    let lines = canvas.into_lines();

    assert_eq!(lines[0].spans.len(), 1);
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
}

#[test]
fn a_row_of_mixed_styles_breaks_into_a_span_per_run() {
    let mut canvas = Canvas::new(4, 1);
    canvas.text(0, 0, "ab", CellStyle::colored(Color::Green));
    canvas.text(2, 0, "cd", CellStyle::colored(Color::Red));

    let lines = canvas.into_lines();

    assert_eq!(lines[0].spans.len(), 2);
    assert_eq!(lines[0].spans[1].style.fg, Some(Color::Red));
}

#[test]
fn a_crossing_keeps_the_colour_of_the_wire_that_got_there_first() {
    let mut canvas = Canvas::new(3, 3);

    canvas.horizontal(0, 2, 1, CellStyle::colored(Color::Green));
    canvas.vertical(1, 0, 2, CellStyle::colored(Color::Red));

    let lines = canvas.into_lines();
    let junction = lines[1]
        .spans
        .iter()
        .find(|span| span.content.contains('┼'))
        .expect("the junction is drawn");
    assert_eq!(junction.style.fg, Some(Color::Green));
}

#[test]
fn a_selected_cell_renders_reversed() {
    let mut canvas = Canvas::new(2, 1);
    canvas.text(0, 0, "ab", CellStyle::default().selected(true));

    let lines = canvas.into_lines();

    assert!(lines[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::REVERSED));
}

#[test]
fn a_dimmed_cell_renders_dim() {
    let mut canvas = Canvas::new(2, 1);
    canvas.text(0, 0, "ab", CellStyle::colored(Color::Green).dimmed());

    let lines = canvas.into_lines();

    assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::DIM));
}
