//! Feature-level tests for the persona-memory ("Memory") tab: the disabled/empty
//! hint, the status header + directive/facet overview, index-navigation clamping,
//! lazy-load on tab entry, and the `/memory` slash command. These drive the `App`
//! with a `MockRuntime` whose scripted memory seams stand in for a real service.

use std::collections::BTreeMap;
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::memory::{MemoryHit, MemoryStatus};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, Cmd, TABS};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

fn memory_tab() -> usize {
    TABS.iter()
        .position(|t| *t == "Memory")
        .expect("Memory tab")
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_str(app: &mut App, s: &str) {
    for ch in s.chars() {
        app.on_event(key(KeyCode::Char(ch)));
    }
}

/// A scripted status with two facets, mirroring a populated persona pack.
fn scripted_status() -> MemoryStatus {
    let mut facet_counts = BTreeMap::new();
    facet_counts.insert("coding_style".to_string(), 3);
    facet_counts.insert("stack".to_string(), 2);
    MemoryStatus {
        enabled: true,
        workspace: "/home/dev/.medulla/memory".into(),
        pack_exists: true,
        pack_path: "/home/dev/.medulla/memory/persona/PERSONA.md".into(),
        entry_count: 5,
        directives_count: 1,
        facet_counts,
    }
}

/// Mirror what `run_cmd` does for `Cmd::LoadMemory`: pull the sync surface off the
/// runtime and hand it to the app.
fn apply_load(app: &mut App, rt: &MockRuntime) {
    app.set_memory_loaded(rt.memory_status(), rt.memory_directives());
}

#[test]
fn memory_tab_disabled_shows_hint() {
    // A bare runtime has no scripted memory, so the tab renders the enable hint.
    let rt = Arc::new(MockRuntime::empty());
    let mut app = App::new(rt, loaded());
    app.tab_index = memory_tab();
    let out = render(&mut app, 100, 32);
    assert!(out.contains("not enabled"), "expected disabled hint: {out}");
    assert!(out.contains("backfill"), "expected backfill hint: {out}");
}

#[test]
fn memory_tab_renders_status_and_directives() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    rt.set_memory_directives(vec!["Always branch before new work".into()]);
    let mut app = App::new(rt.clone(), loaded());
    app.tab_index = memory_tab();
    apply_load(&mut app, &rt);
    let out = render(&mut app, 110, 32);
    assert!(out.contains("enabled"), "status header: {out}");
    assert!(out.contains("observation"), "counts line: {out}");
    assert!(out.contains("coding_style"), "facet summary: {out}");
    assert!(out.contains("Always branch"), "directive listed: {out}");
}

#[test]
fn memory_index_navigation_clamps() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    rt.set_memory_directives(vec!["Rule one".into(), "Rule two".into()]);
    let mut app = App::new(rt.clone(), loaded());
    app.tab_index = memory_tab();
    apply_load(&mut app, &rt);
    // Entries = 2 directives + 2 facets = 4 (max index 3).
    assert_eq!(app.memory_index(), 0);
    for _ in 0..10 {
        app.on_event(key(KeyCode::Down));
    }
    assert_eq!(app.memory_index(), 3, "Down clamps at the last entry");
    for _ in 0..10 {
        app.on_event(key(KeyCode::Up));
    }
    assert_eq!(app.memory_index(), 0, "Up clamps at the first entry");
    // j/k mirror Down/Up.
    app.on_event(key(KeyCode::Char('j')));
    assert_eq!(app.memory_index(), 1);
    app.on_event(key(KeyCode::Char('k')));
    assert_eq!(app.memory_index(), 0);
}

#[test]
fn entering_memory_tab_requests_lazy_load() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    let mut app = App::new(rt, loaded());
    app.tab_index = memory_tab() - 1;
    let cmd = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Memory");
    assert!(matches!(cmd, Some(Cmd::LoadMemory)), "expected LoadMemory");
}

#[test]
fn slash_memory_with_query_triggers_search() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    rt.set_memory_hits(vec![MemoryHit {
        facet: "workflow".into(),
        tier: "t0".into(),
        text: "Commit small and often".into(),
        quote: Some("lots of small, focused commits".into()),
        timestamp: "2026-01-01T00:00:00+00:00".into(),
        score: 0.92,
    }]);
    let mut app = App::new(rt.clone(), loaded());
    // Compose the slash command on the Chat tab and submit it.
    app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap();
    type_str(&mut app, "/memory commit style");
    let cmd = app.on_event(key(KeyCode::Enter));
    assert_eq!(app.tab(), "Memory", "search jumps to the Memory tab");
    match cmd {
        Some(Cmd::SearchMemory(q)) => assert_eq!(q, "commit style"),
        other => panic!("expected SearchMemory, got {other:?}"),
    }
    // Mirror run_cmd: load + results, then the hit renders in the detail pane.
    apply_load(&mut app, &rt);
    let hits = rt.memory_search("commit style".into(), None, 20);
    app.set_memory_results(hits, "commit style".into());
    let out = render(&mut app, 110, 32);
    assert!(out.contains("workflow"), "hit facet shown: {out}");
    assert!(out.contains("Commit small"), "hit text shown: {out}");
}

#[test]
fn memory_tab_enabled_but_pack_absent_and_no_facets() {
    // Enabled, but nothing compiled yet: the "pack absent" line, the "(none)"
    // facet summary, and the empty detail placeholder all render.
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(MemoryStatus {
        enabled: true,
        workspace: "/ws".into(),
        pack_exists: false,
        pack_path: "/ws/persona/PERSONA.md".into(),
        entry_count: 0,
        directives_count: 0,
        facet_counts: BTreeMap::new(),
    });
    let mut app = App::new(rt.clone(), loaded());
    app.tab_index = memory_tab();
    apply_load(&mut app, &rt);
    let out = render(&mut app, 110, 32);
    assert!(out.contains("absent"), "pack absent line: {out}");
    assert!(out.contains("(none)"), "facets none: {out}");
    assert!(out.contains("Select an entry"), "empty detail hint: {out}");
}

#[test]
fn memory_tab_facet_detail_renders_on_selection() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    rt.set_memory_directives(vec!["Rule one".into()]);
    let mut app = App::new(rt.clone(), loaded());
    app.tab_index = memory_tab();
    apply_load(&mut app, &rt);
    // Entries = 1 directive + 2 facets. Move onto the first facet row.
    app.on_event(key(KeyCode::Down));
    let out = render(&mut app, 110, 32);
    // The facet detail pane reports the observation count for the selected facet.
    assert!(
        out.contains("observation(s) in this facet"),
        "facet detail: {out}"
    );
}

#[test]
fn memory_search_with_no_hits_shows_empty_state() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    let mut app = App::new(rt.clone(), loaded());
    app.tab_index = memory_tab();
    apply_load(&mut app, &rt);
    // A search that returns nothing renders the "no hits" hint and Search title.
    app.set_memory_results(Vec::new(), "nonexistent".into());
    let out = render(&mut app, 110, 32);
    assert!(out.contains("No hits"), "empty search hint: {out}");
    assert!(out.contains("Search"), "search title: {out}");
}

#[test]
fn slash_memory_bare_requests_load() {
    let rt = Arc::new(MockRuntime::empty());
    rt.set_memory_status(scripted_status());
    let mut app = App::new(rt, loaded());
    app.tab_index = TABS.iter().position(|t| *t == "Chat").unwrap();
    type_str(&mut app, "/memory");
    let cmd = app.on_event(key(KeyCode::Enter));
    assert_eq!(app.tab(), "Memory");
    assert!(matches!(cmd, Some(Cmd::LoadMemory)), "bare /memory loads");
}
