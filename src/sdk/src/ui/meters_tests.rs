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
    // The window is named, then the breakdown in one bracket.
    assert!(
        line.text
            .contains("32k window · (in 24k / out 2k / cached 75%)"),
        "{}",
        line.text
    );
    assert_eq!(line.color.as_deref(), Some("yellow"), "75% of the window");

    // An untouched lane has nothing to meter.
    assert!(context_meter(&LaneUsage::default(), 32_000).is_none());
}

#[test]
fn the_meter_names_a_million_token_window_without_spurious_precision() {
    let mut usage = LaneUsage::default();
    usage.accumulate(&Usage {
        input_tokens: 40_000,
        output_tokens: 900,
        cache_read_tokens: Some(20_000),
        cache_creation_tokens: None,
    });
    let line = context_meter(&usage, 1_000_000).expect("meter");
    // `1M`, not `1.0M`: a decimal point with nothing after it reads as
    // precision that was not measured.
    assert!(
        line.text
            .contains("1M window · (in 40k / out 900 / cached 50%)"),
        "{}",
        line.text
    );
    // And the same prompt against the real window is no longer an alarm: 4% of
    // 1M is green, where the old 32k default painted these same 40k tokens red
    // at over 100% of a window the model does not actually have.
    assert_eq!(line.color.as_deref(), Some("green"));
    assert_eq!(
        context_meter(&usage, 32_000)
            .expect("meter")
            .color
            .as_deref(),
        Some("red"),
        "the understated window is what made this an alarm"
    );
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
    // The field keeps its place so the row does not change shape between
    // providers, but it states that nothing was reported rather than claiming a
    // figure. `0%` would be a measured cache miss, which is not what happened.
    assert!(
        line.text.contains("cached —"),
        "silence should read as a dash: {}",
        line.text
    );
    assert!(
        !line.text.contains("cached 0%"),
        "an unreported cache must never be inferred as 0%: {}",
        line.text
    );
}
