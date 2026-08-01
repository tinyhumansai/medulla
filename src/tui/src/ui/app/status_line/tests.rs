//! Tests for the Status line page's mutations and their persistence.

use std::sync::Arc;

use medulla::config::{
    AppearanceConfig, ControlStyle, FieldPlacement, HarnessNameStyle, LoadedConfig, PathStyle,
    StatusLineConfig, TuiConfig,
};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

use super::{StatusLineField, STATUS_LINE_ROWS};
use crate::ui::app::App;

fn app(path: &std::path::Path) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults(path.display().to_string()));
    app.set_config_path(path.to_path_buf());
    app
}

fn saved(path: &std::path::Path) -> TuiConfig {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The page row that edits `field`.
fn row_of(field: StatusLineField) -> usize {
    STATUS_LINE_ROWS
        .iter()
        .position(|row| row.field == field)
        .expect("every field has a row")
}

#[test]
fn placements_apply_live_and_persist_independently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = app(&path);

    // Branch: line 1 → line 2.
    app.status_line_index = row_of(StatusLineField::Branch);
    app.cycle_status_line_row(true);
    assert_eq!(app.status_line_config().branch, FieldPlacement::Line2);
    assert_eq!(saved(&path).status_line().branch, FieldPlacement::Line2);
    // Nothing else moved.
    assert_eq!(saved(&path).status_line().path, FieldPlacement::Line1);

    // Path: line 1 → line 2 → line 3.
    app.status_line_index = row_of(StatusLineField::Path);
    app.cycle_status_line_row(true);
    app.cycle_status_line_row(true);
    let saved = saved(&path);
    assert_eq!(saved.status_line().path, FieldPlacement::Line3);
    assert_eq!(saved.status_line().branch, FieldPlacement::Line2);
}

#[test]
fn a_placement_cycles_through_every_line_and_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir.path().join("config.toml"));
    app.status_line_index = row_of(StatusLineField::State);

    for expected in [
        FieldPlacement::Line2,
        FieldPlacement::Line3,
        FieldPlacement::Hidden,
        FieldPlacement::Line1,
    ] {
        app.cycle_status_line_row(true);
        assert_eq!(app.status_line_config().state, expected);
    }
}

#[test]
fn styles_cycle_and_persist_separately_from_placement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = app(&path);

    app.status_line_index = row_of(StatusLineField::HarnessStyle);
    app.cycle_status_line_row(true);
    app.status_line_index = row_of(StatusLineField::ControlStyle);
    app.cycle_status_line_row(true);
    app.status_line_index = row_of(StatusLineField::PathStyle);
    app.cycle_status_line_row(false);

    let saved = saved(&path).status_line();
    assert_eq!(saved.harness_style, HarnessNameStyle::Icon);
    assert_eq!(saved.control_style, ControlStyle::Icon);
    assert_eq!(saved.path_style, PathStyle::Full);
    // Placements are untouched by a style change.
    assert_eq!(saved.harness, FieldPlacement::Line1);
    assert_eq!(saved.path, FieldPlacement::Line1);
}

#[test]
fn a_change_without_a_config_path_still_applies_live() {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("unused.toml".into()));
    app.status_line_index = row_of(StatusLineField::Branch);

    app.cycle_status_line_row(true);

    assert_eq!(app.status_line_config().branch, FieldPlacement::Line2);
    assert!(app.status().contains("not persisted"));
}

#[test]
fn the_legacy_appearance_toggles_are_read_as_placements() {
    let hidden = AppearanceConfig {
        show_harness_branch: false,
        show_harness_path: true,
    };
    let derived = StatusLineConfig::from_appearance(&hidden);

    assert_eq!(derived.branch, FieldPlacement::Hidden);
    assert_eq!(derived.path, FieldPlacement::Line1);
    assert_eq!(
        derived.state,
        FieldPlacement::Line1,
        "fields the old keys said nothing about keep their defaults"
    );
}

#[test]
fn first_edit_persists_the_complete_legacy_derived_layout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[appearance]\nshowHarnessBranch = false\nshowHarnessPath = false\n",
    )
    .unwrap();

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults(path.display().to_string());
    loaded.config.appearance.show_harness_branch = false;
    loaded.config.appearance.show_harness_path = false;
    let mut app = App::new(runtime, loaded);
    app.set_config_path(path.clone());
    app.status_line_index = row_of(StatusLineField::State);

    app.cycle_status_line_row(true);

    let persisted = saved(&path).status_line();
    assert_eq!(persisted.state, FieldPlacement::Line2);
    assert_eq!(persisted.branch, FieldPlacement::Hidden);
    assert_eq!(persisted.path, FieldPlacement::Hidden);
}

#[test]
fn an_explicit_status_line_section_wins_over_the_legacy_keys() {
    let mut config = TuiConfig::default();
    config.appearance.show_harness_path = false;
    assert_eq!(config.status_line().path, FieldPlacement::Hidden);

    config.status_line = Some(StatusLineConfig {
        path: FieldPlacement::Line2,
        ..StatusLineConfig::default()
    });

    assert_eq!(config.status_line().path, FieldPlacement::Line2);
}
