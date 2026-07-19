//! Unit tests for history sharing.
//!
//! The redaction tests carry the most weight: they pin both halves of the
//! contract — secrets must not survive, and scoring metadata must.

use std::collections::HashMap;

use super::redact::{redact_text, REDACTED};
use super::scan::{read_redacted_session, scan_local_history, MAX_SESSION_BYTES};
use super::types::{HistoryScan, HistorySessionFile};
use crate::session_history::SessionAgentKind;

fn env_with_dirs(claude: &str, codex: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TINYPLACE_CLAUDE_SESSIONS_DIR".into(), claude.into());
    env.insert("TINYPLACE_CODEX_SESSIONS_DIR".into(), codex.into());
    env
}

#[test]
fn redacts_openai_style_keys() {
    let (out, count) = redact_text(r#"{"text":"use sk-abcdefghijklmnop0123456789 now"}"#);
    assert!(!out.contains("sk-abcdefghijklmnop"), "key survived: {out}");
    assert!(out.contains(REDACTED));
    assert_eq!(count, 1);
}

#[test]
fn redacts_aws_github_slack_and_google_keys() {
    let secrets = [
        "AKIAIOSFODNN7EXAMPLE",
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        "xoxb-1234567890-abcdefghijkl",
        "AIzaSyA1234567890abcdefghijklmnopqrstuv",
    ];
    for secret in secrets {
        let (out, count) = redact_text(&format!(r#"{{"text":"{secret}"}}"#));
        assert!(!out.contains(secret), "{secret} survived: {out}");
        assert_eq!(count, 1, "expected one redaction for {secret}");
    }
}

#[test]
fn redacts_jwts_and_bearer_tokens_but_keeps_the_scheme() {
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let (out, count) = redact_text(&format!(r#"{{"h":"Authorization: Bearer {jwt}"}}"#));
    assert!(!out.contains("eyJhbGciOiJIUzI1NiJ9"), "jwt survived: {out}");
    assert!(out.contains("Bearer "), "scheme should survive: {out}");
    assert!(count >= 1);
}

#[test]
fn redacts_pem_private_key_blocks() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\\nMIIEowIBAAKCAQEA\\n-----END RSA PRIVATE KEY-----";
    let (out, count) = redact_text(&format!(r#"{{"key":"{pem}"}}"#));
    assert!(
        !out.contains("MIIEowIBAAKCAQEA"),
        "key body survived: {out}"
    );
    assert_eq!(count, 1);
}

#[test]
fn redacts_secret_looking_assignments() {
    let cases = [
        r#"{"t":"api_key=supersecretvalue123"}"#,
        r#"{"t":"PASSWORD: hunter2hunter2"}"#,
        r#"{"t":"ACCESS_TOKEN=abcdefgh12345678"}"#,
        r#"{"t":"aws_secret_access_key=wJalrXUtnFEMIK7MDENGbPxRfiCY"}"#,
    ];
    for case in cases {
        let (out, count) = redact_text(case);
        assert!(out.contains(REDACTED), "no redaction in {case} -> {out}");
        assert_eq!(count, 1, "expected one redaction for {case}");
    }
}

#[test]
fn preserves_token_counters_so_scoring_still_works() {
    // The single most important invariant: redaction must not touch usage
    // numbers, or the user is under-credited for real work.
    let line = r#"{"timestamp":"2026-01-05T10:00:00Z","message":{"usage":{"input_tokens":100000,"output_tokens":50000,"cache_read_input_tokens":250,"total_tokens":150250}}}"#;
    let (out, count) = redact_text(line);

    assert_eq!(out, line, "usage metadata must pass through untouched");
    assert_eq!(count, 0);

    let parsed: serde_json::Value = serde_json::from_str(&out).expect("must stay valid JSON");
    assert_eq!(parsed["message"]["usage"]["input_tokens"], 100_000);
    assert_eq!(parsed["message"]["usage"]["total_tokens"], 150_250);
}

#[test]
fn preserves_timestamps_roles_and_tool_names() {
    let line = r#"{"timestamp":"2026-01-05T10:00:00Z","role":"assistant","tool_name":"Bash","type":"tool_use"}"#;
    let (out, count) = redact_text(line);

    assert_eq!(out, line);
    assert_eq!(count, 0);
}

#[test]
fn leaves_git_shas_alone() {
    // A 40-char hex string is a git SHA, not a secret; transcripts are full of
    // them and a generic entropy rule would shred every one.
    let line = r#"{"text":"commit cb4bdc4e1f2a3b4c5d6e7f8091a2b3c4d5e6f708 landed"}"#;
    let (out, count) = redact_text(line);

    assert_eq!(out, line);
    assert_eq!(count, 0);
}

#[test]
fn redacted_output_remains_parseable_jsonl() {
    let line = r#"{"timestamp":"2026-01-05T10:00:00Z","text":"my key is sk-abcdefghijklmnop0123456789","message":{"usage":{"input_tokens":10}}}"#;
    let (out, _) = redact_text(line);

    let parsed: serde_json::Value = serde_json::from_str(&out).expect("must stay valid JSON");
    assert_eq!(parsed["message"]["usage"]["input_tokens"], 10);
    assert!(parsed["text"].as_str().unwrap().contains(REDACTED));
}

#[test]
fn counts_multiple_secrets_in_one_transcript() {
    let text = format!(
        "{}\n{}",
        r#"{"t":"sk-aaaaaaaaaaaaaaaaaaaaaaaa"}"#, r#"{"t":"AKIAIOSFODNN7EXAMPLE"}"#
    );
    let (out, count) = redact_text(&text);

    assert_eq!(count, 2);
    assert!(!out.contains("sk-aaaa"));
    assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
}

#[test]
fn redacting_clean_text_is_a_no_op() {
    let text = r#"{"role":"user","content":"please refactor the parser"}"#;
    let (out, count) = redact_text(text);

    assert_eq!(out, text);
    assert_eq!(count, 0);
}

#[test]
fn scan_finds_transcripts_from_both_agents_newest_first() {
    let claude_dir = tempfile::tempdir().expect("tempdir");
    let codex_dir = tempfile::tempdir().expect("tempdir");

    std::fs::write(claude_dir.path().join("a.jsonl"), r#"{"a":1}"#).expect("write");
    std::fs::write(
        codex_dir.path().join("rollout-2026-01-01.jsonl"),
        r#"{"b":2}"#,
    )
    .expect("write");

    let scan = scan_local_history(&env_with_dirs(
        claude_dir.path().to_str().unwrap(),
        codex_dir.path().to_str().unwrap(),
    ));

    assert_eq!(scan.session_count(), 2);
    assert!(scan.total_bytes() > 0);
    assert!(!scan.is_empty());

    let tallies = scan.tallies();
    assert_eq!(tallies.len(), 2);
    assert!(tallies.iter().all(|tally| tally.session_count == 1));

    // Newest-first ordering.
    let mtimes: Vec<i64> = scan.files.iter().map(|file| file.mtime_ms).collect();
    let mut sorted = mtimes.clone();
    sorted.sort_by_key(|value| std::cmp::Reverse(*value));
    assert_eq!(mtimes, sorted);
}

#[test]
fn scan_skips_oversize_transcripts() {
    let claude_dir = tempfile::tempdir().expect("tempdir");
    let codex_dir = tempfile::tempdir().expect("tempdir");

    let big = vec![b'x'; (MAX_SESSION_BYTES + 1) as usize];
    std::fs::write(claude_dir.path().join("big.jsonl"), big).expect("write");
    std::fs::write(claude_dir.path().join("small.jsonl"), r#"{"a":1}"#).expect("write");

    let scan = scan_local_history(&env_with_dirs(
        claude_dir.path().to_str().unwrap(),
        codex_dir.path().to_str().unwrap(),
    ));

    assert_eq!(scan.session_count(), 1);
    assert_eq!(scan.skipped_oversize, 1);
}

#[test]
fn scan_of_missing_directories_is_empty_not_an_error() {
    let scan = scan_local_history(&env_with_dirs(
        "/nonexistent/claude/dir",
        "/nonexistent/codex/dir",
    ));

    assert!(scan.is_empty());
    assert_eq!(scan.session_count(), 0);
    assert_eq!(scan.total_bytes(), 0);
    assert!(scan.tallies().is_empty());
}

#[test]
fn reading_a_session_redacts_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, r#"{"t":"sk-abcdefghijklmnop0123456789"}"#).expect("write");

    let file = HistorySessionFile {
        agent: SessionAgentKind::Claude,
        path: path.clone(),
        size_bytes: 40,
        mtime_ms: 0,
    };

    let session = read_redacted_session(&file).expect("session should read");
    assert_eq!(session.agent, SessionAgentKind::Claude);
    assert_eq!(session.redactions, 1);
    assert!(session.content.contains(REDACTED));
    assert!(!session.content.contains("sk-abcdefghijklmnop"));
}

#[test]
fn reading_a_missing_session_yields_none() {
    let file = HistorySessionFile {
        agent: SessionAgentKind::Claude,
        path: "/nonexistent/session.jsonl".into(),
        size_bytes: 1,
        mtime_ms: 0,
    };

    assert!(read_redacted_session(&file).is_none());
}

#[test]
fn empty_scan_reports_zeroes() {
    let scan = HistoryScan::default();

    assert!(scan.is_empty());
    assert_eq!(scan.session_count(), 0);
    assert_eq!(scan.total_bytes(), 0);
}

// --- Numeric secrets and bare token keys (review findings) ------------------

#[test]
fn redacts_numeric_secrets() {
    // Judging by value shape would leak exactly these: a numeric password or
    // API key is all-digits but still a secret. (Keys outside the alternation,
    // like `PIN`, are out of scope — the consent screen promises API keys,
    // tokens, and passwords, and the alternation is kept narrow so it cannot
    // match innocuous keys such as `pinned_version`.)
    for case in [
        r#"{"t":"password: 12345678"}"#,
        r#"{"t":"api_key=1234567890123456"}"#,
        r#"{"secret":"0123456789012345"}"#,
    ] {
        let (out, count) = redact_text(case);
        assert!(out.contains(REDACTED), "not redacted: {case} -> {out}");
        assert_eq!(count, 1, "expected one redaction for {case}");
    }
}

#[test]
fn redacts_a_bare_token_assignment() {
    // The commonest way a credential lands in a transcript, and the consent
    // screen promises tokens are stripped.
    let (out, count) = redact_text(r#"{"token":"abcdefghijklmnopqrstuvwxyz"}"#);

    assert!(
        !out.contains("abcdefghijklmnopqrstuvwxyz"),
        "token survived: {out}"
    );
    assert!(out.contains(REDACTED));
    assert_eq!(count, 1);
}

#[test]
fn the_plural_tokens_counters_are_never_redacted() {
    // The precise carve-out that lets bare `token` be redacted safely: every
    // scorer counter is plural, every credential key is singular.
    let counters = [
        r#"{"message":{"usage":{"input_tokens":21000}}}"#,
        r#"{"message":{"usage":{"output_tokens":12345678}}}"#,
        r#"{"message":{"usage":{"total_tokens":22100}}}"#,
        r#"{"message":{"usage":{"cache_read_input_tokens":60000}}}"#,
        r#"{"info":{"total_token_usage":{"input_tokens":9000,"total_tokens":9400}}}"#,
    ];
    for case in counters {
        let (out, count) = redact_text(case);
        assert_eq!(out, case, "counter was altered: {case} -> {out}");
        assert_eq!(count, 0, "counter was redacted: {case}");
    }
}

#[test]
fn singular_token_keys_are_treated_as_secrets() {
    for case in [
        r#"{"access_token":"abcdefghijklmnop"}"#,
        r#"{"refresh_token":"abcdefghijklmnop"}"#,
        r#"{"api_token":"abcdefghijklmnop"}"#,
    ] {
        let (out, count) = redact_text(case);
        assert!(out.contains(REDACTED), "not redacted: {case} -> {out}");
        assert_eq!(count, 1);
    }
}
