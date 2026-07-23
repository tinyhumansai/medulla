//! Structured, append-only lessons in a workspace `MEDULLA.md`.
//!
//! Entries live under `## Lessons` as `- when <trigger>: <rule>`. The reader is
//! deliberately narrow so similarly-shaped prose elsewhere in the profile is
//! never mistaken for an operator lesson.

mod types;

#[cfg(test)]
mod tests;

pub use types::{AddLessonOutcome, Lesson, LessonError};

use std::fs;
use std::path::Path;

use crate::init::PROFILE_FILE;

const HEADING: &str = "## Lessons";
const ENTRY_PREFIX: &str = "- when ";

/// Parse the `<trigger> -> <rule>` shape shared by CLI and TUI entry points.
pub fn parse_lesson_spec(spec: &str) -> Result<Lesson, LessonError> {
    let (trigger, rule) = spec
        .split_once("->")
        .ok_or(LessonError::InvalidLessonFormat)?;
    normalize(Lesson::new(trigger, rule))
}

/// Read the structured lessons in `workspace/MEDULLA.md`.
pub fn list_lessons(workspace: &Path) -> Result<Vec<Lesson>, LessonError> {
    let path = workspace.join(PROFILE_FILE);
    let document = read_profile(&path)?;
    Ok(parse_lessons(&document))
}

/// Parse structured lessons from a complete workspace profile.
pub fn parse_lessons(document: &str) -> Vec<Lesson> {
    let Some((start, end)) = section_bounds(document) else {
        return Vec::new();
    };
    document[start..end]
        .lines()
        .filter_map(|line| {
            let body = line.trim().strip_prefix(ENTRY_PREFIX)?;
            let (trigger, rule) = body.split_once(": ")?;
            let trigger = trigger.trim();
            let rule = rule.trim();
            (!trigger.is_empty() && !rule.is_empty()).then(|| Lesson::new(trigger, rule))
        })
        .collect()
}

/// Add a lesson to `workspace/MEDULLA.md`, preserving all surrounding content.
///
/// Re-adding the exact normalized trigger/rule pair is idempotent.
pub fn add_lesson(workspace: &Path, lesson: Lesson) -> Result<AddLessonOutcome, LessonError> {
    let lesson = normalize(lesson)?;
    let path = workspace.join(PROFILE_FILE);
    let document = read_profile(&path)?;
    if parse_lessons(&document).contains(&lesson) {
        return Ok(AddLessonOutcome::AlreadyPresent);
    }

    let entry = format!("{ENTRY_PREFIX}{}: {}", lesson.trigger, lesson.rule);
    let updated = match section_bounds(&document) {
        Some((_, end)) => insert_in_section(&document, end, &entry),
        None => append_section(&document, &entry),
    };
    fs::write(&path, updated).map_err(|source| LessonError::Io { path, source })?;
    Ok(AddLessonOutcome::Added)
}

fn normalize(lesson: Lesson) -> Result<Lesson, LessonError> {
    let trigger = lesson.trigger.trim();
    let rule = lesson.rule.trim();
    if trigger.is_empty() || rule.is_empty() {
        return Err(LessonError::EmptyLesson);
    }
    Ok(Lesson::new(trigger, rule))
}

fn read_profile(path: &Path) -> Result<String, LessonError> {
    fs::read_to_string(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            LessonError::MissingProfile(path.to_path_buf())
        } else {
            LessonError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    })
}

/// Byte range after the heading through (but not including) the next H2.
fn section_bounds(document: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    let mut start = None;
    for line in document.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if start.is_none() {
            if content.trim() == HEADING {
                start = Some(offset + line.len());
            }
        } else if let Some(section_start) = start {
            if content.trim_start().starts_with("## ") {
                return Some((section_start, offset));
            }
        }
        offset += line.len();
    }
    start.map(|value| (value, document.len()))
}

fn insert_in_section(document: &str, end: usize, entry: &str) -> String {
    let section_prefix = &document[..end];
    let content_end = section_prefix.trim_end_matches(char::is_whitespace).len();
    let mut out = String::with_capacity(document.len() + entry.len() + 2);
    out.push_str(&document[..content_end]);
    if content_end > 0 && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(entry);
    let suffix = &document[content_end..];
    if !suffix.starts_with('\n') && !suffix.starts_with('\r') {
        out.push('\n');
    }
    out.push_str(suffix);
    out
}

fn append_section(document: &str, entry: &str) -> String {
    let mut out = document.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(HEADING);
    out.push_str("\n\n");
    out.push_str(entry);
    out.push('\n');
    out
}
