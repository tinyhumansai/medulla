//! Unit tests for the `mcp` config section: serde defaults, camelCase parsing,
//! and the in-flight ceiling's fallback onto the workflow fan-out limit.

use super::*;

/// Parse a `[mcp]` section from JSON, the way a `medulla.tui.json` would carry
/// it, and hand back the whole document so the section's default can be
/// observed when the key is absent entirely.
fn parse(json: serde_json::Value) -> TuiConfig {
    serde_json::from_value(json).expect("a valid config document")
}

#[test]
fn an_absent_section_serves_fleet_tools_two_levels_deep() {
    // The defaults are the shipped behaviour for everyone who never edits
    // config, so they are worth pinning rather than inferring from the struct.
    let config = parse(serde_json::json!({}));

    assert!(config.mcp.fleet_tools);
    assert_eq!(config.mcp.max_depth, 2);
    assert_eq!(config.mcp.max_in_flight, None);
    assert_eq!(config.mcp.socket_path, None);
}

#[test]
fn keys_are_read_in_camel_case() {
    let config = parse(serde_json::json!({
        "mcp": {
            "fleetTools": false,
            "maxDepth": 0,
            "maxInFlight": 3,
            "socketPath": "/run/medulla/control.sock",
        }
    }));

    assert!(!config.mcp.fleet_tools);
    assert_eq!(config.mcp.max_depth, 0);
    assert_eq!(config.mcp.max_in_flight, Some(3));
    assert_eq!(
        config.mcp.socket_path.as_deref(),
        Some("/run/medulla/control.sock")
    );
}

#[test]
fn a_partial_section_keeps_the_defaults_for_everything_it_omits() {
    // `#[serde(default)]` on the struct rather than only on its fields: an
    // operator turning one knob must not silently reset the others.
    let config = parse(serde_json::json!({ "mcp": { "maxDepth": 1 } }));

    assert_eq!(config.mcp.max_depth, 1);
    assert!(config.mcp.fleet_tools);
    assert_eq!(config.mcp.max_in_flight, None);
}

#[test]
fn the_in_flight_ceiling_follows_the_workflow_fan_out_limit_when_unset() {
    let section = McpSection::default();

    assert_eq!(section.effective_max_in_flight(4), 4);
    assert_eq!(section.effective_max_in_flight(16), 16);
}

#[test]
fn an_explicit_in_flight_ceiling_wins_over_the_workflow_limit() {
    let section = McpSection {
        max_in_flight: Some(2),
        ..McpSection::default()
    };

    assert_eq!(section.effective_max_in_flight(16), 2);
}

#[test]
fn the_in_flight_ceiling_never_reaches_zero() {
    // A zero ceiling would advertise `fleet_dispatch` and refuse every call,
    // which reads to a model as a broken tool rather than a disabled one.
    // Turning the tools off is what `fleetTools = false` is for.
    let explicit = McpSection {
        max_in_flight: Some(0),
        ..McpSection::default()
    };

    assert_eq!(explicit.effective_max_in_flight(8), 1);
    assert_eq!(McpSection::default().effective_max_in_flight(0), 1);
}

#[test]
fn the_section_round_trips_through_serialization() {
    // The TUI writes config back after an edit; a field that serializes under a
    // different name than it parses would silently reset on the next boot.
    let section = McpSection {
        fleet_tools: false,
        max_depth: 3,
        max_in_flight: Some(5),
        socket_path: Some("/tmp/control.sock".to_string()),
    };

    let round_tripped: McpSection =
        serde_json::from_value(serde_json::to_value(&section).unwrap()).unwrap();

    assert_eq!(round_tripped, section);
}

#[test]
fn absent_optional_keys_are_not_written_back() {
    // `skip_serializing_if` keeps a defaults-only section from writing
    // `socketPath: null` into an operator's file, which TOML cannot even
    // represent.
    let value = serde_json::to_value(McpSection::default()).unwrap();

    assert!(value.get("maxInFlight").is_none());
    assert!(value.get("socketPath").is_none());
}
