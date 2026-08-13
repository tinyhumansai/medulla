//! Unit tests for the loopback module's process-spawning helpers.


/// The browser opener must not inherit our stderr. `xdg-open` delegates to GIO,
/// which prints warnings ("The peer-to-peer connection failed: ... gvfsd ...")
/// that would otherwise be painted straight onto the TUI's frame. Assert the
/// child sees `/dev/null` on stdout and stderr rather than our descriptors.
#[cfg(target_os = "linux")]
#[test]
fn spawn_detached_gives_the_child_null_stdio() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("fds.txt");
    // Duplicate the inherited descriptors before redirecting stdout at the
    // report file, so the readlinks report what `spawn_detached` handed us.
    let script = "exec 3>&1 4>&2; { readlink /proc/self/fd/3; readlink /proc/self/fd/4; } > \"$1\"";

    let mut cmd = Command::new("sh");
    cmd.args([
        "-c",
        script,
        "sh",
        report.to_str().expect("UTF-8 tempdir path"),
    ]);
    super::spawn_detached(&mut cmd);

    // The child is reaped on a detached thread, so poll for its output.
    let contents = wait_for_file(&report);
    let mut lines = contents.lines();
    assert_eq!(lines.next(), Some("/dev/null"), "child stdout must be null");
    assert_eq!(lines.next(), Some("/dev/null"), "child stderr must be null");
}

/// A missing browser opener is a best-effort failure: it must not interrupt
/// the login flow or cause the caller to panic.
#[cfg(target_os = "linux")]
#[test]
fn spawn_detached_ignores_spawn_failure() {
    let mut cmd = Command::new("/definitely-not-an-executable");

    super::spawn_detached(&mut cmd);
}

/// Poll `path` until the detached child has written it, failing the test rather
/// than hanging if the child never produces output.
#[cfg(target_os = "linux")]
fn wait_for_file(path: &std::path::Path) -> String {
    for _ in 0..200 {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if contents.lines().count() >= 2 {
                return contents;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("detached child never wrote {}", path.display());
}
