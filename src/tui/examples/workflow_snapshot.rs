//! Dump the Workflows tab to text, for one workflow, at a chosen size.
//!
//! The tab is the hardest screen in the app to reason about from the source: a
//! rail, a folded graph drawn on a character grid, an inspector and a copilot,
//! all sized to the terminal. Reading the render code tells you what it *can*
//! draw; this tells you what it *does* draw for a given workflow — which is the
//! only way to judge whether a fifteen-node graph is legible at 120 columns.
//!
//! It renders onto ratatui's `TestBackend`, so nothing is spawned: no daemon, no
//! tmux, no touching the operator's real Medulla home. The workflow comes from a
//! JSON file — either a bare definition document or one of the store's revision
//! files (`{"record": {...}}`) — and is installed into a throwaway home.
//!
//! ```text
//! cargo run -p medulla-tui --example workflow_snapshot -- \
//!     --workflow path/to/issue-implement.json --size 160x44 --keys enter
//! ```
//!
//! `--keys` is a comma-separated script of key presses applied before the draw,
//! which is how the views past the graph are reached: `enter` steps into the
//! canvas, `enter,i` opens the inspector on the selected node, `enter,down,i`
//! on the one after it, `c` the copilot.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::workflows::{parse_workflow, FileWorkflowStore, WorkflowStore};
use medulla_tui::ui::app::{App, TABS};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn main() {
    let mut workflow = None;
    let mut size = "160x44".to_string();
    let mut keys = String::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workflow" => workflow = args.next(),
            "--size" => size = args.next().unwrap_or(size),
            "--keys" => keys = args.next().unwrap_or_default(),
            "--help" | "-h" => {
                eprintln!(
                    "usage: workflow_snapshot --workflow <file.json> [--size WxH] [--keys a,b,c]"
                );
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let path = workflow.unwrap_or_else(|| {
        eprintln!("--workflow <file.json> is required");
        std::process::exit(2);
    });
    let (width, height) = parse_size(&size);

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1);
    });
    let document = unwrap_revision(&raw);
    let id = serde_json::from_str::<serde_json::Value>(&document)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .unwrap_or_else(|| "snapshot".to_string());

    let home = tempfile::tempdir().expect("temp home");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![home.path().join("workflows")],
        home.path().join("state").join("workflows").join("runs"),
    ));
    let record = parse_workflow(&document, &id).unwrap_or_else(|e| {
        eprintln!("{path} is not a workflow this build can load: {e}");
        std::process::exit(1);
    });
    store.save(&record).expect("installs");

    let mut app = App::new(
        Arc::new(MockRuntime::demo()),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    app.set_medulla_home(home.path().to_path_buf());
    app.set_workflow_store(store);
    app.tab_index = TABS
        .iter()
        .position(|tab| *tab == "Workflows")
        .expect("Workflows is a tab");
    app.reload_workflows();

    for key in keys.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        let code = key_code(key).unwrap_or_else(|| {
            eprintln!("unknown key: {key}");
            std::process::exit(2);
        });
        app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draws");
    let buffer = terminal.backend().buffer().clone();
    for row in 0..height {
        let line: String = (0..width)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        println!("{}", line.trim_end());
    }
}

/// `WxH`, or the default if it does not parse — a typo should not silently
/// render at some other size and be read as the size that was asked for.
fn parse_size(size: &str) -> (u16, u16) {
    let (w, h) = size.split_once(['x', 'X']).unwrap_or_else(|| {
        eprintln!("--size wants WxH, got {size}");
        std::process::exit(2);
    });
    match (w.trim().parse(), h.trim().parse()) {
        (Ok(w), Ok(h)) => (w, h),
        _ => {
            eprintln!("--size wants WxH, got {size}");
            std::process::exit(2);
        }
    }
}

/// Reduce whatever JSON was handed over to the flat document `parse_workflow`
/// reads: identity keys with `nodes`/`edges`/`inputs` beside them.
///
/// Two shapes arrive. The store keeps each revision as
/// `{"superseded_at": …, "record": {…}}`, and the record holds its graph under
/// a `graph` key; a hand-written workflow file is already the flat document.
/// Both are accepted, so the source of a snapshot can be whichever one is at
/// hand — a stored revision, or a file being authored.
fn unwrap_revision(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    let mut document = match value.get("record") {
        Some(record) => record.clone(),
        None => value,
    };
    // Lift the graph's own keys up beside the identity ones, dropping the
    // duplicate `id`/`name` the graph carries — the record's are the ones the
    // store is keyed by.
    if let Some(graph) = document.get("graph").cloned() {
        if let (Some(document), Some(graph)) = (document.as_object_mut(), graph.as_object()) {
            document.remove("graph");
            for (key, field) in graph {
                if key == "id" || key == "name" {
                    continue;
                }
                document.insert(key.clone(), field.clone());
            }
        }
    }
    document.to_string()
}

fn key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        }
    })
}
