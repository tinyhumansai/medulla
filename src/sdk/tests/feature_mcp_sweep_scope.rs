#![cfg(unix)]

//! `sweep_stale_config_files` never touching a sibling control plane's files
//! — the fix for the P2 Codex found on `pty-mcp-attach-security-fixup`
//! (medulla#177): two Medulla processes can legitimately share one account
//! home while bound to two different sockets (`control_plane::start` in the
//! `medulla-tui` crate already documents this as a real, supported case), and
//! an account-wide sweep would let one instance restarting delete a file the
//! other still needs.
//!
//! A real `ActiveControlPlane` has to be installed to reach `write_config_file`
//! at all (it only writes when a fleet grant was minted, which needs one), so
//! this lives in its own test binary — same reasoning as
//! `feature_mcp_attach_override.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use medulla::control_socket::{ActiveControlPlane, GrantRegistry};
use medulla::mcp::attach_cli;
use medulla::protocol::HarnessProvider;

fn scratch_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a scratch home");
    std::env::set_var("MEDULLA_HOME", dir.path());
    dir
}

#[test]
fn sweeping_this_instance_never_touches_a_sibling_sockets_directory() {
    let _home = scratch_home();
    medulla::control_socket::install(ActiveControlPlane {
        socket: PathBuf::from("/run/medulla-instance-a.sock"),
        grants: GrantRegistry::new(),
        max_depth: 2,
        max_in_flight: 4,
    });

    // A real file this instance's own grant wrote, standing in for one a
    // previous, crashed run of *this same* socket left behind.
    let mut env = HashMap::new();
    let mut args = Vec::new();
    attach_cli(
        HarnessProvider::Claude,
        "claude",
        "instance-a-orphan",
        &mut env,
        &mut args,
        None,
    )
    .expect("a grant is minted against the plane just installed");
    let flag = args
        .iter()
        .position(|arg| arg == "--mcp-config")
        .expect("claude is registered through --mcp-config");
    let own_file = PathBuf::from(&args[flag + 1]);
    assert!(own_file.exists(), "this instance's own file must exist");

    // A sibling instance's file: same account, a socket this process never
    // bound, sitting in a differently-keyed subdirectory of the same `mcp`
    // root. Placed by hand since `control_socket::install` — a `OnceLock` —
    // refuses a second call in this process, which is exactly why a second
    // real Medulla process is the only way this collision happens for real.
    let mcp_root = own_file
        .parent()
        .and_then(|instance_dir| instance_dir.parent())
        .expect("the file lives two levels under the mcp root");
    let sibling_dir = mcp_root.join("sibling-instance-b");
    std::fs::create_dir_all(&sibling_dir).expect("the sibling directory is creatable");
    let sibling_file = sibling_dir.join("instance-b-live-session.json");
    std::fs::write(&sibling_file, "{}").expect("the sibling's file is writable");

    // Named explicitly rather than read from the installed plane: for real
    // this runs before the plane is published, which is the whole reason the
    // sweep takes a socket at all.
    medulla::mcp::sweep_stale_config_files(Path::new("/run/medulla-instance-a.sock"));

    assert!(
        !own_file.exists(),
        "this instance's own orphaned file must be swept: {}",
        own_file.display()
    );
    assert!(
        sibling_file.exists(),
        "a sibling instance's still-live file must survive this instance's own sweep: {}",
        sibling_file.display()
    );

    // Clean up what this test left behind rather than the grant registry's
    // own end-of-process teardown.
    let _ = std::fs::remove_file(&sibling_file);
    let _ = std::fs::remove_dir(&sibling_dir);
}
