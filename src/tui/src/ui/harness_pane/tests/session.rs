//! Tests for the session-facing half of [`LocalSessions`], against a real
//! child on a real pseudo-terminal.
//!
//! `/bin/sh` stands in for a coding agent: it is a genuine pty client with a
//! genuine terminal, so reads, resizes, writes and exit detection are exercised
//! exactly as they will be against `claude`, while staying fast, offline, and
//! deterministic.
//!
//! Unix-only, for the same reason the pty layer's own tests are: Windows has no
//! `/bin/sh` to drive.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use medulla::protocol::HarnessProvider;

use crate::worker::pty::{LaunchSpec, PtyManager, SessionControl};

use super::super::HarnessChoice;
use super::super::LocalSessions;

/// A spec that runs `sh -c <script>` on a pty.
///
/// Codex rather than Claude: Claude's interactive argv carries a minted
/// `--session-id`, which `/bin/sh` would reject as an unknown option. Codex
/// takes no preset id, so its argv is empty and the script is the whole command.
fn sh(script: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        provider: HarnessProvider::Codex,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: "test".to_string(),
        session_id: None,
        model: None,
        control: SessionControl::Orchestrator,
        origin: crate::worker::pty::SessionOrigin::Orchestrator,
        name: None,
        mcp_grant_session: None,
    }
}

/// A [`LocalSessions`] over `sessions`, with a runtime that serves no tasks.
///
/// Task resolution needs a live host and is covered by the daemon's own screen
/// e2e; everything here is about what the pane does *once* a session is named,
/// so the runtime is inert on purpose.
pub(super) fn harnesses(sessions: PtyManager) -> LocalSessions {
    let config = medulla::daemon::DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: "/".to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        task_timeout_ms: 1_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 1_000,
        max_pending: 1,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: None,
        custom_harnesses: Vec::new(),
        budget: None,
        attribution: true,
    };
    let run_task: medulla::daemon::providers::RunTaskFn =
        std::sync::Arc::new(|_| Box::pin(async { Err("not used in these tests".to_string()) }));
    let send: medulla::daemon::SendFn = std::sync::Arc::new(|_, _| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });
    LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions,
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(config, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    }
}

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

/// Spin until `check` passes or the deadline expires.
///
/// The budget is far larger than these conditions actually need: real children
/// on real ptys are at the mercy of machine load, and a tight deadline turns
/// "the box was busy" into a red test.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out after 30s waiting for: {what}");
}

/// The whole screen as one string.
fn text(harnesses: &LocalSessions, id: &str) -> String {
    harnesses
        .screen(id)
        .expect("the session has a screen")
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_running_harness_screen_is_readable_from_the_pane() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("echo pane-sees-this; sleep 30")).unwrap();

    wait_for("the child's output to reach the emulator", || {
        text(&harnesses, &id).contains("pane-sees-this")
    });
    assert!(harnesses.is_running(&id));
    sessions.close(&id);
}

#[test]
fn fitting_the_pane_reflows_the_child_to_the_pane_size() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // `tput cols` asks the terminal how wide it is, so the child's own answer
    // proves the resize reached the pty and not only the emulator.
    let id = sessions.open(sh("sleep 0.4; tput cols; sleep 30")).unwrap();

    harnesses.fit(&id, 100, 24);
    wait_for("the child to report the pane's width", || {
        text(&harnesses, &id).contains("100")
    });

    let snapshot = harnesses.screen(&id).expect("a screen");
    assert_eq!(snapshot.cells.len(), 24, "the emulator moved with the pty");
    assert_eq!(snapshot.cells[0].len(), 100);
    sessions.close(&id);
}

#[test]
fn a_zero_sized_pane_is_ignored_rather_than_collapsing_the_child() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("sleep 30")).unwrap();

    harnesses.fit(&id, 90, 20);
    // A pane can momentarily measure zero mid-layout; resizing a pty to it
    // would tell the harness it has no screen at all.
    harnesses.fit(&id, 0, 0);

    let snapshot = harnesses.screen(&id).expect("a screen");
    assert_eq!(snapshot.cells.len(), 20);
    assert_eq!(snapshot.cells[0].len(), 90);
    sessions.close(&id);
}

#[test]
fn typing_reaches_the_child_as_the_bytes_a_terminal_would_have_sent() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions
        .open(sh("read line; echo got:$line; sleep 30"))
        .unwrap();

    wait_for("the shell to be reading", || harnesses.is_running(&id));
    // Carriage return, not newline: this is what the encoder emits for Enter,
    // and it is what a raw-mode reader accepts as end-of-line.
    harnesses
        .write(&id, b"hello\r")
        .expect("the pty accepts input");

    wait_for("the child to echo what was typed", || {
        text(&harnesses, &id).contains("got:hello")
    });
    sessions.close(&id);
}

#[test]
fn an_exited_harness_stops_being_attachable_but_keeps_its_last_screen() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("echo last-words")).unwrap();

    wait_for("the child to exit", || !harnesses.is_running(&id));
    // The screen survives the child: an operator usually wants to read how it
    // ended, and a pane that blanks on exit throws that away.
    assert!(text(&harnesses, &id).contains("last-words"));
    // Writing to it fails rather than silently vanishing, which is what tells
    // the attached pane to release the keyboard.
    assert!(harnesses.write(&id, b"x").is_err());
}

#[test]
fn an_unknown_session_resolves_to_nothing_rather_than_panicking() {
    let harnesses = harnesses(PtyManager::new());
    assert!(harnesses.screen("w_nope").is_none());
    assert!(!harnesses.is_running("w_nope"));
    assert!(harnesses.write("w_nope", b"x").is_err());
    // A resize against a session that is gone is a no-op, not a crash: the
    // render pass fits every frame and the child can exit between frames.
    harnesses.fit("w_nope", 80, 24);
}

#[test]
fn a_child_that_asks_for_the_mouse_has_wheel_notches_forwarded_to_it() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // Turn on SGR mouse reporting (1000 = report presses, 1006 = SGR encoding)
    // the way a real harness does, then echo whatever arrives on stdin.
    let id = sessions
        .open(sh(
            "printf '\\033[?1000h\\033[?1006h'; sleep 0.3; cat -v; sleep 30",
        ))
        .unwrap();

    wait_for("the child to enable mouse reporting", || {
        matches!(
            harnesses.sessions.mouse_protocol(&id),
            Some((mode, _)) if mode != vt100::MouseProtocolMode::None
        )
    });

    harnesses.scroll(&id, 3, 4, true, 3);

    // `cat -v` prints control bytes visibly, so the report the child received is
    // readable straight off its own screen. 1-based: pane (3,4) is 4;5.
    wait_for("the child to receive the wheel report", || {
        text(&harnesses, &id).contains("[<64;4;5M")
    });
    sessions.close(&id);
}

#[test]
fn alternate_scroll_without_mouse_reporting_gets_arrow_scroll_events() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // Codex uses the terminal's alternate-scroll behaviour instead of mouse
    // reporting: while mode 1007 is active, a terminal translates each wheel
    // notch into cursor-key input for the child.
    let id = sessions
        .open(sh(
            "printf '\\033[?1049h\\033[?1007h'; sleep 0.3; cat -v; sleep 30",
        ))
        .unwrap();

    wait_for("the child to enable alternate scrolling", || {
        sessions
            .alternate_scroll(&id)
            .is_some_and(std::convert::identity)
    });

    harnesses.scroll(&id, 3, 4, true, 3);

    wait_for("the child to receive translated wheel input", || {
        text(&harnesses, &id).contains("^[[A^[[A^[[A")
    });
    harnesses.scroll(&id, 3, 4, false, 2);
    wait_for("the child to receive downward wheel input", || {
        text(&harnesses, &id).contains("^[[B^[[B")
    });
    sessions.close(&id);
}

#[test]
fn alternate_screen_without_alternate_scroll_does_not_receive_arrows() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions
        .open(sh(
            "printf '\\033[?1049h\\033[?1007lready'; sleep 0.3; cat -v; sleep 30",
        ))
        .unwrap();

    wait_for("the child to enter its alternate screen", || {
        text(&harnesses, &id).contains("ready")
    });
    assert_eq!(sessions.alternate_scroll(&id), Some(false));

    harnesses.scroll(&id, 3, 4, true, 3);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        !text(&harnesses, &id).contains("^[[A"),
        "mode 1049 alone must not synthesize cursor input"
    );
    sessions.close(&id);
}

#[test]
fn a_child_that_never_asked_for_the_mouse_gets_our_scrollback_instead() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    // Enough lines to push history off a 30-row screen, and no mouse reporting.
    let id = sessions
        .open(sh(
            "i=1; while [ $i -le 200 ]; do echo line-$i; i=$((i+1)); done; sleep 30",
        ))
        .unwrap();

    wait_for("the child to scroll its output off the screen", || {
        text(&harnesses, &id).contains("line-200")
    });
    assert!(
        matches!(
            harnesses.sessions.mouse_protocol(&id),
            Some((vt100::MouseProtocolMode::None, _))
        ),
        "a plain shell asks for no mouse reporting"
    );

    // Nothing is written to the child — the wheel moves our own emulator. The
    // 40 exceeds both the screen height and what vt100 can safely offset by; it
    // must clamp rather than panic inside the crate's `visible_rows`.
    harnesses.scroll(&id, 0, 0, true, 40);
    let scrolled = text(&harnesses, &id);
    assert!(
        !scrolled.contains("line-200"),
        "the view should have moved back into history:\n{scrolled}"
    );

    // And typing snaps it back, so a harness answering below is not missed.
    harnesses.scroll_to_live(&id);
    assert!(text(&harnesses, &id).contains("line-200"));
    sessions.close(&id);
}

#[test]
fn scrolling_down_at_the_live_edge_stays_put_rather_than_underflowing() {
    let sessions = PtyManager::new();
    let harnesses = harnesses(sessions.clone());
    let id = sessions.open(sh("echo hello; sleep 30")).unwrap();

    wait_for("the child to paint", || {
        text(&harnesses, &id).contains("hello")
    });
    // Already at the bottom; scrolling further down must not wrap into a huge
    // offset and blank the pane.
    harnesses.scroll(&id, 0, 0, false, 500);
    assert!(text(&harnesses, &id).contains("hello"));
    sessions.close(&id);
}
