//! Table-driven coverage for lane-claim blast-radius analysis.

use std::path::PathBuf;

use super::super::{
    claimed_dirty_paths, evaluate_lane_claims, validate_claim_patterns, ClaimedPath, LaneClaim,
    LaneGuardBadge,
};

fn path(workspace: &str, path: &str) -> ClaimedPath {
    ClaimedPath {
        workspace: PathBuf::from(workspace),
        path: PathBuf::from(path),
    }
}

#[test]
fn reports_outside_overlap_and_shared_path_badges() {
    let shared = path("/repo", "Cargo.lock");
    let overlap = path("/repo", "src/shared.rs");
    let report = evaluate_lane_claims(
        &[
            LaneClaim {
                lane_key: "agent:a".into(),
                permitted_paths: vec!["src/**".into()],
                touched_paths: vec![overlap.clone(), shared],
            },
            LaneClaim {
                lane_key: "agent:b".into(),
                permitted_paths: vec!["src/*.rs".into()],
                touched_paths: vec![overlap.clone()],
            },
        ],
        &["**/Cargo.lock".into()],
    )
    .unwrap();

    let a = report.badges("agent:a");
    assert!(a.contains(&LaneGuardBadge::OutsideClaim));
    assert!(a.contains(&LaneGuardBadge::Overlap));
    assert!(a.contains(&LaneGuardBadge::SharedPath));
    assert_eq!(
        report.badges("agent:b"),
        [LaneGuardBadge::Overlap].into_iter().collect()
    );
    assert_eq!(report.overlaps, [overlap].into_iter().collect());
}

#[test]
fn manual_globs_select_dirty_paths_and_preserve_workspace_identity() {
    let dirty = vec![
        path("/one", "src/lib.rs"),
        path("/two", "src/lib.rs"),
        path("/one", "docs/readme.md"),
    ];
    let selected = claimed_dirty_paths(&["src/**/*.rs".into()], &dirty).unwrap();
    assert_eq!(selected, dirty[..2]);
}

#[test]
fn no_claim_lanes_have_no_false_positive_and_invalid_globs_are_typed() {
    let report = evaluate_lane_claims(&[], &[]).unwrap();
    assert!(report.badges("agent:unclaimed").is_empty());
    assert!(report.overlaps.is_empty());

    let error = validate_claim_patterns(&["[".into()]).unwrap_err();
    assert_eq!(error.pattern, "[");
    assert!(claimed_dirty_paths(&["[".into()], &[]).is_err());
    assert!(evaluate_lane_claims(
        &[LaneClaim {
            lane_key: "agent:a".into(),
            permitted_paths: vec!["[".into()],
            touched_paths: vec![],
        }],
        &[],
    )
    .is_err());
    assert!(evaluate_lane_claims(&[], &["[".into()]).is_err());
}

#[test]
fn badges_have_stable_operator_labels() {
    assert_eq!(LaneGuardBadge::OutsideClaim.label(), "⚠ outside-claim");
    assert_eq!(LaneGuardBadge::Overlap.label(), "⚠ overlap");
    assert_eq!(LaneGuardBadge::SharedPath.label(), "⚠ shared-path");
}
