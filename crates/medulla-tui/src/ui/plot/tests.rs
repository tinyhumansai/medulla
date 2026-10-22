use super::{Gradient, Graph, Meter, Symbol};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthChar;

fn render_graph(values: &[f32], width: u16, height: u16, symbol: Symbol, invert: bool) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    Graph::new(values)
        .symbol(symbol)
        .invert(invert)
        .render(area, &mut buffer);
    buffer
}

fn glyph(buffer: &Buffer, x: u16, y: u16) -> char {
    buffer[(x, y)].symbol().chars().next().unwrap_or(' ')
}

#[test]
fn paired_samples_match_btop_glyphs_for_each_symbol_table() {
    let expected = [
        (Symbol::Braille, '⣰', '⠹'),
        (Symbol::Block, '▟', '▜'),
        (Symbol::Tty, '▒', '▒'),
    ];
    for (symbol, up, down) in expected {
        let graph = render_graph(&[25.0, 75.0], 1, 1, symbol, false);
        let inverted = render_graph(&[25.0, 75.0], 1, 1, symbol, true);
        assert_eq!(glyph(&graph, 0, 0), up);
        assert_eq!(glyph(&inverted, 0, 0), down);
    }
}

#[test]
fn short_histories_right_align_and_odd_histories_start_with_an_empty_half() {
    let short = render_graph(&[25.0, 75.0], 3, 1, Symbol::Braille, false);
    assert_eq!(glyph(&short, 0, 0), ' ');
    assert_eq!(glyph(&short, 1, 0), ' ');
    assert_eq!(glyph(&short, 2, 0), '⣰');

    let odd = render_graph(&[25.0, 75.0, 50.0], 2, 1, Symbol::Braille, false);
    assert_eq!(glyph(&odd, 0, 0), '⢀');
    assert_eq!(glyph(&odd, 1, 0), Symbol::Braille.glyph(false, 3, 2));
}

#[test]
fn short_and_empty_histories_clear_stale_buffer_cells() {
    let area = Rect::new(0, 0, 3, 1);
    let mut buffer = Buffer::empty(area);
    for x in 0..area.width {
        buffer[(x, 0)].set_char('x');
    }

    Graph::new(&[25.0, 75.0]).render(area, &mut buffer);
    assert_eq!(glyph(&buffer, 0, 0), ' ');
    assert_eq!(glyph(&buffer, 1, 0), ' ');
    assert_eq!(glyph(&buffer, 2, 0), '⣰');

    Graph::new(&[]).render(area, &mut buffer);
    assert!(buffer.content().iter().all(|cell| cell.symbol() == " "));
}

#[test]
fn percentage_rows_fill_from_the_bottom_and_invert_to_the_top() {
    let up = render_graph(&[0.0, 37.5], 1, 4, Symbol::Braille, false);
    let down = render_graph(&[0.0, 37.5], 1, 4, Symbol::Braille, true);

    assert_eq!(glyph(&up, 0, 0), ' ');
    assert_eq!(glyph(&up, 0, 1), ' ');
    assert_eq!(glyph(&up, 0, 2), '⢠');
    assert_eq!(glyph(&up, 0, 3), '⢸');
    assert_eq!(glyph(&down, 0, 2), '⠘');
    assert_eq!(glyph(&down, 0, 3), '⢸');
    // The lower dot in the up table becomes an upper dot with the down table.
    assert_ne!(
        Symbol::Braille.glyph(false, 1, 0),
        Symbol::Braille.glyph(true, 1, 0)
    );
}

#[test]
fn no_zero_keeps_a_baseline_mark_for_zero_values() {
    let area = Rect::new(0, 0, 1, 4);
    let mut graph = Buffer::empty(area);
    Graph::new(&[0.0]).no_zero(true).render(area, &mut graph);
    assert_eq!(glyph(&graph, 0, 0), ' ');
    assert_eq!(glyph(&graph, 0, 1), ' ');
    assert_eq!(glyph(&graph, 0, 2), ' ');
    assert_eq!(glyph(&graph, 0, 3), '⢀');
}

#[test]
fn meter_fills_zero_half_and_all_of_ten_cells() {
    let gradient = Gradient::new(vec![Color::Rgb(1, 2, 3)]);
    for (value, filled) in [(0.0, 0), (50.0, 5), (100.0, 10)] {
        let area = Rect::new(0, 0, 10, 1);
        let mut buffer = Buffer::empty(area);
        Meter::new(value, &gradient, Color::Rgb(4, 5, 6)).render(area, &mut buffer);
        let actual = (0..10)
            .filter(|x| buffer[(*x, 0)].style().fg == Some(Color::Rgb(1, 2, 3)))
            .count();
        assert_eq!(actual, filled);
        assert!(buffer.content().iter().all(|cell| cell.symbol() == "■"));
    }
}

#[test]
fn braille_glyphs_are_single_terminal_cells() {
    assert_eq!(UnicodeWidthChar::width('⣿'), Some(1));
    let graph = render_graph(&[0.0, 100.0], 1, 1, Symbol::Braille, false);
    assert_eq!(glyph(&graph, 0, 0), '⢸');
}

#[test]
fn gradient_interpolates_rgb_stops() {
    let gradient = Gradient::new(vec![Color::Rgb(0, 10, 20), Color::Rgb(100, 110, 120)]);
    assert_eq!(gradient.at(50.0), Color::Rgb(50, 60, 70));
}
