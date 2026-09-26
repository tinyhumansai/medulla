//! Containment rules. Every case is a real filesystem so the symlink tests
//! exercise the same resolution the tools do.

use super::{contained, PathError};

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("inside.txt"), "hi").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    dir
}

#[test]
fn a_relative_path_inside_the_workspace_resolves() {
    let dir = workspace();
    let got = contained(dir.path(), "inside.txt").unwrap();
    assert!(got.ends_with("inside.txt"));
}

#[test]
fn a_file_that_does_not_exist_yet_resolves_via_its_parent() {
    // The create case: canonicalizing the target itself would fail on exactly
    // the call `write_file` makes most.
    let dir = workspace();
    let got = contained(dir.path(), "sub/new.txt").unwrap();
    assert!(got.ends_with("sub/new.txt"));
}

#[test]
fn a_dotdot_escape_is_refused() {
    let dir = workspace();
    assert!(matches!(
        contained(dir.path(), "../outside.txt"),
        Err(PathError::Escapes(_))
    ));
}

#[test]
fn an_absolute_path_outside_the_workspace_is_refused() {
    let dir = workspace();
    assert!(matches!(
        contained(dir.path(), "/etc/passwd"),
        Err(PathError::Escapes(_))
    ));
}

/// The case a textual `..` check misses entirely: an ordinary-looking relative
/// path that leaves the checkout because a symlink inside it points out.
#[cfg(unix)]
#[test]
fn a_symlink_pointing_out_of_the_workspace_is_refused() {
    let dir = workspace();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();

    assert!(matches!(
        contained(dir.path(), "link/secret.txt"),
        Err(PathError::Escapes(_))
    ));
}

/// The case the resolvable-symlink test above misses entirely: a link whose
/// target does *not* exist. `canonicalize` fails on it exactly as it does on a
/// path yet to be created, so walking past it and re-attaching its name yields a
/// contained-looking path — and `write_file`'s open then follows the link and
/// writes outside the checkout.
#[cfg(unix)]
#[test]
fn a_dangling_symlink_pointing_out_of_the_workspace_is_refused() {
    let dir = workspace();
    let outside = tempfile::tempdir().unwrap();
    // Deliberately not created: the target must not exist.
    let target = outside.path().join("not-created-yet.txt");
    std::os::unix::fs::symlink(&target, dir.path().join("dangling")).unwrap();

    assert!(
        contained(dir.path(), "dangling").is_err(),
        "a dangling link out of the workspace must not resolve as containable"
    );
    assert!(
        !target.exists(),
        "the guard must not have created the external target"
    );
}

/// The same trap one level down: the dangling link is a *directory* component
/// of the requested path, so the write lands inside whatever it points at.
#[cfg(unix)]
#[test]
fn a_dangling_symlink_as_a_parent_component_is_refused() {
    let dir = workspace();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path().join("nowhere"), dir.path().join("linkdir")).unwrap();

    assert!(
        contained(dir.path(), "linkdir/inside.txt").is_err(),
        "a path through a dangling link must not resolve as containable"
    );
}

/// A symlink that stays inside is fine — containment is about where the path
/// lands, not about whether a link was involved.
#[cfg(unix)]
#[test]
fn a_symlink_staying_inside_the_workspace_resolves() {
    let dir = workspace();
    std::os::unix::fs::symlink(dir.path().join("inside.txt"), dir.path().join("alias")).unwrap();
    assert!(contained(dir.path(), "alias").is_ok());
}

/// `write_file` creates parent directories, so a path whose ancestors do not
/// exist yet is the ordinary create case, not an error. Resolution walks up to
/// the deepest existing ancestor — here the checkout root itself.
#[test]
fn a_path_whose_ancestors_are_missing_resolves_against_the_deepest_that_exists() {
    let dir = workspace();
    let got = contained(dir.path(), "no/such/dir/file.txt").expect("a nested create resolves");
    assert!(got.ends_with("no/such/dir/file.txt"), "{got:?}");
    assert!(got.starts_with(dir.path().canonicalize().unwrap()));
}

/// The components below the resolved ancestor were never canonicalized, so a
/// `..` among them could still climb out of the directory whose containment was
/// actually verified. Refused rather than normalised.
#[test]
fn a_dotdot_below_a_missing_ancestor_is_refused() {
    let dir = workspace();
    assert!(
        contained(dir.path(), "missing/../../escape.txt").is_err(),
        "a climb through an unresolved segment must not be accepted"
    );
}

/// The escape check still applies to the resolved ancestor: a nested create
/// under an absolute path outside the checkout is refused, not created.
#[test]
fn a_nested_create_outside_the_workspace_is_still_refused() {
    let dir = workspace();
    assert!(matches!(
        contained(dir.path(), "/tmp/definitely/not/here.txt"),
        Err(PathError::Escapes(_))
    ));
}
