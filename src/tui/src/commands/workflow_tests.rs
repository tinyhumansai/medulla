//! Unit tests for `medulla workflow`'s flag resolution.
//!
//! Only the decisions that need no store, no config file, and no daemon are
//! here; everything that runs a workflow lives in the crate's `tests/`
//! directory.

use medulla::config::WorkflowsConfig;
use medulla_tui::cli::WorkflowArgs;

use super::run_model;

/// A config whose only interesting field is the pinned default model.
fn config(default_model: &str) -> WorkflowsConfig {
    WorkflowsConfig {
        default_model: default_model.to_string(),
        ..WorkflowsConfig::default()
    }
}

/// With no flag the host's configured default answers, exactly as before the
/// flag existed.
#[test]
fn the_configured_default_is_used_when_no_flag_is_given() {
    let parsed = WorkflowArgs::default();
    assert_eq!(
        run_model(&parsed, &config("host/model")).as_deref(),
        Some("host/model")
    );
    assert!(run_model(&parsed, &config("")).is_none());
}

/// `--model` replaces it for this invocation — the one route to choosing an
/// embedded OpenHuman turn's model without editing anything on disk.
#[test]
fn the_flag_overrides_the_configured_default() {
    let parsed = WorkflowArgs {
        model: Some("deepseek/deepseek-v4-pro".into()),
        ..WorkflowArgs::default()
    };
    assert_eq!(
        run_model(&parsed, &config("host/model")).as_deref(),
        Some("deepseek/deepseek-v4-pro")
    );
}

/// An explicitly empty value clears the configured default rather than being
/// ignored, matching what the same flag means on `workflow defaults`. Padding
/// is trimmed on the way through, so a quoted shell argument does not become a
/// model id nothing serves.
#[test]
fn an_empty_flag_clears_the_default_and_values_are_trimmed() {
    let parsed = WorkflowArgs {
        model: Some("   ".into()),
        ..WorkflowArgs::default()
    };
    assert!(run_model(&parsed, &config("host/model")).is_none());

    let parsed = WorkflowArgs {
        model: Some("  spaced/model  ".into()),
        ..WorkflowArgs::default()
    };
    assert_eq!(
        run_model(&parsed, &config("")).as_deref(),
        Some("spaced/model")
    );
}
