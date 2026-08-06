//! Tests for what a session is opened *with*: which providers and presets the
//! picker offers, commit attribution, and the argv a real spawn produces.
//!
//! The last group asserts against a real child process rather than the argv
//! builder in isolation, because everything between the two is exactly what
//! has previously dropped a flag silently — see the doc comments on the tests
//! themselves.
//!
//! Shares its `harnesses()`/`wait_for()` fixtures with [`super::session`],
//! which covers the pty-behaviour half (screen reading, resize, mouse, key
//! input) once a session is already open.
//!
//! Unix-only, for the same reason `session` is: it drives a real child on a
//! real pseudo-terminal via `/bin/sh` or a stand-in script.

use std::collections::HashMap;

use medulla::protocol::HarnessProvider;

use crate::worker::pty::PtyManager;

use super::super::HarnessChoice;
use super::session::{harnesses, wait_for};

#[test]
fn picker_choices_include_every_native_provider_and_registered_preset() {
    let mut harnesses = harnesses(PtyManager::new());
    harnesses
        .env
        .insert("OPENHUMAN_BIN".to_string(), "/bin/sh".to_string());
    harnesses.providers = vec![
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ];
    harnesses.custom_harnesses = vec![medulla::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek Codex | codex | deepseek/deepseek-chat | | this-device",
    )
    .unwrap()];

    let choices = harnesses.choices();
    let labels = choices
        .iter()
        .map(HarnessChoice::display_name)
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        [
            "Claude Code",
            "Codex",
            "OpenCode",
            "OpenHuman",
            "DeepSeek Codex"
        ]
    );
    assert_eq!(choices[3].id(), "openhuman");
    assert_eq!(choices[3].provider, HarnessProvider::Openhuman);
    assert_eq!(choices[4].id(), "deepseek");
    assert_eq!(choices[4].provider, HarnessProvider::Codex);
}

#[test]
fn picker_hides_openhuman_when_its_binary_is_unavailable() {
    let harnesses = harnesses(PtyManager::new());

    assert!(!harnesses
        .choices()
        .iter()
        .any(|choice| choice.provider == HarnessProvider::Openhuman));
}

#[test]
fn registered_openrouter_preset_uses_the_proxy_without_leaking_its_key() {
    let mut harnesses = harnesses(PtyManager::new());
    // This test is about the router injection, and attribution adds argv and
    // environment of its own; it has its own tests below.
    harnesses.attribution = false;
    harnesses
        .env
        .insert("OPENROUTER_API_KEY".into(), "secret".into());
    harnesses.env.insert(
        medulla::inference_proxy::UPSTREAM_URL_ENV.into(),
        "http://127.0.0.1:1/api".into(),
    );
    let preset = medulla::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek Claude | claude | deepseek/deepseek-chat | deepseek/fast | this-device",
    )
    .unwrap();

    let (env, extra_args) = harnesses
        .spawn_env(&HarnessChoice::custom(preset))
        .expect("registered preset is launchable");

    let base_url = &env["ANTHROPIC_BASE_URL"];
    assert!(
        base_url.starts_with("http://127.0.0.1:") && base_url.ends_with("/anthropic"),
        "Claude must use the proxy's Anthropic mount: {base_url}"
    );
    let token = &env["ANTHROPIC_AUTH_TOKEN"];
    assert!(
        token.starts_with("mdl-"),
        "child must receive a proxy token"
    );
    assert_eq!(
        env.get(medulla::inference_proxy::PROXY_TOKEN_ENV),
        Some(token),
        "router injection must resolve the minted proxy token"
    );
    assert!(
        !env.contains_key("OPENROUTER_API_KEY"),
        "the upstream key must not survive in the child environment"
    );
    assert_ne!(
        token, "secret",
        "the upstream key must not be passed through"
    );
    assert_eq!(
        env["ANTHROPIC_DEFAULT_OPUS_MODEL"],
        "deepseek/deepseek-chat"
    );
    assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek/fast");
    assert!(extra_args.is_empty());
}

/// A harness the operator opens by hand is still one Medulla launched, and was
/// the last spawn seam that produced unattributed commits with
/// `attribution.commit = true` — the executor's own path had been fixed, this
/// one had not.
#[test]
fn an_operator_started_harness_carries_commit_attribution() {
    let harnesses = harnesses(PtyManager::new());

    let (env, extra_args) = harnesses
        .spawn_env(&HarnessChoice::native(HarnessProvider::Claude))
        .expect("a native provider is launchable");

    assert!(
        env.get("MEDULLA_ATTRIBUTION")
            .is_some_and(|v| v.contains("Co-authored-by: Medulla")),
        "the child must carry the trailer: {env:?}"
    );
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0").map(String::as_str),
        Some("core.hooksPath"),
        "the hook is the mechanism of record and must be activated: {env:?}"
    );
    assert!(
        env.contains_key("GIT_CONFIG_VALUE_0"),
        "the hook directory must be named: {env:?}"
    );
    // Claude additionally takes the advisory `--settings` hint.
    assert!(
        extra_args.windows(2).any(|w| w[0] == "--settings"),
        "claude also gets the inline settings hint: {extra_args:?}"
    );
}

#[test]
fn an_operator_started_harness_omits_attribution_when_configured_off() {
    let mut harnesses = harnesses(PtyManager::new());
    harnesses.attribution = false;

    let (env, extra_args) = harnesses
        .spawn_env(&HarnessChoice::native(HarnessProvider::Claude))
        .expect("a native provider is launchable");

    assert!(!env.contains_key("MEDULLA_ATTRIBUTION"), "{env:?}");
    assert!(!env.contains_key("GIT_CONFIG_KEY_0"), "{env:?}");
    assert!(extra_args.is_empty(), "{extra_args:?}");
}

/// Codex has no `--settings` equivalent, so its attribution is the hook alone.
/// Inventing a flag for it would be a spawn that exits on an unknown argument.
#[test]
fn a_codex_harness_is_attributed_by_hook_without_extra_argv() {
    let harnesses = harnesses(PtyManager::new());

    let (env, extra_args) = harnesses
        .spawn_env(&HarnessChoice::native(HarnessProvider::Codex))
        .expect("a native provider is launchable");

    assert!(env.contains_key("MEDULLA_ATTRIBUTION"), "{env:?}");
    assert!(extra_args.is_empty(), "{extra_args:?}");
}

/// The door an operator actually sits in. It spawns a CLI on a pty, where there
/// is no `session/new` to carry an offer of Medulla's tools — so the
/// registration has to reach the child on its argv, and for a long while it did
/// not reach it at all: the harness came up with the workflow skills installed
/// and no `workflow_run` to call.
///
/// Asserted against the *real spawn*, not the argv builder, because everything
/// between the two is exactly what used to drop it.
#[cfg(feature = "workflows")]
#[test]
fn an_operator_started_claude_is_handed_medullas_own_tools() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let argv = dir.path().join("argv");
    let bin = dir.path().join("fake-claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nsleep 30\n",
            argv.display()
        ),
    )
    .expect("the stand-in harness is writable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("the stand-in harness is executable");
    }

    let sessions = PtyManager::new();
    let mut harnesses = harnesses(sessions.clone());
    harnesses.workspace = dir.path().to_string_lossy().into_owned();
    harnesses.env.insert(
        "TINYVERSE_CLAUDE_BIN".to_string(),
        bin.to_string_lossy().into_owned(),
    );

    let id = harnesses
        .open_unmanaged(&HarnessChoice::native(HarnessProvider::Claude), "", false)
        .expect("the stand-in harness starts");
    wait_for("the child to record its argv", || argv.exists());
    let recorded = std::fs::read_to_string(&argv).expect("the argv was recorded");
    sessions.close(&id);

    let mut lines = recorded.lines();
    let Some(document) = lines
        .find(|line| *line == "--mcp-config")
        .and_then(|_| lines.next())
    else {
        panic!("claude must be registered through --mcp-config: {recorded}");
    };
    let document: serde_json::Value =
        serde_json::from_str(document).expect("the registration is a JSON document");
    assert_eq!(document["mcpServers"]["medulla"]["args"][0], "mcp");
    assert!(
        !recorded.contains(medulla::control_socket::MCP_GRANT_ENV),
        "no bearer token may appear on an argv other users can read: {recorded}"
    );
}

/// The other half of what a spawned harness needs: the tools arrive on the argv
/// above, but a session that does not know `babysit` exists never reaches for
/// them. `--scope managed` is documented as covering "the harnesses Medulla
/// spawns", and the pty doors — the Workers pane and the task frames opened on
/// this device — were not among them: only the headless executor added the
/// `--add-dir` that makes Claude load the managed skills.
///
/// Against the real spawn for the same reason the test above is.
#[cfg(all(unix, feature = "workflows"))]
#[test]
fn an_operator_started_claude_is_pointed_at_the_managed_skills() {
    use medulla::workflows::skills::{
        install, managed_dir, managed_root, InstallOptions, SkillScope, SkillTarget,
    };
    use medulla::workflows::WorkflowSummary;

    let dir = tempfile::tempdir().expect("a scratch directory");
    let argv = dir.path().join("argv");
    let bin = dir.path().join("fake-claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nsleep 30\n",
            argv.display()
        ),
    )
    .expect("the stand-in harness is writable");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .expect("the stand-in harness is executable");
    }

    // A Medulla home of our own, so the test reads its own managed skills
    // rather than whatever the developer running it has installed.
    let home = dir.path().join("medulla-home");
    let mut env = HashMap::new();
    env.insert("MEDULLA_HOME".to_string(), home.display().to_string());
    install(
        &[WorkflowSummary {
            id: "babysit".to_string(),
            name: "babysit".to_string(),
            description: "Watch a PR until it is green.".to_string(),
            enabled: true,
            node_count: 3,
            trigger_kind: Some("manual".to_string()),
            inputs: Vec::new(),
        }],
        &InstallOptions {
            targets: vec![SkillTarget::Claude],
            scope: SkillScope::Managed,
            root: managed_root(&env),
            with_commands: false,
            dry_run: false,
        },
    )
    .expect("the managed skill is installable");
    let expected = managed_dir(SkillTarget::Claude, &env).display().to_string();

    let sessions = PtyManager::new();
    let mut harnesses = harnesses(sessions.clone());
    harnesses.workspace = dir.path().to_string_lossy().into_owned();
    harnesses.env.insert(
        "TINYVERSE_CLAUDE_BIN".to_string(),
        bin.to_string_lossy().into_owned(),
    );
    harnesses
        .env
        .insert("MEDULLA_HOME".to_string(), home.display().to_string());

    let id = harnesses
        .open_unmanaged(&HarnessChoice::native(HarnessProvider::Claude), "", false)
        .expect("the stand-in harness starts");
    wait_for("the child to record its argv", || argv.exists());
    let recorded = std::fs::read_to_string(&argv).expect("the argv was recorded");
    sessions.close(&id);

    let mut lines = recorded.lines();
    let added = lines
        .find(|line| *line == "--add-dir")
        .and_then(|_| lines.next());
    assert_eq!(
        added,
        Some(expected.as_str()),
        "claude loads .claude/skills from an --add-dir directory; without the flag \
         the managed skills are invisible to the session: {recorded}"
    );
}
