//! Tests for credential status rows without reading or mutating process secrets.

use super::credential_line;

#[test]
fn credential_rows_distinguish_connected_and_missing_sources() {
    let connected = credential_line("Codex", true, "unused");
    let missing = credential_line("OpenAI", false, "set the environment variable");

    let connected_text = connected
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let missing_text = missing
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(connected_text.contains("● Codex"));
    assert!(connected_text.contains("connected"));
    assert!(missing_text.contains("○ OpenAI"));
    assert!(missing_text.contains("set the environment variable"));
}
