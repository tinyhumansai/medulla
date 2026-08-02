//! Workspace discovery, completion, and recent-history persistence for the
//! manual harness launcher.

use std::collections::{BinaryHeap, HashSet};
use std::path::Path;

use super::types::{App, HarnessPickerStep, WorkspaceChoice};
use crate::ui::composer::flatten_paste;

const MAX_WORKSPACE_CHOICES: usize = 10;
const MAX_RECENT_WORKSPACES: usize = 12;
const FOLDER_SCORE_OFFSET: usize = 5;
const KNOWN_FUZZY_SCORE_OFFSET: usize = 20;

impl App {
    /// Advance the launcher to its workspace step and populate the first list.
    pub(super) fn open_harness_workspace_step(&mut self, edit_default: bool) {
        let default = self
            .harness_picker
            .as_ref()
            .map(|picker| picker.cwd.clone())
            .unwrap_or_default();
        if let Some(picker) = &mut self.harness_picker {
            picker.step = HarnessPickerStep::Workspace;
            picker.workspace_query = if edit_default { default } else { String::new() };
            picker.workspace_index = 0;
        }
        self.refresh_harness_workspace_choices();
        self.set_status("Workspace · type to filter · Tab complete · Enter start · Esc back");
    }

    /// Take a pasted directory into the workspace query, on the step that has
    /// one.
    ///
    /// The picker is two overlays in one. Its harness step is a list of
    /// providers with no field on it, so a paste there is dropped for the same
    /// reason it swallows the keyboard — leaving it to surface in the query once
    /// the step advanced would be worse than losing it. Its workspace step *is*
    /// a text field ("type to filter"), and pasting a path into it is the common
    /// case.
    ///
    /// Appended rather than inserted, and flattened to one line, because that is
    /// what the query is: a single-line box with no caret, edited by the same
    /// `push`/`pop` that typing uses. A path copied with a trailing newline
    /// therefore lands as the path plus a space, which
    /// [`resolve_workspace`](crate::ui::harness_pane::LocalHarnesses::resolve_workspace)
    /// trims before it is used.
    pub(super) fn paste_into_harness_workspace(&mut self, text: &str) {
        let Some(picker) = &mut self.harness_picker else {
            return;
        };
        if picker.step != HarnessPickerStep::Workspace {
            return;
        }
        picker.workspace_query.push_str(&flatten_paste(text));
        // Narrowing the completions invalidates where the cursor pointed,
        // exactly as typing a character does.
        picker.workspace_index = 0;
        self.refresh_harness_workspace_choices();
    }

    /// Recompute cached completions after the query changes.
    pub(super) fn refresh_harness_workspace_choices(&mut self) {
        let Some(picker) = &self.harness_picker else {
            return;
        };
        let query = picker.workspace_query.clone();
        let choices = self.workspace_choices(&query);
        if let Some(picker) = &mut self.harness_picker {
            picker.workspace_choices = choices;
            picker.workspace_index = picker
                .workspace_index
                .min(picker.workspace_choices.len().saturating_sub(1));
        }
    }

    /// Selected completion, falling back to valid typed input.
    pub(super) fn selected_harness_workspace(&self) -> Option<String> {
        let picker = self.harness_picker.as_ref()?;
        if let Some(choice) = picker.workspace_choices.get(picker.workspace_index) {
            return Some(choice.path.clone());
        }
        let harnesses = self.harnesses.as_ref()?;
        let resolved = harnesses.resolve_workspace(&picker.workspace_query);
        Path::new(&resolved).is_dir().then_some(resolved)
    }

    /// Make the highlighted completion the editable query.
    pub(super) fn complete_harness_workspace(&mut self) {
        let selected = self.harness_picker.as_ref().and_then(|picker| {
            picker
                .workspace_choices
                .get(picker.workspace_index)
                .map(|choice| choice.path.clone())
        });
        if let (Some(picker), Some(selected)) = (&mut self.harness_picker, selected) {
            picker.workspace_query = selected;
            picker.workspace_index = 0;
            self.refresh_harness_workspace_choices();
        }
    }

    /// Remember a successful launch newest-first, both in memory and config.
    pub(super) fn remember_harness_workspace(&mut self, workspace: &str) -> Result<(), String> {
        let recent = &mut self.loaded.config.harness.recent_workspaces;
        recent.retain(|candidate| candidate != workspace);
        recent.insert(0, workspace.to_string());
        recent.truncate(MAX_RECENT_WORKSPACES);
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        medulla::config::persist_setting(
            path,
            "harness",
            "recentWorkspaces",
            toml::Value::Array(recent.iter().cloned().map(toml::Value::String).collect()),
        )
        .map_err(|error| format!("workspace history was not saved ({error})"))
    }

    /// Rank recent, configured, and filesystem-derived workspace suggestions.
    fn workspace_choices(&self, query: &str) -> Vec<WorkspaceChoice> {
        let Some(harnesses) = &self.harnesses else {
            return Vec::new();
        };
        let base = Path::new(&harnesses.workspace);
        let process_dir = std::env::current_dir().unwrap_or_else(|_| base.to_path_buf());
        let resolved_query = harnesses.resolve_workspace(query);
        let mut known = Vec::new();
        for path in &self.loaded.config.harness.recent_workspaces {
            known.push((absolute(path, base), "recent"));
        }
        known.push((harnesses.workspace.clone(), "default"));
        if !self.loaded.config.host.workspace.trim().is_empty() {
            known.push((
                absolute(&self.loaded.config.host.workspace, &process_dir),
                "registered",
            ));
        }
        for path in &self.loaded.config.host.workspaces {
            known.push((absolute(path, &process_dir), "registered"));
        }
        for host in &self.loaded.config.hosts {
            if !host.workspace.trim().is_empty() {
                known.push((absolute(&host.workspace, &process_dir), "registered"));
            }
            for path in &host.workspaces {
                known.push((absolute(path, &process_dir), "registered"));
            }
        }

        let folder_order = known.len();
        let mut ranked = known
            .into_iter()
            .enumerate()
            .filter(|(_, (path, _))| Path::new(path).is_dir())
            .filter_map(|(order, (path, source))| {
                match_score(&path, query)
                    .map(|score| (score, order, WorkspaceChoice { path, source }))
            })
            .collect::<Vec<_>>();

        if !query.trim().is_empty() {
            ranked.extend(
                folder_completions(&resolved_query)
                    .into_iter()
                    .enumerate()
                    .map(|(index, (score, path))| {
                        (
                            score + FOLDER_SCORE_OFFSET,
                            folder_order + index,
                            WorkspaceChoice {
                                path,
                                source: "folder",
                            },
                        )
                    }),
            );
        }
        ranked.sort_by(
            |(left_score, left_order, _), (right_score, right_order, _)| {
                left_score
                    .cmp(right_score)
                    .then_with(|| left_order.cmp(right_order))
            },
        );

        let mut seen = HashSet::new();
        ranked
            .into_iter()
            .map(|(_, _, choice)| choice)
            .filter(|choice| seen.insert(choice.path.clone()))
            .take(MAX_WORKSPACE_CHOICES)
            .collect()
    }
}

/// Make a configured path absolute against its owning resolution directory.
pub(super) fn absolute(path: &str, base: &Path) -> String {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        base.join(path).to_string_lossy().into_owned()
    }
}

/// Rank an existing path against the query; lower scores are better.
pub(super) fn match_score(path: &str, query: &str) -> Option<usize> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    let path_lower = path.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if path_lower == query_lower {
        return Some(0);
    }
    if path_lower.starts_with(&query_lower) {
        return Some(1);
    }
    let name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name == query_lower {
        return Some(0);
    }
    if name.starts_with(&query_lower) {
        return Some(1);
    }
    // Exact and prefix matches still favour known workspaces. A loose fuzzy
    // match does not: otherwise random parent names such as `.tmpbQM6Hg`
    // outrank a concrete `project-beta` folder for the query `pb`.
    fuzzy_subsequence_score(&name, &query_lower)
        .or_else(|| fuzzy_subsequence_score(&path_lower, &query_lower))
        .map(|score| score + KNOWN_FUZZY_SCORE_OFFSET)
}

/// Complete only immediate child directories, keeping filesystem work bounded.
pub(super) fn folder_completions(query: &str) -> Vec<(usize, String)> {
    let path = Path::new(query);
    let (parent, needle) = if query.ends_with(std::path::MAIN_SEPARATOR) {
        (path, "")
    } else {
        (
            path.parent().unwrap_or_else(|| Path::new(".")),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
        )
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches = BinaryHeap::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(score) =
            fuzzy_subsequence_score(&name.to_ascii_lowercase(), &needle.to_ascii_lowercase())
        else {
            continue;
        };
        matches.push((score, entry.path().to_string_lossy().into_owned()));
        if matches.len() > MAX_WORKSPACE_CHOICES {
            matches.pop();
        }
    }
    let mut matches = matches.into_vec();
    matches.sort();
    matches
}

/// Subsequence matcher that rewards prefixes and tightly grouped characters.
pub(super) fn fuzzy_subsequence_score(candidate: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    if candidate.starts_with(query) {
        return Some(candidate.len().saturating_sub(query.len()));
    }
    let mut score = 0;
    let mut position = 0;
    for needle in query.chars() {
        let relative = candidate.get(position..)?.find(needle)?;
        score += relative + 1;
        position += relative + needle.len_utf8();
    }
    Some(score + candidate.len().saturating_sub(query.len()))
}
