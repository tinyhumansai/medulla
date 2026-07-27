//! Unit tests for the compact meters: bar fill, pressure colouring, the
//! omit-rather-than-zero rule, and the lane usage accumulation.

use super::meters::*;
use crate::runtime::HostResources;
use crate::ui::events::Usage;

#[test]
fn the_bar_fills_proportionally_and_clamps_at_both_ends() {
    assert_eq!(bar(0.0), "░░░░░░░░");
    assert_eq!(bar(1.0), "████████");
    assert_eq!(
        bar(-3.0),
        "░░░░░░░░",
        "a negative fraction cannot underflow"
    );
    assert_eq!(
        bar(4.0),
        "████████",
        "an over-full fraction cannot overflow"
    );
    assert_eq!(bar(0.5).chars().count(), 8, "width is fixed");
    // A small non-zero reading must still draw something: rounding it away
    // would render "nearly out" identically to "empty".
    assert_ne!(bar(0.02), bar(0.0));
}

#[test]
fn colour_tracks_pressure_not_kind() {
    assert_eq!(pressure_color(0.1), "green");
    assert_eq!(pressure_color(0.7), "yellow");
    assert_eq!(pressure_color(0.95), "red");
}

#[test]
fn memory_reads_as_used_so_a_full_machine_reads_hot() {
    let resources = HostResources {
        total_memory_bytes: Some(32 << 30),
        available_memory_bytes: Some(4 << 30),
        ..Default::default()
    };
    let line = memory_meter(&resources).expect("both numbers known");
    assert!(line.text.contains("28.0GB / 32.0GB"), "{}", line.text);
    assert_eq!(line.color.as_deref(), Some("yellow"), "87% used is warm");

    // Red is reserved for "about to stop the work".
    let nearly_full = HostResources {
        total_memory_bytes: Some(32 << 30),
        available_memory_bytes: Some(1 << 30),
        ..Default::default()
    };
    let line = memory_meter(&nearly_full).expect("both numbers known");
    assert_eq!(line.color.as_deref(), Some("red"));
}

#[test]
fn an_unreported_reading_is_omitted_rather_than_drawn_as_zero() {
    // Nothing declared: no meters at all.
    let empty = HostResources::default();
    assert!(memory_meter(&empty).is_none());
    assert!(cpu_meter(&empty).is_none());
    assert!(disk_line(&empty).is_none());

    // A total with no current reading cannot be divided, so it is not a bar.
    let partial = HostResources {
        total_memory_bytes: Some(16 << 30),
        ..Default::default()
    };
    assert!(memory_meter(&partial).is_none());
}

#[test]
fn cpu_states_capacity_when_no_load_was_reported() {
    let cores_only = HostResources {
        cpu_cores: Some(8.0),
        ..Default::default()
    };
    let line = cpu_meter(&cores_only).expect("cores are worth stating");
    assert_eq!(line.text, "cpu · 8 cores");
    assert!(line.color.is_none(), "a capacity figure has no pressure");

    let loaded = HostResources {
        cpu_cores: Some(8.0),
        load_average_1m: Some(7.6),
        ..Default::default()
    };
    let line = cpu_meter(&loaded).expect("load over cores is a fraction");
    assert!(line.text.contains("7.60 load / 8 cores"), "{}", line.text);
    assert_eq!(line.color.as_deref(), Some("red"));
}

#[test]
fn lane_usage_keeps_the_current_prompt_and_sums_the_output() {
    let mut usage = LaneUsage::default();
    usage.accumulate(&Usage {
        input_tokens: 1_000,
        output_tokens: 100,
        ..Default::default()
    });
    usage.accumulate(&Usage {
        input_tokens: 1_800,
        output_tokens: 250,
        ..Default::default()
    });
    // The prompt is resent every turn, so summing it would count the same
    // tokens repeatedly; the output genuinely accumulates.
    assert_eq!(usage.input, 1_800);
    assert_eq!(usage.output, 350);
    assert_eq!(usage.cache_hit_rate(), None, "nothing reported a cache");
}

#[test]
fn the_context_meter_reports_in_out_and_cache_when_known() {
    let mut usage = LaneUsage::default();
    usage.accumulate(&Usage {
        input_tokens: 24_000,
        output_tokens: 1_500,
        cache_read_tokens: Some(18_000),
        cache_creation_tokens: None,
    });
    let line = context_meter(&usage, 32_000).expect("a used lane has a meter");
    assert!(line.text.contains("in 24k / 32k"), "{}", line.text);
    assert!(line.text.contains("out 2k"), "{}", line.text);
    assert!(line.text.contains("cache 75%"), "{}", line.text);
    assert_eq!(line.color.as_deref(), Some("yellow"), "75% of the window");

    // An untouched lane has nothing to meter.
    assert!(context_meter(&LaneUsage::default(), 32_000).is_none());
}

#[test]
fn a_cache_figure_is_never_inferred_from_silence() {
    let mut usage = LaneUsage::default();
    usage.accumulate(&Usage {
        input_tokens: 10_000,
        output_tokens: 10,
        ..Default::default()
    });
    let line = context_meter(&usage, 32_000).expect("meter");
    assert!(
        !line.text.contains("cache"),
        "an unreported cache is absent, not 0%: {}",
        line.text
    );
}
