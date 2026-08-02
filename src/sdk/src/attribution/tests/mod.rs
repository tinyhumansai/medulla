//! Unit tests for attribution policy and generated Git hook behavior.

mod hook_behavior;
#[cfg(unix)]
mod legacy_git;
mod policy;

use super::{
    attribution_args, attribution_trailer, prepare_commit_msg, ATTRIBUTION_EMAIL, ATTRIBUTION_NAME,
};
use crate::config::AttributionConfig;
use crate::tinyplace::HarnessProvider;
