//! `seed_hook_grant` with no control plane bound — the ordinary case for this
//! crate's own test process, which never installs one (see
//! `mcp::attach`'s own unit tests, which deliberately never install a plane
//! either, and the dedicated `feature_mcp_attach_override` binary that does).

use super::*;

#[test]
fn seeds_nothing_and_revokes_nothing_without_a_bound_control_plane() {
    let mut env = std::collections::HashMap::new();
    let guard = seed_hook_grant("grant-test-session", &mut env);
    assert!(
        env.is_empty(),
        "no control plane is bound in this test process, so nothing should be seeded: {env:?}"
    );
    // Dropping must not panic even though nothing was minted.
    drop(guard);
}
