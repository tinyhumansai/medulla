//! Rendering tests for folded workflow layout and animation.

use super::*;

#[test]
fn a_graph_wider_than_the_canvas_folds_into_view() {
    let mut long = diamond("long");
    long.graph = serde_json::from_value(json!({
        "nodes": (0..12).map(|index| json!({
            "id": format!("n{index}"),
            "kind": if index == 0 { "trigger" } else { "transform" },
            "name": format!("Step {index}"),
            "config": {},
        })).collect::<Vec<_>>(),
        "edges": (0..11).map(|index| json!({
            "from_node": format!("n{index}"), "to_node": format!("n{}", index + 1),
        })).collect::<Vec<_>>(),
    }))
    .expect("graph parses");
    let (_home, mut app) = app_with(&[long], &[]);

    // Twelve steps do not fit across the pane, so the chain wraps onto further
    // bands instead of running off the side. Nothing has to be scrolled to.
    let screen = render(&mut app);
    assert!(screen.contains("Step 0"));
    assert!(screen.contains("Step 11"), "the far end folded into view");
    assert_eq!(app.wf.canvas_row, 0, "and nothing had to scroll: {screen}");

    // Alternate bands run backwards, so the second band's first step is on the
    // right — directly under where the first band ran out.
    let per_band = app.layers_per_band();
    assert!(per_band < 12, "the chain has to fold at all");
    let last_of_first = app.graph_cell(per_band - 1, 0);
    let first_of_second = app.graph_cell(per_band, 0);
    assert_eq!(
        last_of_first.0, first_of_second.0,
        "the fold picks up in the column it left off in"
    );
    assert!(
        first_of_second.1 > last_of_first.1,
        "one band below the other"
    );
}

/// The foreground colours of the whole screen, cell by cell.
///
/// The flow highlight changes no characters — only which cell on a wire is lit —
/// so a text dump cannot see it at all.
fn colors(app: &mut App, width: u16, height: u16) -> Vec<Option<ratatui::style::Color>> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    buffer.content().iter().map(|cell| Some(cell.fg)).collect()
}

#[test]
fn the_wires_carry_a_highlight_that_moves_between_frames() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    // The animation clock is the app's frame counter, which the event loop
    // advances per tick; a test drives it directly so the frames it compares
    // are the ones the terminal would have drawn.
    let first_drawing = render(&mut app);
    let first = colors(&mut app, 160, 40);
    let highlight_moved_without_drift = (1..64).any(|frame| {
        app.frame = frame;
        render(&mut app) == first_drawing && colors(&mut app, 160, 40) != first
    });

    assert!(
        highlight_moved_without_drift,
        "wire colours move during a frame where node positions stay fixed"
    );
}

#[test]
fn the_graph_is_still_the_same_drawing_while_the_highlight_moves() {
    // Only colour may change with the frame. A wire whose glyph changed would be
    // the drawing itself flickering, which is a different thing entirely.
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    let first = render(&mut app);
    app.frame = app.frame.wrapping_add(7);
    let second = render(&mut app);

    assert_eq!(first, second);
}

#[test]
fn a_reversed_band_draws_its_wires_leftward() {
    let mut long = diamond("long");
    long.graph = serde_json::from_value(json!({
        "nodes": (0..12).map(|index| json!({
            "id": format!("n{index}"),
            "kind": if index == 0 { "trigger" } else { "transform" },
            "name": format!("Step {index}"),
            "config": {},
        })).collect::<Vec<_>>(),
        "edges": (0..11).map(|index| json!({
            "from_node": format!("n{index}"), "to_node": format!("n{}", index + 1),
        })).collect::<Vec<_>>(),
    }))
    .expect("graph parses");
    let (_home, mut app) = app_with(&[long], &[]);

    let screen = render(&mut app);

    assert!(screen.contains('▶'), "the first band points right");
    assert!(
        screen.contains('◀'),
        "the band under it runs the other way: {screen}"
    );
}

#[test]
fn the_nodes_sway_but_the_selected_one_holds_still() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    let selected = app
        .selected_graph_node()
        .expect("a node is selected")
        .name
        .clone();

    // A whole sway cycle, sampled often enough to catch the sway at both ends.
    let frames: Vec<String> = (0..64)
        .map(|step| {
            app.frame = step * 4;
            render(&mut app)
        })
        .collect();
    let column_of = |screen: &str, label: &str| {
        screen
            .lines()
            .find_map(|line| line.find(label))
            .expect("the label is on screen")
    };

    let moved = frames
        .iter()
        .any(|screen| column_of(screen, "◆ Check") != column_of(&frames[0], "◆ Check"));
    assert!(moved, "an unselected node sways");

    let anchor = column_of(&frames[0], &selected);
    for screen in &frames {
        assert_eq!(
            column_of(screen, &selected),
            anchor,
            "the selected node is the anchor and never moves"
        );
    }
}

/// A chain of `count` steps, each named with `name(index)`.
fn chain(id: &str, count: usize, name: impl Fn(usize) -> String) -> WorkflowRecord {
    let mut record = diamond(id);
    record.graph = serde_json::from_value(json!({
        "nodes": (0..count).map(|index| json!({
            "id": format!("n{index}"),
            "kind": if index == 0 { "trigger" } else { "transform" },
            "name": name(index),
            "config": {},
        })).collect::<Vec<_>>(),
        "edges": (0..count.saturating_sub(1)).map(|index| json!({
            "from_node": format!("n{index}"), "to_node": format!("n{}", index + 1),
        })).collect::<Vec<_>>(),
    }))
    .expect("graph parses");
    record
}

#[test]
fn each_column_is_sized_to_its_own_label() {
    // One wordy step among short ones. Only its own column widens: the rest
    // keep their own width, so the graph is not padded out to the longest name
    // in it and every connector stays short.
    let wordy = chain("wordy", 4, |index| {
        if index == 2 {
            "A conspicuously long step name".into()
        } else {
            format!("S{index}")
        }
    });
    let (_home, mut app) = app_with(&[wordy], &[]);
    render_sized(&mut app, 160, 34);

    assert_eq!(
        app.column_width(0),
        super::super::MIN_LABEL_WIDTH,
        "short name"
    );
    assert_eq!(
        app.column_width(2),
        super::super::MAX_NODE_WIDTH,
        "long name"
    );
    assert!(
        app.column_width(1) < app.column_width(2),
        "a short step keeps its own narrow column"
    );
}

#[test]
fn short_names_pack_more_steps_into_a_band() {
    let (_home, mut app) = app_with(&[chain("terse", 12, |index| format!("S{index}"))], &[]);
    render_sized(&mut app, 160, 34);
    let terse = app.layers_per_band();

    let (_home, mut app) = app_with(
        &[chain("wordy", 12, |index| {
            format!("A rather long step name {index}")
        })],
        &[],
    );
    render_sized(&mut app, 160, 34);
    let wordy = app.layers_per_band();

    assert!(
        terse > wordy,
        "narrow columns fit more per band: {terse} against {wordy}"
    );
}

#[test]
fn a_band_never_runs_past_the_pane() {
    let (_home, mut app) = app_with(&[chain("long", 12, |index| format!("Step {index}"))], &[]);

    for width in [100u16, 140, 200] {
        let screen = render_sized(&mut app, width, 34);
        let (per_band, canvas) = (app.layers_per_band(), app.canvas_width());

        for layer in 0..per_band {
            let (x, _) = app.graph_cell(layer, 0);
            let end = x + app.column_width(layer);
            assert!(
                end <= canvas + super::super::FOLD_MARGIN,
                "at {width} columns layer {layer} ends at {end} of {canvas}: {screen}"
            );
        }
        // Connectors stay short: the gap between one column and the next is the
        // gutter, never the padding of a column sized for another step's name.
        if per_band > 1 {
            let gap = app.graph_cell(1, 0).0 - (app.graph_cell(0, 0).0 + app.column_width(0));
            assert_eq!(
                gap,
                super::super::GUTTER_SPAN,
                "at {width} columns the gap is {gap}: {screen}"
            );
        }
    }
}

#[test]
fn a_fold_picks_up_where_the_band_above_ended() {
    let (_home, mut app) = app_with(&[chain("long", 12, |index| format!("Step {index}"))], &[]);
    render_sized(&mut app, 140, 34);
    let per_band = app.layers_per_band();
    assert!(per_band < 12, "the chain has to fold at all");

    // The last column of a band and the first of the one below it are the two
    // ends of a fold, and alternating the direction is what puts them in the
    // same place — a fold is a hop down, not a run back across the pane.
    let (last_x, last_row) = app.graph_cell(per_band - 1, 0);
    let last_end = last_x + app.column_width(per_band - 1);
    let (next_x, next_row) = app.graph_cell(per_band, 0);
    let next_end = next_x + app.column_width(per_band);

    assert!(next_row > last_row, "one band below the other");
    assert!(
        last_end.abs_diff(next_end) <= super::super::GUTTER_SPAN,
        "the fold's two ends are within a gutter of each other: {last_end} then {next_end}"
    );
}
