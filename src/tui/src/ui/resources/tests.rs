//! Formatting tests for local-process resource indicators.

use medulla::config::{AppearanceConfig, ResourceDisplay};

use super::{segments, ResourceSnapshot};

fn sample() -> ResourceSnapshot {
    ResourceSnapshot {
        cpu_fraction: 0.25,
        memory_bytes: 512 * 1024 * 1024,
        total_memory_bytes: 2 * 1024 * 1024 * 1024,
        disk_read_bytes_per_second: 1024.0,
        disk_write_bytes_per_second: 2048.0,
        disk_peak_bytes_per_second: 4096.0,
    }
}

#[test]
fn disabled_resources_draw_nothing() {
    assert!(segments(&AppearanceConfig::default(), sample()).is_empty());
}

#[test]
fn each_resource_uses_its_selected_format() {
    let config = AppearanceConfig {
        cpu: ResourceDisplay::Percent,
        ram: ResourceDisplay::Value,
        disk_io: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    assert_eq!(
        segments(&config, sample()),
        ["CPU 25%", "RAM 512.0M", "IO ███░░ R 1.0K/s W 2.0K/s"]
    );
}
