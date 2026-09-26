//! Tests for the draw module.

use super::super::types::format_usd as usd;
use super::{bytes, meter, METER_WIDTH};

#[test]
fn meter_fills_proportionally_and_clamps() {
    assert_eq!(meter(0.0), format!("[{}]", "░".repeat(METER_WIDTH)));
    assert_eq!(meter(1.0), format!("[{}]", "█".repeat(METER_WIDTH)));
    assert_eq!(meter(2.0), format!("[{}]", "█".repeat(METER_WIDTH)));
    assert_eq!(meter(-1.0), format!("[{}]", "░".repeat(METER_WIDTH)));
    assert!(meter(0.5).contains('█'));
    assert!(meter(0.5).contains('░'));
}

#[test]
fn usd_drops_cents_for_whole_dollars() {
    assert_eq!(usd(25.0), "$25");
    assert_eq!(usd(0.0), "$0");
    assert_eq!(usd(7.5), "$7.50");
}

#[test]
fn bytes_scales_units() {
    assert_eq!(bytes(512), "512 B");
    assert_eq!(bytes(2048), "2 KB");
    assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
}
