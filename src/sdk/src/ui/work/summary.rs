//! The one-line forms of a work snapshot: the chip a list row can carry and the
//! headline a pane header can show.
//!
//! Both exist because the full panel needs a pane and most surfaces have a row.
//! A rail row that says `dev · ▶2/5 ⑂1` tells an operator which machine to open
//! without opening any of them, which is the whole job of a fleet list.

use crate::harness_work::WorkSnapshot;

/// A compact chip for a list row: todo progress and running sub-agents, e.g.
/// `2/5 ⑂1`. Empty when the snapshot has neither, so a row can skip it.
pub fn work_chip(work: &WorkSnapshot) -> String {
    let mut parts: Vec<String> = Vec::new();
    let (done, total) = work.todo_progress();
    if total > 0 {
        parts.push(format!("{done}/{total}"));
    }
    let running = work.running_subagents();
    if running > 0 {
        // A forked-branch glyph rather than the word: this rides in a rail row
        // whose whole budget is a handful of columns.
        parts.push(format!("⑂{running}"));
    }
    parts.join(" ")
}

/// A one-line headline for a pane header: what the agent is on right now, then
/// its goal, then the counts. `None` when the snapshot says none of those.
pub fn work_headline(work: &WorkSnapshot) -> Option<String> {
    if let Some(current) = work.current_todo() {
        let (done, total) = work.todo_progress();
        return Some(format!("{} · {done}/{total}", current.display()));
    }
    if let Some(goal) = &work.goal {
        return Some(goal.clone());
    }
    let running = work.running_subagents();
    if running > 0 {
        let plural = if running == 1 { "" } else { "s" };
        return Some(format!("{running} sub-agent{plural} running"));
    }
    let (done, total) = work.todo_progress();
    (total > 0).then(|| format!("{done}/{total} tasks done"))
}
