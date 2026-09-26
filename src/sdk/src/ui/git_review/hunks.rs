//! Locating unified-diff hunks and stepping between them.
//!
//! Hunk boundaries are derived from the patch text a caller already holds, so
//! nothing here runs Git or touches the filesystem.

use super::types::Hunk;

/// Index every `@@ … @@` hunk in a patch rendered as one line per entry.
///
/// A hunk runs from its header up to the line before the next header, or to the
/// end of the patch for the last one. Patch preamble (the `diff --git` and
/// `+++`/`---` lines) belongs to no hunk and is deliberately not included.
pub fn hunks(patch: &[String]) -> Vec<Hunk> {
    let headers: Vec<usize> = patch
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("@@"))
        .map(|(index, _)| index)
        .collect();
    headers
        .iter()
        .enumerate()
        .map(|(position, &header)| Hunk {
            header,
            end: headers
                .get(position + 1)
                .copied()
                .unwrap_or(patch.len())
                .max(header + 1),
            label: patch[header].clone(),
        })
        .collect()
}

/// The header line of the first hunk that starts after `line`.
///
/// Returns `None` when the cursor already sits at or past the final hunk, which
/// lets a caller leave the cursor where it is instead of wrapping.
pub fn next_hunk(hunks: &[Hunk], line: usize) -> Option<usize> {
    hunks
        .iter()
        .map(|hunk| hunk.header)
        .find(|&header| header > line)
}

/// The header line of the last hunk that starts before `line`.
pub fn previous_hunk(hunks: &[Hunk], line: usize) -> Option<usize> {
    hunks
        .iter()
        .map(|hunk| hunk.header)
        .rfind(|&header| header < line)
}
