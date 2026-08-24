//! Unit tests for the OpenHuman hook adapter (matcher filtering, exit-status
//! semantics, and timeout bounding). Registration itself is exercised only by
//! boot coverage — `register_embedder_tool_hook` mutates a process-global
//! singleton that cannot be torn down between tests.

use openhuman_core::openhuman::agent::hooks::{ToolHook, ToolHookContext, ToolHookEvent};

use super::hooks::*;
use super::turn_cwd::with_turn_cwd;
use crate::harness_hooks::{HookEvent, HookHandler, HookSpec};
use crate::protocol::HarnessProvider;

fn spec(command: &str) -> HookSpec {
    HookSpec {
        event: HookEvent::PreToolUse,
        matcher: "*".into(),
        handler: HookHandler::Command {
            command: command.into(),
            timeout: None,
        },
        harnesses: vec![HarnessProvider::Openhuman],
        label: None,
        builtin: false,
    }
}

fn spec_with(command: &str, matcher: &str, timeout: Option<u64>) -> HookSpec {
    let mut hook = spec(command);
    hook.matcher = matcher.into();
    hook.handler = HookHandler::Command {
        command: command.into(),
        timeout,
    };
    hook
}

fn tool_context(tool_name: &str) -> ToolHookContext {
    ToolHookContext {
        event: ToolHookEvent::PreToolUse,
        call_id: "call-1".into(),
        tool_name: tool_name.into(),
        arguments: serde_json::json!({}),
        success: None,
        duration_ms: None,
        output: None,
        error: None,
        session_id: None,
        agent_id: None,
    }
}

#[test]
fn a_selective_matcher_runs_only_for_the_tool_it_names() {
    assert!(matcher_selects("Edit", "Edit"));
    assert!(!matcher_selects("Edit", "Write"));
    assert!(!matcher_selects("Edit", "Bash"));
}

#[test]
fn a_pipe_matcher_matches_any_alternation() {
    assert!(matcher_selects("Edit|Write", "Edit"));
    assert!(matcher_selects("Edit|Write", "Write"));
    assert!(!matcher_selects("Edit|Write", "Bash"));
}

#[test]
fn the_wildcard_matcher_selects_every_tool() {
    assert!(matcher_selects("*", "Edit"));
    assert!(matcher_selects("*", "Read"));
}

#[test]
fn question_mark_matcher_selects_exactly_one_character() {
    assert!(matcher_selects("Edit?", "Edits"));
    assert!(!matcher_selects("Edit?", "Edit"));
    assert!(!matcher_selects("Edit?", "Editxx"));
}

#[test]
fn tool_payload_uses_the_native_hook_input_envelope() {
    let payload = tool_hook_payload(&tool_context("Write"));
    assert_eq!(payload["hook_event_name"], "PreToolUse");
    assert_eq!(payload["session_id"], "call-1");
    assert_eq!(payload["tool_name"], "Write");
    assert_eq!(payload["tool_input"], serde_json::json!({}));
    assert!(payload["cwd"].is_string());
}

#[tokio::test]
async fn the_payload_names_the_directory_the_turn_is_working_in() {
    // The bug this guards: a turn working in a feature worktree used to be
    // reported with the Medulla process's startup directory, so a PostToolUse
    // auto-commit hook checkpointed the wrong repository.
    let turn_dir = std::path::Path::new("/repo/worktrees/feature");
    let payload = with_turn_cwd(Some(turn_dir), async {
        tool_hook_payload(&tool_context("Write"))
    })
    .await;
    assert_eq!(payload["cwd"], "/repo/worktrees/feature");
}

#[tokio::test]
async fn two_turns_in_two_directories_each_get_their_own() {
    let first = with_turn_cwd(Some(std::path::Path::new("/repo/one")), async {
        tool_hook_payload(&tool_context("Write"))
    })
    .await;
    let second = with_turn_cwd(Some(std::path::Path::new("/repo/two")), async {
        tool_hook_payload(&tool_context("Write"))
    })
    .await;
    assert_eq!(first["cwd"], "/repo/one");
    assert_eq!(second["cwd"], "/repo/two");
}

#[tokio::test]
async fn a_turn_that_declares_no_directory_still_reports_the_process_one() {
    // The TUI's own chat and MCP calls open no scope; the process directory is
    // the honest answer for them, so the fallback must survive.
    let expected = std::env::current_dir().expect("a test process has a working directory");
    let payload = with_turn_cwd(None, async { tool_hook_payload(&tool_context("Write")) }).await;
    assert_eq!(payload["cwd"], expected.to_str().expect("utf-8 path"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_post_tool_command_is_handed_the_turns_directory_on_stdin() {
    // End to end through the command boundary: what the operator's hook script
    // actually reads out of `.cwd` is the run's directory.
    let hook = MedullaToolHook::new(
        Vec::new(),
        vec![spec_with(
            "payload=$(cat); case \"$payload\" in *'\"cwd\":\"/repo/worktrees/feature\"'*) exit 0 ;; *) exit 1 ;; esac",
            "*",
            Some(5),
        )],
    );
    with_turn_cwd(
        Some(std::path::Path::new("/repo/worktrees/feature")),
        async {
            hook.after_tool(&tool_context("Write"))
                .await
                .expect("the post-tool command is told the turn's directory")
        },
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_hook_command_starts_in_the_turns_directory() {
    // A hook script that shells out to `git` without an explicit `-C` resolves
    // its repository from where it was started, so the command's own working
    // directory has to agree with the payload's `cwd`. It used to inherit
    // Medulla's process directory instead.
    let turn = tempfile::tempdir().expect("temp dir");
    let recorded = turn.path().join("pwd.txt");
    let hook = MedullaToolHook::new(
        Vec::new(),
        vec![spec_with(
            &format!("pwd > {}", recorded.display()),
            "*",
            Some(5),
        )],
    );
    with_turn_cwd(Some(turn.path()), async {
        hook.after_tool(&tool_context("Write"))
            .await
            .expect("the post-tool command runs")
    })
    .await;
    let observed = std::fs::read_to_string(&recorded).expect("the hook recorded its directory");
    assert_eq!(
        std::fs::canonicalize(observed.trim()).expect("the recorded directory exists"),
        std::fs::canonicalize(turn.path()).expect("the turn directory exists"),
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_turn_directory_that_no_longer_exists_falls_back_to_the_process_one() {
    // A worktree named by an earlier turn can be gone by the time a hook runs.
    // Spawning into it would fail the hook outright; the process directory at
    // least runs it, which is what happened before a turn directory existed.
    let removed = tempfile::tempdir().expect("temp dir");
    let path = removed.path().to_path_buf();
    drop(removed);
    let observed = with_turn_cwd(Some(&path), async { hook_working_dir() }).await;
    assert_eq!(observed, None);
}

#[cfg(unix)]
#[tokio::test]
async fn an_openhuman_post_tool_callback_passes_native_input_to_the_command() {
    let hook = MedullaToolHook::new(
        Vec::new(),
        vec![spec_with(
            "payload=$(cat); case \"$payload\" in *'\"tool_input\":{\"file_path\":\"/repo/edited.txt\"}'*) exit 0 ;; *) exit 1 ;; esac",
            "*",
            Some(1),
        )],
    );
    let context = ToolHookContext {
        arguments: serde_json::json!({ "file_path": "/repo/edited.txt" }),
        ..tool_context("Write")
    };
    hook.after_tool(&context)
        .await
        .expect("the post-tool command receives the native hook envelope");
}

#[tokio::test]
async fn a_non_matching_matcher_skips_the_command_entirely() {
    // An "Edit"-scoped command that always fails must not run for a "Bash" tool:
    // skipping it is what honouring the matcher means.
    let hooks = [spec_with("exit 1", "Edit", None)];
    run_hook_commands(&hooks, &tool_context("Bash"), false, &Default::default())
        .await
        .expect("matcher must skip the command");
}

#[tokio::test]
async fn a_matching_pre_hook_passes_and_a_failing_one_vetoes() {
    run_command(&spec("exit 0"), b"{}", true, &Default::default())
        .await
        .expect("a successful pre-hook passes");
    let err = run_command(&spec("exit 3"), b"{}", true, &Default::default())
        .await
        .expect_err("a failing pre-hook vetoes the tool call");
    assert!(err.to_string().contains("exited with"));
}

#[tokio::test]
async fn a_stop_hook_that_fails_is_observational() {
    // Stop hooks observe: a non-zero exit must not fail the turn.
    run_command(&spec("exit 9"), b"{}", false, &Default::default())
        .await
        .expect("stop hook failure is ignored");
}

#[tokio::test]
async fn a_command_that_exceeds_its_timeout_is_killed_not_left_to_hang() {
    let started = std::time::Instant::now();
    let command = if cfg!(windows) {
        "ping 127.0.0.1 -n 6 > NUL"
    } else {
        "sleep 5"
    };
    let err = run_command(
        &spec_with(command, "*", Some(1)),
        b"{}",
        true,
        &Default::default(),
    )
    .await
    .expect_err("a timed-out pre-hook vetoes");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "the command must be killed rather than left to sleep out"
    );
    assert!(err.to_string().contains("timed out"));
}

#[tokio::test]
async fn a_timed_out_stop_hook_is_killed_without_failing_the_turn() {
    let command = if cfg!(windows) {
        "ping 127.0.0.1 -n 6 > NUL"
    } else {
        "sleep 5"
    };
    let started = std::time::Instant::now();
    run_command(
        &spec_with(command, "*", Some(1)),
        b"{}",
        false,
        &Default::default(),
    )
    .await
    .expect("a timed-out stop hook is observational");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "the stop hook must return after its timeout, not command completion"
    );
}

/// One byte on top of the pipe buffer: commands that close stdin deterministically
/// hit a broken-pipe write with a payload this big, whichever side of the race
/// the write lands on.
const STDIN_RACE_PAYLOAD: &[u8] = &[b'x'; 1 << 20];

#[cfg(unix)]
#[tokio::test]
async fn an_enforced_hook_whose_stdin_write_fails_is_still_observational() {
    // The command closes its stdin immediately and stays alive briefly, so the
    // payload cannot fully arrive — the same race a hook that simply never
    // drains its stdin (`git add -A && git commit`) hits. Surfacing that write
    // failure as a veto made a `PreToolUse` hook veto the tool call it had just
    // approved with `exit 0`, so only the exit status decides now, for enforced
    // hooks exactly as for observational ones.
    run_command(
        &spec("exec 0<&-; sleep 0.2"),
        STDIN_RACE_PAYLOAD,
        true,
        &Default::default(),
    )
    .await
    .expect("a stdin write failure alone must not veto an enforced hook");
}

#[cfg(unix)]
#[tokio::test]
async fn a_stop_hook_whose_stdin_write_fails_is_observational() {
    // The race this guards: a stop hook may close its stdin — or exit — before
    // the payload is written. The hook observes, so that write failure must not
    // fail the turn; swallowing it is exactly the non-enforced path's job.
    run_command(
        &spec("exec 0<&-; sleep 0.2"),
        STDIN_RACE_PAYLOAD,
        false,
        &Default::default(),
    )
    .await
    .expect("a stop hook whose stdin write fails is observational");
}
