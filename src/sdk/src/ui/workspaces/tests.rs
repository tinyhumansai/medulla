//! Tests for the workspaces surface.

use super::*;
use crate::runtime::fleet::{
    HarnessDescriptor, HostDescriptor, WorkspaceDescriptor, WorkspaceProfile,
};

fn host_with(workspaces: &[&str]) -> HostSection {
    HostSection {
        workspaces: workspaces.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn fleet() -> CapacitySnapshot {
    CapacitySnapshot {
        hosts: vec![HostDescriptor {
            id: "host-1".into(),
            name: "workshop".into(),
            availability: "online".into(),
            address: None,
            resources: None,
            metadata: Default::default(),
        }],
        harnesses: vec![HarnessDescriptor {
            id: "harness-1".into(),
            host_id: "host-1".into(),
            kind: "claude".into(),
            availability: "online".into(),
            ready: true,
            ready_reason: None,
            providers: Vec::new(),
            template_ids: Vec::new(),
            budgets: Vec::new(),
            metadata: Default::default(),
        }],
        workspaces: vec![WorkspaceDescriptor {
            id: "ws-1".into(),
            name: "repo".into(),
            path: "/srv/repos/medulla".into(),
            harness_id: "harness-1".into(),
            profile: Some(WorkspaceProfile {
                instructions: "\n  Rust workspace: SDK plus TUI.\nMore detail.".into(),
                ..Default::default()
            }),
            project: Some("medulla".into()),
            template_ids: Vec::new(),
            metadata: Default::default(),
        }],
        templates: Vec::new(),
    }
}

#[test]
fn this_devices_directories_lead_the_list_with_the_working_one_first() {
    let rows = workspace_rows(
        &host_with(&["/srv/side-project"]),
        "/srv/primary",
        &CapacitySnapshot::default(),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path, "/srv/primary");
    assert!(rows[0].primary, "the working directory leads");
    assert_eq!(rows[1].path, "/srv/side-project");
    assert!(!rows[1].primary);
    // Both are this device's, so both can be removed here.
    assert!(rows.iter().all(|r| r.editable && r.host.is_none()));
}

#[test]
fn a_remote_workspace_says_where_it_is_and_what_it_is_for() {
    let rows = workspace_rows(&HostSection::default(), "", &fleet());
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.path, "/srv/repos/medulla");
    assert_eq!(row.label, "medulla", "the declared project names it");
    assert_eq!(row.host.as_deref(), Some("workshop"));
    assert_eq!(
        row.detail.as_deref(),
        Some("Rust workspace: SDK plus TUI."),
        "the profile's first real line, not its blank one"
    );
    // Another machine's declaration is not this one's to edit.
    assert!(!row.editable);
    assert!(!row.primary);
}

#[test]
fn a_directory_declared_twice_is_listed_once_and_stays_editable() {
    // The same path from both sources is one place to work, and the local row is
    // the one that can be removed.
    let mut capacity = fleet();
    capacity.workspaces[0].path = "/srv/primary".into();
    let rows = workspace_rows(&HostSection::default(), "/srv/primary", &capacity);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].editable);
    assert!(rows[0].primary);
}

#[test]
fn a_blank_or_repeated_local_entry_does_not_produce_a_row() {
    let rows = workspace_rows(
        &host_with(&["", "   ", "/srv/primary", "/srv/other", "/srv/other"]),
        "/srv/primary",
        &CapacitySnapshot::default(),
    );
    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, vec!["/srv/primary", "/srv/other"]);
}

#[test]
fn a_missing_directory_is_flagged_and_a_real_one_is_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let real = dir.path().to_string_lossy().into_owned();
    let rows = workspace_rows(
        &host_with(&["/definitely/not/here"]),
        &real,
        &CapacitySnapshot::default(),
    );
    assert!(rows[0].exists, "the tempdir is there");
    assert!(!rows[1].exists, "the bogus path is not");
}

#[test]
fn a_workspace_falls_back_to_its_directory_name_when_nothing_labels_it() {
    let mut capacity = fleet();
    capacity.workspaces[0].project = None;
    capacity.workspaces[0].name = String::new();
    let rows = workspace_rows(&HostSection::default(), "", &capacity);
    assert_eq!(rows[0].label, "medulla");
}

#[test]
fn a_workspace_whose_harness_is_missing_still_lists_without_inventing_a_host() {
    let mut capacity = fleet();
    capacity.harnesses.clear();
    let rows = workspace_rows(&HostSection::default(), "", &capacity);
    assert_eq!(rows.len(), 1, "a directory nobody can place is still one");
    assert!(rows[0].host.is_none(), "{:?}", rows[0].host);
    assert!(rows[0].detail.is_some(), "the profile still describes it");
}

#[test]
fn nothing_declared_anywhere_yields_no_rows() {
    let rows = workspace_rows(&HostSection::default(), "", &CapacitySnapshot::default());
    assert!(rows.is_empty());
}

#[test]
fn the_same_path_on_two_hosts_is_two_places_to_send_work() {
    // Container fleets make identical machine-local paths the norm. Collapsing
    // them by path alone hid real capacity.
    let mut capacity = fleet();
    capacity.hosts.push(HostDescriptor {
        id: "host-2".into(),
        name: "builder".into(),
        availability: "online".into(),
        address: None,
        resources: None,
        metadata: Default::default(),
    });
    capacity.harnesses.push(HarnessDescriptor {
        id: "harness-2".into(),
        host_id: "host-2".into(),
        kind: "codex".into(),
        availability: "online".into(),
        ready: true,
        ready_reason: None,
        providers: Vec::new(),
        template_ids: Vec::new(),
        budgets: Vec::new(),
        metadata: Default::default(),
    });
    let mut twin = capacity.workspaces[0].clone();
    twin.id = "ws-2".into();
    twin.harness_id = "harness-2".into();
    capacity.workspaces.push(twin);

    let rows = workspace_rows(&HostSection::default(), "", &capacity);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let hosts: Vec<Option<&str>> = rows.iter().map(|r| r.host.as_deref()).collect();
    assert_eq!(hosts, vec![Some("workshop"), Some("builder")]);
}

#[test]
fn a_remote_row_is_still_folded_into_a_local_one_at_the_same_path() {
    // This device's own workspace, reported back through the fleet, genuinely is
    // one directory seen twice — and the local row is the editable one.
    let mut capacity = fleet();
    capacity.workspaces[0].path = "/srv/primary".into();
    let rows = workspace_rows(&HostSection::default(), "/srv/primary", &capacity);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].editable);
}
