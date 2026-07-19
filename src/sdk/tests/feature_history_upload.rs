//! Feature tests for history sharing against realistic transcripts.
//!
//! The unit tests in `history_upload::tests` pin redaction with focused
//! one-liners. This suite runs the same code over fixtures shaped like what
//! Claude Code and Codex actually write to disk — the *same two files* the
//! backend's scoring spec uses — so both halves of the pipeline are pinned to
//! identical input.
//!
//! The load-bearing assertion is the last one: after redaction, every token
//! counter and timestamp the backend scores on must still be byte-identical.
//! If redaction ever eats those, users get silently under-credited.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use medulla::history_upload::{read_redacted_session, redact_text, scan_local_history, REDACTED};
use medulla::session_history::SessionAgentKind;

/// Absolute path to a checked-in fixture transcript.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/history")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture should exist")
}

/// Lay the fixtures out the way each agent really stores them, and return an env
/// pointing the scanner at those directories.
///
/// Codex only recognises `rollout-*.jsonl`, so the fixture is renamed on the way
/// in — the scanner's own filter is part of what this exercises.
fn staged_history(dir: &Path) -> HashMap<String, String> {
    let claude_dir = dir.join("claude/projects/-Users-dev-work-parser");
    let codex_dir = dir.join("codex/sessions/2026/01/08");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&codex_dir).unwrap();

    std::fs::copy(
        fixture("claude-session.jsonl"),
        claude_dir.join("9f3c1e70-2a4b-4c8d-9e1f-77a0b3c5d611.jsonl"),
    )
    .unwrap();
    std::fs::copy(
        fixture("codex-rollout.jsonl"),
        codex_dir.join("rollout-2026-01-08T14-00-00.jsonl"),
    )
    .unwrap();

    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_CLAUDE_SESSIONS_DIR".into(),
        dir.join("claude/projects").to_string_lossy().into_owned(),
    );
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".into(),
        dir.join("codex/sessions").to_string_lossy().into_owned(),
    );
    env
}

#[test]
fn scans_both_agents_from_their_real_directory_layouts() {
    let dir = tempfile::tempdir().unwrap();
    let scan = scan_local_history(&staged_history(dir.path()));

    assert_eq!(scan.session_count(), 2, "one transcript per agent");
    assert_eq!(scan.skipped_oversize, 0);
    assert_eq!(scan.skipped_over_cap, 0);
    assert!(scan.total_bytes() > 0);

    let tallies = scan.tallies();
    assert_eq!(tallies.len(), 2);
    assert_eq!(tallies[0].agent, SessionAgentKind::Claude);
    assert_eq!(tallies[0].session_count, 1);
    assert_eq!(tallies[1].agent, SessionAgentKind::Codex);
    assert_eq!(tallies[1].session_count, 1);
}

#[test]
fn ignores_files_that_are_not_session_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let env = staged_history(dir.path());

    // Codex only counts `rollout-*.jsonl`; Claude skips anything under subagents/.
    let codex_dir = dir.path().join("codex/sessions/2026/01/08");
    std::fs::write(codex_dir.join("notes.jsonl"), "{}").unwrap();
    std::fs::write(codex_dir.join("rollout-x.txt"), "{}").unwrap();
    let subagents = dir
        .path()
        .join("claude/projects/-Users-dev-work-parser/subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(subagents.join("sub.jsonl"), "{}").unwrap();

    assert_eq!(scan_local_history(&env).session_count(), 2);
}

#[test]
fn redacts_every_secret_pasted_into_a_real_claude_transcript() {
    let raw = read_fixture("claude-session.jsonl");
    // Sanity: the fixture really does carry the secrets we expect to strip.
    assert!(raw.contains("sk-ant-api03-"));
    assert!(raw.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(raw.contains("ghp_"));

    let (out, count) = redact_text(&raw);

    assert!(!out.contains("sk-ant-api03-"), "anthropic key survived");
    assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "aws key survived");
    assert!(!out.contains("ghp_EXAMPLE"), "github token survived");
    assert!(!out.contains("hunter2hunter2"), "password survived");
    assert!(count >= 4, "expected at least four redactions, got {count}");
    assert!(out.contains(REDACTED));
}

#[test]
fn redacts_a_bearer_token_from_a_real_codex_rollout() {
    let raw = read_fixture("codex-rollout.jsonl");
    assert!(raw.contains("Bearer abcdefghijklmnopqrstuvwxyz012345"));

    let (out, count) = redact_text(&raw);

    assert!(!out.contains("abcdefghijklmnopqrstuvwxyz012345"));
    assert!(out.contains("Bearer "), "the scheme should survive");
    assert_eq!(count, 1);
}

#[test]
fn redacted_transcripts_remain_valid_jsonl() {
    for name in ["claude-session.jsonl", "codex-rollout.jsonl"] {
        let (out, _) = redact_text(&read_fixture(name));
        let lines: Vec<&str> = out.lines().filter(|line| !line.trim().is_empty()).collect();
        assert!(!lines.is_empty(), "{name} produced no lines");
        for line in lines {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("{name} line is not valid JSON after redaction: {e}"));
        }
    }
}

#[test]
fn redaction_preserves_every_number_the_backend_scores_on() {
    // These are exactly the figures the backend's fixture spec asserts:
    // 187,126 Claude tokens over 3 days, and a 22,100 Codex running total over 2.
    let claude_counters = [
        "\"input_tokens\":8",
        "\"cache_creation_input_tokens\":60000",
        "\"output_tokens\":900",
        "\"cache_read_input_tokens\":60000",
        "\"output_tokens\":1500",
        "\"cache_read_input_tokens\":62000",
        "\"output_tokens\":700",
    ];
    let (claude, _) = redact_text(&read_fixture("claude-session.jsonl"));
    for counter in claude_counters {
        assert!(claude.contains(counter), "lost {counter} from claude");
    }
    for day in [
        "2026-01-05T09:00:00",
        "2026-01-06T11:00:00",
        "2026-01-07T08:30:00",
    ] {
        assert!(claude.contains(day), "lost timestamp {day}");
    }

    let (codex, _) = redact_text(&read_fixture("codex-rollout.jsonl"));
    for counter in ["\"total_tokens\":9400", "\"total_tokens\":22100"] {
        assert!(codex.contains(counter), "lost {counter} from codex");
    }
    for day in ["2026-01-08T14:00:00", "2026-01-09T09:15:00"] {
        assert!(codex.contains(day), "lost timestamp {day}");
    }
}

#[test]
fn redaction_leaves_ordinary_transcript_prose_intact() {
    let (out, _) = redact_text(&read_fixture("claude-session.jsonl"));

    assert!(out.contains("refactor the tokenizer to stream input"));
    assert!(out.contains("I'll stream it in chunks."));
    assert!(out.contains("cargo test"), "tool input preserved");
    assert!(out.contains("claude-opus-4-8"), "model name preserved");
    assert!(
        out.contains("cb4bdc4e1f2a3b4c5d6e7f8091a2b3c4d5e6f708"),
        "a git SHA is not a secret"
    );
}

#[test]
fn reading_a_staged_session_yields_a_redacted_upload_payload() {
    let dir = tempfile::tempdir().unwrap();
    let scan = scan_local_history(&staged_history(dir.path()));

    let mut total_redactions = 0usize;
    let mut agents_seen = Vec::new();
    for file in &scan.files {
        let session = read_redacted_session(file).expect("staged session should read");
        assert_eq!(session.agent, file.agent);
        assert!(!session.content.is_empty());
        total_redactions += session.redactions;
        agents_seen.push(session.agent);

        // Whatever the agent, nothing secret may leave.
        assert!(!session.content.contains("sk-ant-api03-"));
        assert!(!session.content.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!session.content.contains("abcdefghijklmnopqrstuvwxyz012345"));
    }

    agents_seen.sort_by_key(|agent| agent.as_str());
    assert_eq!(
        agents_seen,
        vec![SessionAgentKind::Claude, SessionAgentKind::Codex]
    );
    assert!(
        total_redactions >= 5,
        "expected secrets scrubbed across both transcripts, got {total_redactions}"
    );
}

#[test]
fn the_agent_label_matches_what_the_upload_endpoint_expects() {
    // The `agent` multipart field is validated server-side against this enum.
    assert_eq!(SessionAgentKind::Claude.as_str(), "claude");
    assert_eq!(SessionAgentKind::Codex.as_str(), "codex");
}
