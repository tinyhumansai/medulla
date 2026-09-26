//! Tests for the shared footer layout: where the value sits, and what gives way
//! first when the rail is too narrow for the aligned form.

use super::gauge::{bar, bar_value, percent, Gauge, BAR_WIDTH, PERCENT_WIDTH};

/// Columns a bar and its percentage occupy together, gap included.
const BAR_VALUE_WIDTH: usize = BAR_WIDTH + 1 + PERCENT_WIDTH;

#[test]
fn the_value_is_flushed_to_the_right_edge() {
    let gauge = Gauge::new("Claude 5h", "  6%");
    assert_eq!(gauge.render(20), "Claude 5h         6%");
    assert_eq!(gauge.render(20).chars().count(), 20);
}

#[test]
fn a_percentage_is_the_same_width_at_every_reading() {
    for fraction in [0.0, 0.06, 0.5, 1.0] {
        assert_eq!(percent(fraction).chars().count(), PERCENT_WIDTH);
    }
    assert_eq!(percent(0.06), "  6%");
    assert_eq!(percent(1.0), "100%");
    // Out-of-range readings are clamped rather than widening the column.
    assert_eq!(percent(1.5), "100%");
}

#[test]
fn a_bar_is_the_same_width_at_every_reading() {
    for fraction in [0.0, 0.06, 0.5, 1.0] {
        assert_eq!(bar(fraction).chars().count(), BAR_WIDTH);
        assert_eq!(bar_value(fraction).chars().count(), BAR_VALUE_WIDTH);
    }
    assert_eq!(bar(0.0), "░░░░");
    assert_eq!(bar(1.0), "████");
}

#[test]
fn two_readings_of_different_widths_line_up_on_the_right() {
    let lines = [
        Gauge::new("Device CPU", bar_value(0.42)).render(26),
        Gauge::new("Claude week", percent(0.4)).render(26),
        Gauge::new("OpenRouter", "$37.5").render(26),
    ];
    for line in &lines {
        assert_eq!(line.chars().count(), 26, "{line}");
        assert!(!line.ends_with(' '), "{line}");
    }
}

#[test]
fn the_natural_width_is_what_the_rail_should_be_sized_to() {
    let gauge = Gauge::new("Device CPU", bar_value(0.42));
    // Label, one column of gap, and the fixed-width bar and percentage.
    assert_eq!(gauge.natural_width(), 10 + 1 + BAR_VALUE_WIDTH);
    // Rendered at exactly that width, the gap is the single column it promised.
    assert_eq!(
        gauge.render(gauge.natural_width()),
        format!("Device CPU {}", bar_value(0.42))
    );
}

#[test]
fn a_narrow_rail_takes_the_padding_before_it_takes_the_digits() {
    let gauge = Gauge::new("Claude 5h", percent(0.06));
    // One column short of the aligned form: the percentage's own padding goes,
    // and the reading survives.
    assert_eq!(gauge.render(12), "Claude 5h 6%");
    // Narrower still, and the line overhangs rather than losing a digit — the
    // caller is expected to have degraded to a shorter form before this.
    assert_eq!(gauge.render(4), "Claude 5h 6%");
}
