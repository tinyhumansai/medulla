//! Tests for the authoring gates.
//!
//! Two obligations, and the second is the one that keeps a gate trustworthy:
//! it fires on the graph that is guaranteed broken, and it stays silent on
//! everything else. A gate with false positives costs authors their edits and
//! teaches them to route around it.

use super::*;
use serde_json::json;

/// A graph from a node list, with no edges — the gates read configs, not
/// topology, and a trigger would only be noise here.
fn graph(nodes: serde_json::Value) -> WorkflowGraph {
    serde_json::from_value(json!({ "name": "test", "nodes": nodes, "edges": [] }))
        .expect("graph parses")
}

// ---- prompts written as expressions ----

#[test]
fn an_instruction_written_as_an_expression_is_refused() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "=You are given an issue: .item. Summarise it" } },
    ]));

    let failures = failures(&graph);

    // `=` does not interpolate. The whole expression resolves to null and the
    // node dispatches a harness session with nothing to do.
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("does not interpolate"), "{failures:?}");
    assert!(failures[0].contains("work"), "the node has to be named");
}

#[test]
fn the_instruction_alias_is_checked_too() {
    // `instruction` is what the rest of Medulla calls the same field, and the
    // engine accepts it — so a gate that only read `prompt` would miss half of
    // what authors actually write.
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "instruction": "=Look at the diff and fix it" } },
    ]));

    assert_eq!(failures(&graph).len(), 1, "{:?}", failures(&graph));
}

#[test]
fn a_plain_instruction_is_left_alone() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "You are given an issue. Summarise it." } },
    ]));

    assert!(failures(&graph).is_empty());
}

#[test]
fn a_real_expression_is_not_mistaken_for_prose() {
    for expr in [
        "=.item.text",
        "=nodes.fetch.item.json.title",
        "=if .item.ok then .item.text else \"none\" end",
        "=.item.issues | map(.title) | join(\", \")",
        "=\"Summarise this issue for me\"",
    ] {
        let graph = graph(json!([
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": expr } },
        ]));

        assert!(
            failures(&graph).is_empty(),
            "{expr} is valid jq and must not be refused: {:?}",
            failures(&graph)
        );
    }
}

// ---- the output envelope ----

#[test]
fn reading_an_agents_output_without_the_envelope_is_refused() {
    let graph = graph(json!([
        { "id": "fetch", "kind": "agent", "name": "Fetch", "config": { "prompt": "get it" } },
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo", "args": { "text": "=nodes.fetch.item.title" } } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("args.text"), "{failures:?}");
    // The message has to carry the correction, not just the complaint.
    assert!(
        failures[0].contains("=nodes.fetch.item.json.title"),
        "{failures:?}"
    );
}

#[test]
fn reading_through_the_envelope_is_accepted() {
    let graph = graph(json!([
        { "id": "fetch", "kind": "agent", "name": "Fetch", "config": { "prompt": "get it" } },
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo",
                      "args": { "text": "=nodes.fetch.item.json.title" } } },
    ]));

    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

#[test]
fn a_node_kind_that_does_not_wrap_its_output_is_read_directly() {
    // A transform's output is the item itself, so `.item.<field>` is correct
    // there and refusing it would be a false positive.
    let graph = graph(json!([
        { "id": "shape", "kind": "transform", "name": "Shape",
          "config": { "set": { "title": "=.item.name" } } },
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo", "args": { "text": "=nodes.shape.item.title" } } },
    ]));

    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

#[test]
fn a_binding_nested_deep_inside_args_is_still_found_and_named() {
    let graph = graph(json!([
        { "id": "fetch", "kind": "agent", "name": "Fetch", "config": { "prompt": "get it" } },
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo", "args": {
              "blocks": [{ "fields": { "value": "=nodes.fetch.item.title" } }] } } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    // Named precisely enough for an author to find it in a large config.
    assert!(
        failures[0].contains("args.blocks.0.fields.value"),
        "{failures:?}"
    );
}

#[test]
fn a_binding_to_a_node_that_does_not_exist_is_left_to_the_engine() {
    let graph = graph(json!([
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo", "args": { "text": "=nodes.ghost.item.title" } } },
    ]));

    // The engine already reports a reference to a node that is not there;
    // saying it twice in different words helps nobody.
    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

#[test]
fn an_expression_that_is_not_a_node_binding_is_not_second_guessed() {
    let graph = graph(json!([
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo",
                      "args": { "text": "=.item.text | ascii_downcase",
                                "count": "=run.trigger.n" } } },
    ]));

    // A gate that guessed at arbitrary jq would refuse graphs that work.
    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

// ---- the error surface ----

#[test]
fn check_reports_every_failure_at_once_rather_than_the_first() {
    let graph = graph(json!([
        { "id": "fetch", "kind": "agent", "name": "Fetch",
          "config": { "prompt": "=Go and fetch the issues" } },
        { "id": "notify", "kind": "tool_call", "name": "Notify",
          "config": { "slug": "medulla:echo", "args": { "text": "=nodes.fetch.item.title" } } },
    ]));

    let err = check("sweep", &graph).expect_err("both are wrong");

    let WorkflowError::Invalid { id, messages } = err else {
        panic!("expected Invalid");
    };
    assert_eq!(id, "sweep");
    // One round trip has to tell an agent everything, or it spends a turn per
    // mistake.
    assert_eq!(messages.len(), 2, "{messages:?}");
}

#[test]
fn a_clean_graph_passes() {
    let graph = graph(json!([
        { "id": "t", "kind": "trigger", "name": "Start",
          "config": { "trigger_kind": "manual" } },
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "summarise the open issues" } },
    ]));

    assert!(check("sweep", &graph).is_ok());
}

// ---- code node languages ----

#[test]
fn a_code_node_asking_for_shell_is_refused_and_pointed_at_the_shell_tool() {
    let graph = graph(json!([
        { "id": "compute", "kind": "code", "name": "Compute",
          "config": { "language": "shell", "source": "echo hi" } },
    ]));

    let failures = failures(&graph);

    // The engine treats anything but the literal "python" as JavaScript, so
    // this would run a shell script through node and fail with a syntax error
    // naming an interpreter the author never chose.
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("medulla:shell"), "{failures:?}");
}

#[test]
fn a_near_miss_language_spelling_is_refused_rather_than_silently_becoming_javascript() {
    for spelling in ["python3", "py", "js", "node"] {
        let graph = graph(json!([
            { "id": "compute", "kind": "code", "name": "Compute",
              "config": { "language": spelling, "source": "print(1)" } },
        ]));

        assert_eq!(
            failures(&graph).len(),
            1,
            "{spelling} must not silently become javascript"
        );
    }
}

#[test]
fn the_two_spellings_the_engine_actually_reads_are_accepted() {
    for spelling in ["javascript", "python"] {
        let graph = graph(json!([
            { "id": "compute", "kind": "code", "name": "Compute",
              "config": { "language": spelling, "source": "x" } },
        ]));

        assert!(
            failures(&graph).is_empty(),
            "{spelling} is exactly what the engine matches: {:?}",
            failures(&graph)
        );
    }
}

#[test]
fn a_code_node_that_names_no_language_is_left_alone() {
    let graph = graph(json!([
        { "id": "compute", "kind": "code", "name": "Compute",
          "config": { "source": "console.log(1)" } },
    ]));

    // Absent is legal and means JavaScript, which the engine's own default
    // already says — refusing it would be a false positive.
    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

// ---- harness and model selection ----

#[test]
fn a_misspelled_builtin_harness_is_refused() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": "claude code" } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("work"), "{failures:?}");
    assert!(failures[0].contains("custom harness id"), "{failures:?}");
}

#[test]
fn a_harness_written_as_an_expression_is_refused() {
    // The injection case: whichever value flowed down the graph would choose
    // which binary and which credentials run the next instruction.
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": "=nodes.pick.item.json.harness" } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("decision the graph makes"),
        "{failures:?}"
    );
}

#[test]
fn a_non_string_harness_is_refused_by_type() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": true } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("must be a string"), "{failures:?}");
}

#[test]
fn the_provider_alias_is_checked_too() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "provider": "claude code" } },
    ]));

    assert_eq!(failures(&graph).len(), 1);
}

#[test]
fn a_legitimate_harness_choice_passes() {
    // Built-in names, custom preset ids, a model on its own, and an omitted
    // choice are all ordinary authoring — a gate that fired on any of them
    // would cost an author their edit for nothing.
    let graph = graph(json!([
        { "id": "a", "kind": "agent", "name": "A",
          "config": { "prompt": "go", "harness": "codex", "model": "gpt-5-codex" } },
        { "id": "b", "kind": "agent", "name": "B",
          "config": { "prompt": "go", "harness": "deepseek-claude" } },
        { "id": "c", "kind": "agent", "name": "C",
          "config": { "prompt": "go", "model": "=nodes.pick.item.json.model" } },
        { "id": "d", "kind": "agent", "name": "D", "config": { "prompt": "go" } },
        { "id": "e", "kind": "agent", "name": "E",
          "config": { "prompt": "go", "harness": "" } },
    ]));

    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

#[test]
fn a_harness_on_a_node_that_is_not_an_agent_is_not_this_gate_s_business() {
    // Other node kinds do not dispatch to a harness, so the key is inert there
    // — refusing it would be a gate firing on a graph that works.
    let graph = graph(json!([
        { "id": "t", "kind": "transform", "name": "T",
          "config": { "harness": "not a harness" } },
    ]));

    assert!(failures(&graph).is_empty());
}
