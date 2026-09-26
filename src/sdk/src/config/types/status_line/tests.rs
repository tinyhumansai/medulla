//! Unit tests for status-line option cycling and persisted wire values.

use super::*;

#[test]
fn every_option_cycle_wraps_backwards() {
    assert_eq!(FieldPlacement::Line1.cycled(false), FieldPlacement::Hidden);
    assert_eq!(
        FieldVisibility::Always.cycled(false),
        FieldVisibility::Alert
    );
    assert_eq!(HarnessNameStyle::Long.cycled(false), HarnessNameStyle::Icon);
    assert_eq!(ControlStyle::Text.cycled(false), ControlStyle::Icon);
    assert_eq!(PathStyle::Full.cycled(false), PathStyle::Last);
}

#[test]
fn wire_values_match_camel_case_config_spelling() {
    assert_eq!(wire_value(&FieldPlacement::Line2), "line2");
    assert_eq!(wire_value(&FieldVisibility::Active), "active");
    assert_eq!(wire_value(&HarnessNameStyle::Icon), "icon");
    assert_eq!(wire_value(&ControlStyle::Text), "text");
    assert_eq!(wire_value(&PathStyle::Shortened), "shortened");
}
