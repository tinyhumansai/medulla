//! The `before_send` privacy filter every crash report passes through.
//!
//! Crash reports leave the machine, so this is an allowlist in spirit: the
//! event keeps what identifies the *failure* (exception type, stack shape,
//! release, OS) and loses what identifies the *person* — hostname, request
//! data, breadcrumbs, free-form extras, local variables, source context, every
//! user field but the account id, and the operator's home directory wherever it
//! appears in a message or frame path. Bearer tokens and JWTs that end up in a
//! panic message are redacted as a last line of defence.

use std::sync::OnceLock;

use regex::Regex;
use sentry::protocol::{Event, Frame, Stacktrace, User};

/// Longest free-text value kept from a message or exception. A panic message
/// can embed arbitrary data (an `unwrap` on an error that quotes its input), so
/// capping it bounds how much of that can ever leave the machine.
const MAX_TEXT_LEN: usize = 1024;

/// Strip everything user-identifying from `event`.
///
/// `home` is the operator's home directory, replaced by `~` wherever it
/// appears; other users' directories are masked by pattern regardless.
pub(super) fn scrub_event(mut event: Event<'static>, home: Option<&str>) -> Event<'static> {
    event.server_name = None;
    event.request = None;
    event.breadcrumbs = Default::default();
    event.extra.clear();
    event.modules.clear();
    // `tags` and `contexts` are extensible metadata containers a caller or
    // integration can populate with arbitrary data (a hostname, an email, a
    // request id); they carry nothing this filter allowlists, so they are
    // dropped rather than passed through.
    event.tags.clear();
    event.contexts.clear();
    event.user = event.user.and_then(|user| user.id).map(|id| User {
        id: Some(id),
        ..Default::default()
    });

    let text = |value: &mut Option<String>| {
        if let Some(value) = value.as_mut() {
            *value = scrub_text(value, home);
        }
    };
    text(&mut event.message);
    text(&mut event.culprit);
    text(&mut event.transaction);
    if let Some(entry) = event.logentry.as_mut() {
        entry.message = scrub_text(&entry.message, home);
        // Format parameters are exactly where caller data goes; the template
        // alone still says what went wrong.
        entry.params.clear();
    }
    for exception in &mut event.exception.values {
        text(&mut exception.value);
        scrub_stacktrace(exception.stacktrace.as_mut(), home);
        scrub_stacktrace(exception.raw_stacktrace.as_mut(), home);
    }
    scrub_stacktrace(event.stacktrace.as_mut(), home);
    for thread in &mut event.threads.values {
        scrub_stacktrace(thread.stacktrace.as_mut(), home);
        scrub_stacktrace(thread.raw_stacktrace.as_mut(), home);
    }
    event
}

/// Scrub every frame of an optional stack trace in place.
fn scrub_stacktrace(stacktrace: Option<&mut Stacktrace>, home: Option<&str>) {
    let Some(stacktrace) = stacktrace else {
        return;
    };
    for frame in &mut stacktrace.frames {
        scrub_frame(frame, home);
    }
}

/// Mask paths in a frame and drop anything that could carry runtime data.
fn scrub_frame(frame: &mut Frame, home: Option<&str>) {
    for path in [&mut frame.filename, &mut frame.abs_path, &mut frame.package] {
        if let Some(value) = path.as_mut() {
            *value = scrub_paths(value, home);
        }
    }
    frame.vars.clear();
    frame.pre_context.clear();
    frame.context_line = None;
    frame.post_context.clear();
}

/// Scrub one free-text value: paths, then credentials, then length.
pub(super) fn scrub_text(value: &str, home: Option<&str>) -> String {
    let scrubbed = scrub_secrets(&scrub_paths(value, home));
    truncate(scrubbed)
}

/// Replace the home directory with `~`, then mask any other user directory.
pub(super) fn scrub_paths(value: &str, home: Option<&str>) -> String {
    // A path-component boundary, not a word boundary: `\b` would also match
    // inside `/home/alice2`, eating the unrelated account's directory as if it
    // were the operator's own. Requiring a separator, quote, whitespace, or
    // end of string after the directory keeps the match to whole path
    // components. Compiled per call: crash events are rare enough that
    // caching buys nothing worth the extra state.
    let home = home
        .map(|home| home.trim_end_matches(['/', '\\']))
        .filter(|home| home.len() > 1)
        .and_then(|home| {
            // Either a path separator (consumed and kept in the replacement)
            // or a zero-width lookahead for whitespace, a quote, or the end of
            // the string — so the boundary check never eats a character that
            // belongs to whatever follows.
            Regex::new(&format!(
                r#"{}(?:(?P<sep>[/\\])|(?=[\s"':]|$))"#,
                regex::escape(home)
            ))
            .ok()
        });
    let value = match home {
        Some(home) => home
            .replace_all(value, |caps: &regex::Captures<'_>| match caps.name("sep") {
                Some(sep) => format!("~{}", sep.as_str()),
                None => "~".to_owned(),
            })
            .into_owned(),
        None => value.to_owned(),
    };
    user_dir_pattern()
        .replace_all(&value, "${prefix}<user>")
        .into_owned()
}

/// Redact bearer credentials and JWTs.
fn scrub_secrets(value: &str) -> String {
    let value = bearer_pattern().replace_all(value, "${scheme} <redacted>");
    jwt_pattern()
        .replace_all(&value, "<redacted-jwt>")
        .into_owned()
}

/// Cap a value at [`MAX_TEXT_LEN`] bytes on a character boundary.
fn truncate(mut value: String) -> String {
    if value.len() > MAX_TEXT_LEN {
        let mut end = MAX_TEXT_LEN;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push('…');
    }
    value
}

/// `/Users/<name>`, `/home/<name>`, and `C:\Users\<name>` — the per-user
/// directory of any account, not just the one running Medulla.
fn user_dir_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"(?P<prefix>/Users/|/home/|(?i:[a-z]:\\Users\\))[^/\\\s"':]+"#)
            .expect("valid user-dir pattern")
    })
}

/// An HTTP `Authorization`-style credential.
fn bearer_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(?P<scheme>bearer|token|basic)\s+[A-Za-z0-9._~+/=-]{8,}")
            .expect("valid bearer pattern")
    })
}

/// A compact-serialized JWT (three base64url segments, header first).
fn jwt_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").expect("valid jwt pattern")
    })
}
