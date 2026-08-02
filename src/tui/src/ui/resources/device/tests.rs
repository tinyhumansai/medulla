//! Deterministic formatting tests for the device sidebar footer.
//!
//! Every case injects a fixed [`DeviceSnapshot`]; nothing here samples the host,
//! so the assertions are exact rather than "contains a digit".

use medulla::config::{AppearanceConfig, ResourceDisplay};

use super::super::types::DeviceSnapshot;
use super::{device_lines, device_width_hint, DeviceMonitor};

/// A 32 GiB machine at 25% CPU with 8 GiB in use and a 400 GiB disk 75% full.
fn sample() -> DeviceSnapshot {
    DeviceSnapshot {
        cpu_fraction: Some(0.25),
        memory_used_bytes: Some(8 * 1024 * 1024 * 1024),
        memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        disk_used_bytes: Some(300 * 1024 * 1024 * 1024),
        disk_total_bytes: Some(400 * 1024 * 1024 * 1024),
    }
}

/// Every device metric is labelled "Device …" so it cannot be read as the
/// Medulla process's own usage on the status line.
#[test]
fn each_device_metric_uses_its_selected_format_and_a_device_label() {
    let config = AppearanceConfig {
        device_cpu: ResourceDisplay::Percent,
        device_ram: ResourceDisplay::Value,
        device_disk: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    assert_eq!(
        device_lines(&config, sample(), 30, 3),
        [
            "Device CPU 25%",
            "Device RAM 8G/32G",
            "Device disk ███░ 75%"
        ]
    );
}

#[test]
fn metrics_are_toggled_independently() {
    let config = AppearanceConfig {
        device_ram: ResourceDisplay::Percent,
        ..AppearanceConfig::default()
    };
    assert_eq!(device_lines(&config, sample(), 30, 3), ["Device RAM 25%"]);
    assert!(device_lines(&AppearanceConfig::default(), sample(), 30, 3).is_empty());
}

#[test]
fn unavailable_readings_render_without_hiding_available_metrics() {
    let config = AppearanceConfig {
        device_cpu: ResourceDisplay::Percent,
        device_ram: ResourceDisplay::Value,
        device_disk: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    let sample = DeviceSnapshot {
        cpu_fraction: None,
        // A used byte count with no total cannot be turned into pressure.
        memory_used_bytes: Some(1),
        memory_total_bytes: None,
        disk_used_bytes: Some(1),
        disk_total_bytes: Some(4),
    };
    assert_eq!(
        device_lines(&config, sample, 30, 3),
        ["Device CPU —", "Device RAM —", "Device disk █░░░ 25%"]
    );
}

#[test]
fn narrow_rails_degrade_values_and_bars_to_percentages() {
    let config = AppearanceConfig {
        device_cpu: ResourceDisplay::Bar,
        device_ram: ResourceDisplay::Value,
        device_disk: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    assert_eq!(
        device_lines(&config, sample(), 16, 3),
        ["Device CPU 25%", "Device RAM 25%", "Device disk 75%"]
    );
}

#[test]
fn a_short_sidebar_drops_the_lowest_priority_metrics_first() {
    let config = AppearanceConfig {
        device_cpu: ResourceDisplay::Percent,
        device_ram: ResourceDisplay::Percent,
        device_disk: ResourceDisplay::Percent,
        ..AppearanceConfig::default()
    };
    assert_eq!(
        device_lines(&config, sample(), 30, 2),
        ["Device CPU 25%", "Device RAM 25%"]
    );
    assert!(device_lines(&config, sample(), 30, 0).is_empty());
}

#[test]
fn small_filesystems_report_megabytes_rather_than_zero_gigabytes() {
    let config = AppearanceConfig {
        device_disk: ResourceDisplay::Value,
        ..AppearanceConfig::default()
    };
    let sample = DeviceSnapshot {
        disk_used_bytes: Some(200 * 1024 * 1024),
        disk_total_bytes: Some(512 * 1024 * 1024),
        ..DeviceSnapshot::default()
    };
    assert_eq!(
        device_lines(&config, sample, 30, 3),
        ["Device disk 200M/512M"]
    );
}

/// An injected reading short-circuits sampling entirely, which is what lets
/// render tests assert on device output without touching the host.
#[test]
fn an_injected_reading_is_returned_verbatim() {
    let mut monitor = DeviceMonitor::default();
    monitor.inject(sample());
    assert_eq!(monitor.sample(), sample());
    assert_eq!(monitor.sample(), sample());
}

#[test]
fn a_diskless_sample_can_be_upgraded_within_the_refresh_interval() {
    let mut monitor = DeviceMonitor::default();
    let without_disk = monitor.sample_for(false);
    assert_eq!(without_disk.disk_used_bytes, None);
    assert_eq!(without_disk.disk_total_bytes, None);

    // Requesting disk immediately afterwards must not reuse the intentionally
    // diskless cached sample.
    let with_disk = monitor.sample_for(true);
    assert_eq!(
        with_disk.disk_used_bytes.is_some(),
        with_disk.disk_total_bytes.is_some()
    );
}

/// A smoke test over real sampling, asserting only invariants that hold on any
/// host: nothing panics, CPU is withheld until there are two refreshes to
/// compare, and any reported figures are internally consistent. Values are
/// deliberately not asserted — that is what the injected cases above are for.
#[test]
fn live_sampling_is_self_consistent_and_withholds_cpu_on_the_first_reading() {
    let mut monitor = DeviceMonitor::default();
    let first = monitor.sample();
    assert_eq!(first.cpu_fraction, None, "{first:?}");
    // Within the refresh interval the same reading is handed back rather than
    // re-polling the host on every redraw.
    assert_eq!(monitor.sample(), first);

    for snapshot in [first, monitor.sample()] {
        if let Some(fraction) = snapshot.cpu_fraction {
            assert!((0.0..=1.0).contains(&fraction), "{snapshot:?}");
        }
        // A used figure never appears without the total it is used out of, and
        // never exceeds it.
        for (used, total) in [
            (snapshot.memory_used_bytes, snapshot.memory_total_bytes),
            (snapshot.disk_used_bytes, snapshot.disk_total_bytes),
        ] {
            assert_eq!(used.is_some(), total.is_some(), "{snapshot:?}");
            if let Some((used, total)) = used.zip(total) {
                assert!(total > 0 && used <= total, "{snapshot:?}");
            }
        }
    }
}

#[test]
fn an_oversized_value_pair_falls_back_to_the_percentage() {
    let config = AppearanceConfig {
        device_disk: ResourceDisplay::Value,
        ..AppearanceConfig::default()
    };
    // `bytes` tops out at gigabytes, so a petabyte-scale filesystem renders a
    // very wide pair on a rail that comfortably fits ordinary readings. The
    // line must degrade rather than overflow into the navigation rows.
    let sample = DeviceSnapshot {
        cpu_fraction: None,
        memory_used_bytes: None,
        memory_total_bytes: None,
        disk_used_bytes: Some(9_500_000_000_000_000),
        disk_total_bytes: Some(10_000_000_000_000_000),
    };
    assert_eq!(device_lines(&config, sample, 25, 3), ["Device disk 95%"]);

    // With room for the full pair it is kept.
    assert_eq!(
        device_lines(&config, sample, 40, 3),
        ["Device disk 8847564G/9313226G"]
    );
}

#[test]
fn completed_value_and_bar_lines_fall_back_at_their_exact_boundaries() {
    let value_config = AppearanceConfig {
        device_disk: ResourceDisplay::Value,
        ..AppearanceConfig::default()
    };
    let ten_tib = 10 * 1024 * 1024 * 1024 * 1024;
    let full_disk = DeviceSnapshot {
        disk_used_bytes: Some(ten_tib),
        disk_total_bytes: Some(ten_tib),
        ..DeviceSnapshot::default()
    };
    assert_eq!(
        device_lines(&value_config, full_disk, 24, 3),
        ["Device disk 100%"]
    );

    let bar_config = AppearanceConfig {
        device_disk: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    assert_eq!(
        device_lines(&bar_config, full_disk, 20, 3),
        ["Device disk 100%"]
    );
}

#[test]
fn a_value_pair_is_kept_on_any_rail_wide_enough_to_hold_it() {
    let config = AppearanceConfig {
        device_ram: ResourceDisplay::Value,
        ..AppearanceConfig::default()
    };
    // "Device RAM 8G/32G" is 17 columns. A fixed minimum-width gate used to
    // reject it on rails narrower than 24 even though it fits, so the reading
    // degraded to a percentage for no reason.
    assert_eq!(
        device_lines(&config, sample(), 17, 3),
        ["Device RAM 8G/32G"]
    );
    // One column short, and the percentage is the honest fallback.
    assert_eq!(device_lines(&config, sample(), 16, 3), ["Device RAM 25%"]);
}

#[test]
fn the_width_hint_reserves_room_for_a_configured_bar() {
    let config = AppearanceConfig {
        device_cpu: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    // "Device CPU ████ 100%" — the rail must be sized to this, not to the 18
    // columns the orchestrator row alone would ask for, or the bar would
    // degrade to a percentage on every frame and the setting would look broken.
    assert_eq!(device_width_hint(&config, sample()), 20);

    // Sized against a saturated reading, so the hint does not shrink when the
    // machine is idle and make the rail jitter as usage moves.
    let idle = DeviceSnapshot {
        cpu_fraction: Some(0.0),
        ..sample()
    };
    assert_eq!(device_width_hint(&config, idle), 20);

    // Nothing enabled asks for nothing.
    assert_eq!(device_width_hint(&AppearanceConfig::default(), sample()), 0);
}

#[test]
fn the_width_hint_reserves_room_for_every_bar_at_full() {
    let config = AppearanceConfig {
        device_disk: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    // The sample's disk sits at 75%, rendering `Device disk ███░ 75%` — 20
    // columns. At 100% the same line needs 21 for the extra digit, so a hint
    // taken from the live reading alone would leave the bar a column short and
    // silently degrade it to a percentage even on a wide terminal.
    assert_eq!(device_width_hint(&config, sample()), 21);

    // A value pair is widest at whatever magnitudes the host reports, so the
    // live reading has to be measured as well: saturating RAM to 32G/32G gives
    // 18 columns against the live 8G/32G's 17.
    let values = AppearanceConfig {
        device_ram: ResourceDisplay::Value,
        ..AppearanceConfig::default()
    };
    assert_eq!(device_width_hint(&values, sample()), 18);
}
