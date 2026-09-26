//! The shipped example workflows must stay valid.
//!
//! An example is the first thing an operator copies and the first thing a model
//! is pointed at, so one that no longer compiles is worse than no example. This
//! guards them against drift in the engine's schema or this host's validation.

#![cfg(feature = "workflows")]

use medulla::workflows::store::{parse_workflow, validate_graph};

/// Every `examples/workflows/*.json` in the repository.
fn shipped_examples() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("workflows");
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()));

    entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
            (stem, body)
        })
        .collect()
}

#[test]
fn at_least_one_example_ships() {
    assert!(
        !shipped_examples().is_empty(),
        "the docs point operators at examples/workflows; it must not be empty"
    );
}

#[test]
fn every_shipped_example_parses_and_validates() {
    for (id, body) in shipped_examples() {
        let record =
            parse_workflow(&body, &id).unwrap_or_else(|err| panic!("{id} does not parse: {err}"));
        validate_graph(&record.id, &record.graph)
            .unwrap_or_else(|err| panic!("{id} does not validate: {err}"));
    }
}

#[test]
fn every_shipped_example_describes_itself() {
    // An example with no description is a puzzle, not an example.
    for (id, body) in shipped_examples() {
        let record = parse_workflow(&body, &id).expect("parses");
        assert!(
            !record.description.trim().is_empty(),
            "{id} needs a description"
        );
        assert!(!record.name.trim().is_empty(), "{id} needs a name");
    }
}
