//! Workspace discovery, completion, and recent-history persistence for the
//! manual harness launcher.

use std::collections::{BinaryHeap, HashSet};
use std::path::Path;

use super::types::{App, SessionPickerStep, WorkspaceChoice};
use crate::ui::composer::flatten_paste;

const MAX_WORKSPACE_CHOICES: usize = 10;
const MAX_RECENT_WORKSPACES: usize = 12;
const FOLDER_SCORE_OFFSET: usize = 5;
const KNOWN_FUZZY_SCORE_OFFSET: usize = 20;

impl App {
    /// Advance the launcher to its workspace step and populate the first list.
    pub(super) fn open_harness_workspace_step(&mut self, edit_default: bool) {
        let default = self
            .session_picker
            .as_ref()
            .map(|picker| picker.cwd.clone())
            .unwrap_or_default();
        if let Some(picker) = &mut self.session_picker {
            picker.step = SessionPickerStep::Workspace;
            picker.workspace_query = if edit_default { default } else { String::new() };
            picker.workspace_index = 0;
            picker.workspace_picked = false;
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
    /// [`resolve_workspace`](crate::ui::harness_pane::LocalSessions::resolve_workspace)
    /// trims before it is used.
    pub(super) fn paste_into_harness_workspace(&mut self, text: &str) {
        let Some(picker) = &mut self.session_picker else {
            return;
        };
        if picker.step != SessionPickerStep::Workspace {
            return;
        }
        picker.workspace_query.push_str(&flatten_paste(text));
        // Narrowing the completions invalidates where the cursor pointed,
        // exactly as typing a character does.
        picker.workspace_index = 0;
        picker.workspace_picked = false;
        self.refresh_harness_workspace_choices();
    }

    /// Recompute cached completions after the query changes.
    pub(super) fn refresh_harness_workspace_choices(&mut self) {
        let Some(picker) = &self.session_picker else {
            return;
        };
        let query = picker.workspace_query.clone();
        let choices = self.workspace_choices(&query);
        if let Some(picker) = &mut self.session_picker {
            picker.workspace_choices = choices;
            picker.workspace_index = picker
                .workspace_index
                .min(picker.workspace_choices.len().saturating_sub(1));
        }
    }

    /// The workspace Enter would start in: a deliberately chosen completion, an
    /// entered path that already names a directory, or nothing.
    ///
    /// A query that resolves to a real directory is the operator's own answer,
    /// so it outranks the completions listed *under* it — until they arrow onto
    /// one, which sets `workspace_picked` and is honoured instead. That flag is
    /// tracked rather than inferred from `workspace_index`, because a query
    /// offering a single completion leaves the cursor on row zero however
    /// deliberately it was moved there.
    ///
    /// Without that, a directory with a trailing separator — the form a path
    /// copied out of a file manager takes — filled the list with its own
    /// children, and Enter silently started the harness in the first of them
    /// rather than in the directory that was asked for.
    pub(super) fn selected_picker_workspace(&self) -> Option<String> {
        let picker = self.session_picker.as_ref()?;
        let resolved = self
            .local_sessions
            .as_ref()
            .map(|harnesses| harnesses.resolve_workspace(&picker.workspace_query));
        // Blank means "the default", which is what the completions already rank
        // for, so an empty query is left to them.
        if !picker.workspace_picked && !picker.workspace_query.trim().is_empty() {
            if let Some(resolved) = resolved.as_ref().filter(|r| Path::new(r).is_dir()) {
                return Some(resolved.clone());
            }
        }
        if let Some(choice) = picker.workspace_choices.get(picker.workspace_index) {
            return Some(choice.path.clone());
        }
        resolved.filter(|resolved| Path::new(resolved).is_dir())
    }

    /// Make the highlighted completion the editable query.
    pub(super) fn complete_harness_workspace(&mut self) {
        let selected = self.session_picker.as_ref().and_then(|picker| {
            picker
                .workspace_choices
                .get(picker.workspace_index)
                .map(|choice| choice.path.clone())
        });
        if let (Some(picker), Some(selected)) = (&mut self.session_picker, selected) {
            picker.workspace_query = selected;
            picker.workspace_index = 0;
            // Completing *is* entering it: the query now names the directory,
            // so it answers for itself rather than for the rows beneath it.
            picker.workspace_picked = false;
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

    /// Persist `workspace` as a named shortcut for the manual launcher.
    ///
    /// A name replaces any older favorite with the same spelling, while the
    /// path is de-duplicated so one directory cannot occupy several top-ranked
    /// rows under different aliases.
    pub(in crate::ui::app) fn save_favorite_workspace(&mut self, name: &str, workspace: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.set_status("Favorite name cannot be empty");
            return;
        }
        let Some(harnesses) = &self.local_sessions else {
            self.set_status("This device is not hosting, so it has no workspace favorites");
            return;
        };
        let path = harnesses.resolve_workspace(workspace);
        if !Path::new(&path).is_dir() {
            self.set_status("Favorites must point to an existing directory");
            return;
        }
        // Build the candidate list without touching the live one: persistence
        // is the commit point, so a write failure must not leave a favorite in
        // memory that the config file never recorded — the picker would then
        // offer a save that is not there, and a later successful save could
        // silently persist it.
        let mut favorites = self.loaded.config.harness.favorite_workspaces.clone();
        // Compare effective resolved paths rather than the stored spellings: a
        // favorite saved from relative input (e.g. `repo` against the host
        // workspace) must not survive beside the same directory re-saved under
        // its resolved absolute path — a re-alias, not a second directory.
        favorites.retain(|favorite| {
            !favorite.name.eq_ignore_ascii_case(name)
                && harnesses.resolve_workspace(&favorite.path) != path
        });
        favorites.insert(
            0,
            medulla::config::FavoriteWorkspace {
                name: name.to_string(),
                path: path.clone(),
            },
        );
        let Some(config_path) = &self.config_path else {
            self.loaded.config.harness.favorite_workspaces = favorites;
            self.set_status(format!(
                "Saved favorite {name} · this run only — no config file"
            ));
            self.after_saving_favorite(&path);
            return;
        };
        match medulla::config::persist_setting(
            config_path,
            "harness",
            "favoriteWorkspaces",
            toml::Value::try_from(favorites.clone()).expect("favorite workspaces serialize"),
        ) {
            Ok(()) => {
                self.loaded.config.harness.favorite_workspaces = favorites;
                self.set_status(format!("Saved favorite {name} · {path}"));
                self.after_saving_favorite(&path);
            }
            Err(error) => self.set_status(format!("Could not save favorite ({error})")),
        }
    }

    /// Re-anchor the launcher on the workspace that was just favorited.
    ///
    /// Saving the favorite is the operator's way of saying Enter should start
    /// the harness there, so the arrowed cursor — which pointed at the saved
    /// row beforehand — must follow the saved workspace to the row it now
    /// occupies rather than keep its old index and silently select a
    /// *different* directory parked in that row. The ranking orders by match
    /// score first and insertion order second, so the promoted favorite only
    /// leads the list when it also scores best against the active query;
    /// otherwise it lands at a non-zero row. Re-run the completions and place
    /// the cursor on the actual row of the saved workspace, so the highlight
    /// and Enter both follow the favorite.
    ///
    /// When the rename happens under a filter whose query only matched the old
    /// label, the saved favorite is absent from the refreshed list: the refresh
    /// dropped the old match while the new name does not match the unchanged
    /// query, leaving the list empty or leading with an unrelated row. Forcing
    /// the cursor onto row 0 there would make Enter reject the workspace or
    /// start the harness in that row, so the query is re-pointed at the saved
    /// workspace instead — the one row Enter must still land on.
    fn after_saving_favorite(&mut self, saved: &str) {
        self.refresh_harness_workspace_choices();
        if let Some(picker) = &mut self.session_picker {
            if let Some(index) = picker
                .workspace_choices
                .iter()
                .position(|choice| choice.path == saved)
            {
                picker.workspace_index = index;
                picker.workspace_picked = true;
            } else {
                picker.workspace_query = saved.to_string();
                picker.workspace_index = 0;
                picker.workspace_picked = false;
                self.refresh_harness_workspace_choices();
            }
        }
    }

    /// Rank recent, configured, and filesystem-derived workspace suggestions.
    fn workspace_choices(&self, query: &str) -> Vec<WorkspaceChoice> {
        let Some(harnesses) = &self.local_sessions else {
            return Vec::new();
        };
        let base = Path::new(&harnesses.workspace);
        let process_dir = std::env::current_dir().unwrap_or_else(|_| base.to_path_buf());
        let resolved_query = harnesses.resolve_workspace(query);
        let mut known = Vec::new();
        for favorite in &self.loaded.config.harness.favorite_workspaces {
            known.push((
                absolute(&favorite.path, base),
                "favorite".to_string(),
                Some(favorite.name.clone()),
            ));
        }
        for path in &self.loaded.config.harness.recent_workspaces {
            known.push((absolute(path, base), "recent".to_string(), None));
        }
        known.push((harnesses.workspace.clone(), "default".to_string(), None));
        if !self.loaded.config.host.workspace.trim().is_empty() {
            known.push((
                absolute(&self.loaded.config.host.workspace, &process_dir),
                "registered".to_string(),
                None,
            ));
        }
        for path in &self.loaded.config.host.workspaces {
            known.push((absolute(path, &process_dir), "registered".to_string(), None));
        }
        for host in &self.loaded.config.hosts {
            if !host.workspace.trim().is_empty() {
                known.push((
                    absolute(&host.workspace, &process_dir),
                    "registered".to_string(),
                    None,
                ));
            }
            for path in &host.workspaces {
                known.push((absolute(path, &process_dir), "registered".to_string(), None));
            }
        }

        let folder_order = known.len();
        let mut ranked = known
            .into_iter()
            .enumerate()
            .filter(|(_, (path, _, _))| Path::new(path).is_dir())
            .filter_map(|(order, (path, source, label))| {
                workspace_match_score(&path, label.as_deref(), query).map(|score| {
                    (
                        score,
                        order,
                        WorkspaceChoice {
                            path,
                            source,
                            label,
                        },
                    )
                })
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
                                source: "folder".to_string(),
                                label: None,
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

/// Match a saved name and its path, keeping whichever scores better.
///
/// A favorite is searchable by both spellings an operator can use, and the two
/// can disagree: a query that names the directory exactly is a strictly better
/// match than one that only loosely resembles the label. `or_else` would skip
/// the path score whenever the label scored at all, so a favorite could rank
/// behind a plain filesystem completion for the same directory and lose the
/// row to path de-duplication — the exact case the name was added to fix.
pub(super) fn workspace_match_score(path: &str, label: Option<&str>, query: &str) -> Option<usize> {
    let label_score = label.and_then(|label| match_score(label, query));
    let path_score = match_score(path, query);
    match (label_score, path_score) {
        (Some(label_score), Some(path_score)) => Some(label_score.min(path_score)),
        (label_score, path_score) => label_score.or(path_score),
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
