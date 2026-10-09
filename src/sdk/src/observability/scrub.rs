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
    // Debug images can carry local source/build paths (`code_file`,
    // `debug_file`, and image names). None are needed for the stack shape.
    event.debug_meta = Default::default();
    event.fingerprint = Default::default();
    // `logger` is free text an integration can set to anything, and a
    // template frame carries file paths and source lines; neither is needed
    // to identify the failure.
    event.logger = None;
    event.template = None;
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
        // Mechanism data is extensible and can contain arbitrary runtime or
        // user supplied metadata; it is not needed to diagnose the exception.
        exception.mechanism = None;
        scrub_stacktrace(exception.stacktrace.as_mut(), home);
        scrub_stacktrace(exception.raw_stacktrace.as_mut(), home);
    }
    scrub_stacktrace(event.stacktrace.as_mut(), home);
    for thread in &mut event.threads.values {
        thread.name = None;
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
    stacktrace.registers.clear();
    for frame in &mut stacktrace.frames {
        scrub_frame(frame, home);
    }
}

/// Drop every path field of a frame and anything else that could carry
/// runtime data.
///
/// Masking home/user directories (as [`scrub_paths`] does for free-text
/// values) is not enough here: a build or CI checkout path
/// (`/opt/checkout/...`, `/work/project/...`) carries no user directory to
/// mask but still describes the machine's layout, and it is exactly what
/// `abs_path`/`filename`/`package` hold on every frame. Frame strings are not
/// allowlisted because integrations can populate them dynamically; numeric
/// location metadata remains available for the stack shape.
fn scrub_frame(frame: &mut Frame, _home: Option<&str>) {
    // Function names come from the compiled program and are useful for
    // diagnosis; drop symbol/module strings, which may be integration data.
    frame.symbol = None;
    frame.module = None;
    frame.filename = None;
    frame.abs_path = None;
    frame.package = None;
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
            // A path separator or any non-path punctuation terminates the
            // configured directory. Capture and restore it because regex has
            // no lookahead. Letters, digits, and combining marks in any script,
            // plus path punctuation, remain part of the component, so neither
            // `/home/alice2` nor `/home/aliceé` is mistaken for `/home/alice`.
            Regex::new(&format!(
                r#"{}([/\\]|[^/\\\p{{L}}\p{{N}}\p{{M}}._-]|$)"#,
                regex::escape(home)
            ))
            .ok()
        });
    let value = match home {
        Some(home) => home
            .replace_all(value, |caps: &regex::Captures<'_>| format!("~{}", &caps[1]))
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
        let mut end = MAX_TEXT_LEN - '…'.len_utf8();
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push('…');
    }
    value
}

/// `/Users/<name>`, `/home/<name>`, `C:\Users\<name>`, and `C:/Users/<name>` —
/// the per-user directory of any account, not just the one running Medulla.
/// Windows paths appear with either slash direction (APIs, panic messages, and
/// normalization all vary), so both are matched.
fn user_dir_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"(?P<prefix>/Users/|/home/|(?i:[a-z]:[\\/]Users[\\/]))[^/\\\s"':]+"#)
            .expect("valid user-dir pattern")
    })
}

/// An HTTP `Authorization`-style credential. No minimum length: this is the
/// last line of defence before a panic message leaves the machine, so even a
/// short bearer token or Basic credential must be redacted.
fn bearer_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(?P<scheme>bearer|token|basic)\s+[A-Za-z0-9._~+/=-]+")
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
