//! One workflow's notes on disk.
//!
//! Stored under the state directory beside run records rather than beside the
//! definitions, because a journal is *host* knowledge: it is what this machine
//! observed while running the workflow, not part of the document an operator
//! edits and commits.
//!
//! One file per workflow, not one per note. Notes are only ever read as a whole
//! set — a brief wants all of them or none — and a directory per workflow would
//! reproduce the unindexed scan that already makes run history expensive.
//! Writes are read-modify-write under the store's existing write lock, which is
//! what makes appending safe against a concurrent supersession.

use std::path::{Path, PathBuf};

use crate::workflows::types::{NoteId, WorkflowError, WorkflowNote};

use super::paths::{safe_component, write_atomic};

/// How many notes one workflow keeps.
///
/// Generous, because a note is a sentence rather than a graph, and a workflow
/// that has failed a hundred times has a hundred things worth remembering. The
/// cap exists so an automated pass writing on every failure cannot grow a file
/// without bound.
pub const MAX_NOTES: usize = 100;

/// Tie-breaker for notes written inside the same millisecond.
///
/// Process-wide for the same reason revisions use one: it only has to increase,
/// and a per-file count read off disk could be raced into reuse.
static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Mint a note id that sorts chronologically.
///
/// Same three-part scheme as a revision id: a zero-padded stamp so a lexical
/// sort is a chronological one, a monotonic counter because a pass writes
/// several notes inside one millisecond, and a random token because two
/// processes can pick the same counter.
pub fn mint_id(recorded_at: u64) -> NoteId {
    format!(
        "{recorded_at:013}-{:012}-{}",
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        uuid::Uuid::new_v4()
    )
}

/// Where one workflow's journal lives.
fn path_for(journal_dir: &Path, workflow_id: &str) -> Result<PathBuf, WorkflowError> {
    Ok(journal_dir.join(format!("{}.json", safe_component(workflow_id)?)))
}

/// Every note for `workflow_id`, newest first, superseded ones included.
///
/// A journal this host cannot parse yields an empty list with a warning rather
/// than an error. The alternative is that one bad file makes a workflow
/// unreadable everywhere its notes are shown, which is a worse failure than
/// forgetting what it learned — and run history already behaves this way.
pub fn list(journal_dir: &Path, workflow_id: &str) -> Result<Vec<WorkflowNote>, WorkflowError> {
    let mut notes = read_all(journal_dir, workflow_id)?;
    notes.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(notes)
}

/// Append `note`, then prune to [`MAX_NOTES`].
///
/// # Errors
///
/// Fails when the workflow id is not a usable filename, or when the file cannot
/// be written. A note that could not be recorded is a real failure: the callers
/// that append are the ones claiming the host now knows something.
pub fn append(journal_dir: &Path, note: &WorkflowNote) -> Result<(), WorkflowError> {
    let mut notes = read_all(journal_dir, &note.workflow_id)?;
    notes.push(note.clone());
    prune(&mut notes);
    write(journal_dir, &note.workflow_id, &notes)
}

/// Mark `id` as replaced by `by`.
///
/// Silently does nothing when the note is not there. Supersession is a tidying
/// action taken after the fact, and a caller naming a note that has already
/// been pruned away has nothing left to fix.
pub fn supersede(
    journal_dir: &Path,
    workflow_id: &str,
    id: &str,
    by: &str,
) -> Result<(), WorkflowError> {
    let mut notes = read_all(journal_dir, workflow_id)?;
    let mut changed = false;
    for note in notes.iter_mut() {
        if note.id == id && note.superseded_by.is_none() {
            note.superseded_by = Some(by.to_string());
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    write(journal_dir, workflow_id, &notes)
}

/// Read the file, treating absence and corruption alike as "nothing learned".
fn read_all(journal_dir: &Path, workflow_id: &str) -> Result<Vec<WorkflowNote>, WorkflowError> {
    let path = path_for(journal_dir, workflow_id)?;
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(WorkflowError::Io { path, source }),
    };
    match serde_json::from_slice::<Vec<WorkflowNote>>(&body) {
        Ok(notes) => Ok(notes),
        Err(err) => {
            tracing::warn!(
                workflow = %workflow_id,
                path = %path.display(),
                "workflow journal is unreadable, treating it as empty: {err}"
            );
            Ok(Vec::new())
        }
    }
}

/// Write the whole journal back.
fn write(
    journal_dir: &Path,
    workflow_id: &str,
    notes: &[WorkflowNote],
) -> Result<(), WorkflowError> {
    let body = serde_json::to_vec_pretty(notes)
        .map_err(|err| WorkflowError::Malformed(err.to_string()))?;
    write_atomic(&path_for(journal_dir, workflow_id)?, &body)
}

/// Drop the oldest notes past [`MAX_NOTES`].
///
/// Pinned notes and superseded ones are protected for opposite reasons: a
/// pinned note is what an operator said, and a superseded one is already the
/// cheap half of a pair whose replacement would otherwise dangle. Everything
/// else goes oldest-first.
fn prune(notes: &mut Vec<WorkflowNote>) {
    if notes.len() <= MAX_NOTES {
        return;
    }
    let mut droppable: Vec<usize> = notes
        .iter()
        .enumerate()
        .filter(|(_, note)| !note.pinned && note.superseded_by.is_none())
        .map(|(index, _)| index)
        .collect();
    // Oldest first: ids sort chronologically, and so does insertion order.
    droppable.sort_by(|a, b| notes[*a].id.cmp(&notes[*b].id));
    droppable.truncate(notes.len() - MAX_NOTES);
    let doomed: std::collections::HashSet<usize> = droppable.into_iter().collect();
    let mut index = 0;
    notes.retain(|_| {
        let keep = !doomed.contains(&index);
        index += 1;
        keep
    });
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
