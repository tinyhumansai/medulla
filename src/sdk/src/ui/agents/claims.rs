//! Pure blast-radius analysis for worker-lane path claims.
//!
//! The TUI supplies live dirty paths and optional manual claims today. A later
//! harness-contract revision can supply the same [`LaneClaim`] shape without
//! changing overlap, outside-boundary, or shared-path evaluation.

use std::collections::{BTreeMap, BTreeSet};

use globset::{Glob, GlobSet, GlobSetBuilder};
use thiserror::Error;

use super::types::{ClaimedPath, LaneClaim, LaneGuardBadge, LaneGuardReport};

/// A malformed path-claim glob.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid lane-claim pattern {pattern}: {message}")]
pub struct ClaimPatternError {
    /// Pattern that failed compilation.
    pub pattern: String,
    /// Glob parser diagnostic.
    pub message: String,
}

/// Compile a group of claim patterns, returning the offending pattern on error.
fn compile_patterns(patterns: &[String]) -> Result<GlobSet, ClaimPatternError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|error| ClaimPatternError {
            pattern: pattern.clone(),
            message: error.to_string(),
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|error| ClaimPatternError {
        pattern: patterns.join(","),
        message: error.to_string(),
    })
}

/// Validate manual permitted-path globs before storing them in TUI state.
pub fn validate_claim_patterns(patterns: &[String]) -> Result<(), ClaimPatternError> {
    compile_patterns(patterns).map(|_| ())
}

/// Select live dirty paths covered by a manual lane claim.
///
/// This bridges the pre-contract UI: the operator enters the intended globs,
/// and matching dirty paths become that lane's observed touched set. Paths that
/// match multiple claims are intentionally retained in every set so the guard
/// can surface overlap.
pub fn claimed_dirty_paths(
    permitted_paths: &[String],
    dirty_paths: &[ClaimedPath],
) -> Result<Vec<ClaimedPath>, ClaimPatternError> {
    let matcher = compile_patterns(permitted_paths)?;
    Ok(dirty_paths
        .iter()
        .filter(|item| matcher.is_match(&item.path))
        .cloned()
        .collect())
}

/// Evaluate lane claims against permitted boundaries and shared-path policy.
///
/// Lanes absent from `claims` receive no badges. This is important during the
/// manual-claim milestone: merely being visible in the Agents tab must not turn
/// an unconfigured lane red.
pub fn evaluate_lane_claims(
    claims: &[LaneClaim],
    shared_path_denylist: &[String],
) -> Result<LaneGuardReport, ClaimPatternError> {
    let shared = compile_patterns(shared_path_denylist)?;
    let mut report = LaneGuardReport::default();
    let mut owners: BTreeMap<ClaimedPath, BTreeSet<String>> = BTreeMap::new();

    for claim in claims {
        let permitted = compile_patterns(&claim.permitted_paths)?;
        let badges = report.lanes.entry(claim.lane_key.clone()).or_default();
        for path in &claim.touched_paths {
            if !permitted.is_match(&path.path) {
                badges.insert(LaneGuardBadge::OutsideClaim);
            }
            if shared.is_match(&path.path) {
                badges.insert(LaneGuardBadge::SharedPath);
            }
            owners
                .entry(path.clone())
                .or_default()
                .insert(claim.lane_key.clone());
        }
    }

    for (path, lane_keys) in owners {
        if lane_keys.len() < 2 {
            continue;
        }
        report.overlaps.insert(path);
        for lane_key in lane_keys {
            report
                .lanes
                .entry(lane_key)
                .or_default()
                .insert(LaneGuardBadge::Overlap);
        }
    }
    Ok(report)
}
