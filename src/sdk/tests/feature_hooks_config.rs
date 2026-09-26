//! A `[[hooks]]` section in a real config file, all the way to the argv a
//! spawned harness is launched with.
//!
//! The unit tests in `harness_hooks` cover the vocabulary and each harness's
//! delivery in isolation. What they cannot cover is the join: that the section
//! an operator actually types parses into [`HooksConfig`], survives
//! `load_config`, and comes out the other side as a flag the harness reads.
//! Every link in that chain has its own spelling rules, and a break anywhere
//! shows up as hooks that silently never fire.

use std::collections::HashMap;

use medulla::protocol::HarnessProvider;

/// Write `body` as a config file and load it the way the app does.
fn load_raw(body: &str) -> medulla::config::LoadedConfig {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("medulla.tui.toml");
    std::fs::write(&path, body).expect("the config is writable");
    medulla::config::load_config(
        Some(path.to_str().expect("a utf-8 path")),
        &HashMap::new(),
        dir.path(),
    )
    .expect("the config loads")
}

/// Load `body` with Medulla's own reporting hooks switched off.
///
/// Those built-ins are resolved into the same list at load (see
/// [`medulla::harness_hooks::builtin`]), and every case below is about the
/// *operator's* `[[hooks]]` section arriving intact — asserting on absolute
/// positions in a document Medulla also writes into would pin the built-in set
/// rather than the documentation. The two together are covered by
/// [`medullas_own_hooks_do_not_displace_the_operators`].
fn load(body: &str) -> medulla::config::LoadedConfig {
    load_raw(&format!("[hookDefaults]\nenabled = false\n{body}"))
}

/// The `--settings` document Claude Code would be launched with.
fn claude_settings(hooks: &medulla::harness_hooks::HooksConfig) -> serde_json::Value {
    let (args, _notes) =
        medulla::harness_hooks::launch_args(HarnessProvider::Claude, false, hooks, &HashMap::new());
    let index = args
        .iter()
        .position(|arg| arg == "--settings")
        .expect("claude carries hooks on --settings");
    serde_json::from_str(&args[index + 1]).expect("the settings flag holds a JSON document")
}

#[test]
fn a_documented_hooks_section_reaches_the_harness_argv() {
    let loaded = load(
        r#"
[[hooks]]
event = "PostToolUse"
matcher = "Edit|Write"
type = "command"
command = "just fmt"
"#,
    );

    assert_eq!(loaded.config.hooks.hooks.len(), 1);
    let settings = claude_settings(&loaded.config.hooks);
    let entry = &settings["hooks"]["PostToolUse"][0];
    assert_eq!(entry["matcher"], "Edit|Write");
    assert_eq!(entry["hooks"][0]["type"], "command");
    assert_eq!(entry["hooks"][0]["command"], "just fmt");
}

/// The spelling the rest of the config teaches. Before the aliases this failed
/// the *entire* load — Medulla refused to start over one hook's `event`.
#[test]
fn camel_case_events_load_and_are_written_back_canonically() {
    let loaded = load(
        r#"
[[hooks]]
event = "postToolUse"
type = "command"
command = "just fmt"

[[hooks]]
event = "sessionStart"
type = "command"
command = "echo hello"

[[hooks]]
event = "stop"
type = "command"
command = "echo done"
"#,
    );

    assert_eq!(loaded.config.hooks.hooks.len(), 3);
    let settings = claude_settings(&loaded.config.hooks);
    // Canonical on the way out regardless of how they were written: these are
    // the names the harness itself answers to.
    for event in ["PostToolUse", "SessionStart", "Stop"] {
        assert!(
            settings["hooks"].get(event).is_some(),
            "{event} must reach the harness under its canonical name: {settings}"
        );
    }
}

/// Mixing spellings across hooks is not a state anyone should be in, but it
/// must not produce two half-populated event buckets.
#[test]
fn the_two_spellings_of_one_event_land_in_the_same_bucket() {
    let loaded = load(
        r#"
[[hooks]]
event = "PostToolUse"
matcher = "Edit"
type = "command"
command = "first"

[[hooks]]
event = "postToolUse"
matcher = "Write"
type = "command"
command = "second"
"#,
    );

    let settings = claude_settings(&loaded.config.hooks);
    let entries = settings["hooks"]["PostToolUse"]
        .as_array()
        .expect("one bucket, however the events were spelled");
    assert_eq!(entries.len(), 2, "both hooks belong to it: {settings}");
    assert_eq!(entries[0]["hooks"][0]["command"], "first");
    assert_eq!(entries[1]["hooks"][0]["command"], "second");
}

/// A hook restricted to one harness is installed there and nowhere else. The
/// provider names are lowercase, unlike the events — that is `HarnessProvider`'s
/// own `snake_case` serde, and the documentation says so rather than leaving an
/// operator to discover it from a load failure.
#[test]
fn a_harness_restriction_is_honoured_by_name() {
    let loaded = load(
        r#"
[[hooks]]
event = "SessionStart"
type = "command"
command = "codex only"
harnesses = ["codex"]
"#,
    );

    let (claude_args, _) = medulla::harness_hooks::launch_args(
        HarnessProvider::Claude,
        false,
        &loaded.config.hooks,
        &HashMap::new(),
    );
    assert!(
        claude_args.is_empty(),
        "a codex-only hook must not reach claude: {claude_args:?}"
    );

    let injection =
        medulla::harness_hooks::hook_injection(HarnessProvider::Codex, &loaded.config.hooks);
    assert!(
        !injection.args.is_empty(),
        "the harness it names must still get it"
    );
}

/// The two blocks `config.example.toml` shows an operator, verbatim.
///
/// Documentation that does not parse is worse than none — it costs the reader
/// the assumption that anything else in the file is right — so the example is
/// pinned here rather than trusted. Copied by hand on purpose: a test that
/// un-commented the file itself would pass on a snippet nobody can read.
#[test]
fn the_documented_example_parses_and_installs() {
    let loaded = load(
        r#"
[[hooks]]
event = "PostToolUse"
matcher = "Edit|Write"
type = "command"
command = "just fmt"

[[hooks]]
event = "sessionStart"
type = "command"
command = "git fetch --all --prune"
timeout = 30
harnesses = ["claude"]
"#,
    );

    assert_eq!(loaded.config.hooks.hooks.len(), 2);
    let settings = claude_settings(&loaded.config.hooks);
    assert_eq!(
        settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "just fmt"
    );
    let session_start = &settings["hooks"]["SessionStart"][0]["hooks"][0];
    assert_eq!(session_start["command"], "git fetch --all --prune");
    assert_eq!(
        session_start["timeout"], 30,
        "a documented key that never reached the harness would be a lie: {settings}"
    );
}

/// The operator's section and Medulla's own reporting hooks share one document,
/// and the operator's half must survive the join: same command, same bucket, and
/// after the report rather than in place of it.
#[test]
fn medullas_own_hooks_do_not_displace_the_operators() {
    let loaded = load_raw(
        r#"
[[hooks]]
event = "PostToolUse"
matcher = "Edit|Write"
type = "command"
command = "just fmt"
"#,
    );

    // The config file's own contents are the operator's half alone — a built-in
    // must never be mistaken for something they declared.
    let operator = loaded.config.hooks.operator_hooks();
    assert_eq!(operator.len(), 1);
    assert_eq!(operator[0].command(), "just fmt");
    assert!(loaded
        .config
        .hooks
        .hooks
        .iter()
        .any(|hook| hook.builtin && hook.event == medulla::harness_hooks::HookEvent::PostToolUse));

    let settings = claude_settings(&loaded.config.hooks);
    let entries = settings["hooks"]["PostToolUse"]
        .as_array()
        .expect("one bucket, shared with Medulla's own report");
    assert_eq!(entries.len(), 2, "both reach the harness: {settings}");
    // Medulla's report is installed ahead of the operator's hook, so a hook that
    // decides sees the event with the report already on its way.
    assert!(
        entries[0]["hooks"][0]["command"]
            .as_str()
            .expect("a command string")
            .contains("hook PostToolUse"),
        "Medulla's own report goes first: {settings}"
    );
    assert_eq!(entries[1]["matcher"], "Edit|Write");
    assert_eq!(entries[1]["hooks"][0]["command"], "just fmt");
}

/// An event name that is neither spelling fails the load with the offending
/// value in the message. Worth pinning: the failure is loud on purpose, since a
/// hook that silently does not install is the failure this module exists to
/// prevent.
#[test]
fn an_unknown_event_fails_the_load_loudly() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("medulla.tui.toml");
    std::fs::write(
        &path,
        "[[hooks]]\nevent = \"postToolUsage\"\ntype = \"command\"\ncommand = \"x\"\n",
    )
    .expect("the config is writable");

    let error = medulla::config::load_config(
        Some(path.to_str().expect("a utf-8 path")),
        &HashMap::new(),
        dir.path(),
    )
    .expect_err("an unknown event is not silently dropped");
    let message = error.to_string();
    assert!(
        message.contains("postToolUsage"),
        "the message must name what was rejected: {message}"
    );
}
