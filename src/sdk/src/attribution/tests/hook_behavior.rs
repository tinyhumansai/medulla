//! Exercises generated hook shims and attribution environment behavior.

use std::collections::HashMap;

use super::super::attribution_env;

// ---------------------------------------------------------------------------
// prepare_commit_msg hook generator tests
// ---------------------------------------------------------------------------

/// On Unix, `hook_env` returns env vars carrying the attribution trailer
/// and the `core.hooksPath` git-config overrides.
#[cfg(unix)]
#[test]
fn generate_hook_returns_env_vars() {
    let env = super::prepare_commit_msg::hook_env(
        "Co-authored-by: Medulla <medulla@tinyhumans.ai>",
        &HashMap::new(),
    );
    assert_eq!(
        env.get("MEDULLA_ATTRIBUTION"),
        Some(&"Co-authored-by: Medulla <medulla@tinyhumans.ai>".to_string()),
    );
    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"1".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0"),
        Some(&"core.hooksPath".to_string())
    );
    assert!(
        env.contains_key("GIT_CONFIG_VALUE_0"),
        "hooksPath must be set"
    );
}

/// Every client-side hook name must exist and be executable, so redirecting
/// `core.hooksPath` cannot disable the repository's own hooks.
#[cfg(unix)]
#[test]
fn every_client_hook_is_shimmed_and_executable() {
    use std::os::unix::fs::PermissionsExt;

    let hook_dir = super::prepare_commit_msg::generate_hook_dir();
    for name in super::prepare_commit_msg::CLIENT_HOOKS {
        let hook_path = hook_dir.join(name);
        assert!(hook_path.exists(), "{name} shim must exist: {hook_path:?}");
        let perms = std::fs::metadata(&hook_path)
            .expect("hook metadata")
            .permissions();
        assert_eq!(perms.mode() & 0o111, 0o111, "{name} must be executable");
    }
}

// ---------------------------------------------------------------------------
// End-to-end hook behaviour, driven through a real `git commit`
//
// These run git itself rather than invoking the script directly: the hook
// resolves the repository's own hooks directory and calls
// `git interpret-trailers`, so only a real repository exercises it honestly.
// ---------------------------------------------------------------------------

/// A throwaway git repository plus the generated hook directory, wired together
/// the way a Medulla-launched harness sees them.
#[cfg(unix)]
struct HookRepo {
    _tmp: tempfile::TempDir,
    repo: std::path::PathBuf,
    hook_dir: std::path::PathBuf,
    trailer: String,
}

#[cfg(unix)]
impl HookRepo {
    fn new(trailer: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir for repo");
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let hook_dir = super::prepare_commit_msg::generate_hook_dir();

        let this = Self {
            _tmp: tmp,
            repo,
            hook_dir,
            trailer: trailer.to_string(),
        };
        this.git(&["init", "-q", "."]);
        this.git(&["config", "user.email", "t@example.com"]);
        this.git(&["config", "user.name", "T"]);
        this
    }

    /// Run git in the repo with the attribution env wired in, as the harness
    /// child would see it. Returns stdout.
    fn git(&self, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .env("MEDULLA_ATTRIBUTION", &self.trailer)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", &self.hook_dir)
            .env(
                "GIT_CONFIG_PARAMETERS",
                format!("'core.hooksPath={}'", self.hook_dir.display()),
            )
            .env("MEDULLA_GIT_CONFIG_BASE_PARAMETERS", "")
            .output()
            .expect("git invocation");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Stage a new file and commit it with `message`.
    fn commit(&self, name: &str, message: &str) {
        std::fs::write(self.repo.join(name), name).unwrap();
        self.git(&["add", name]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// The full message of `HEAD`.
    fn head_message(&self) -> String {
        self.git(&["log", "-1", "--format=%B"])
    }
}

/// The regression this module exists for: a commit made with an explicit
/// message still carries the trailer, because the hook adds it rather than
/// asking the model to.
#[cfg(unix)]
#[test]
fn commit_with_an_explicit_message_carries_the_trailer() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);
    repo.commit("a.txt", "an explicit subject");

    let message = repo.head_message();
    assert!(
        message.starts_with("an explicit subject"),
        "author's message preserved: {message:?}"
    );
    assert!(message.contains(trailer), "trailer added: {message:?}");
}

/// Amending replays a message that already carries the trailer. It must not
/// gain a second copy — one co-author, not two.
#[cfg(unix)]
#[test]
fn amend_does_not_duplicate_the_trailer() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);
    repo.commit("a.txt", "subject");
    repo.git(&["commit", "-q", "--amend", "--no-edit"]);

    let message = repo.head_message();
    assert_eq!(
        message.matches(trailer).count(),
        1,
        "exactly one trailer after amend: {message:?}"
    );
}

/// The trailer must join the existing trailer block rather than starting a new
/// one — a blank line between blocks hides the earlier trailers from GitHub,
/// which would drop a provider's own co-author line (Codex hardcodes one).
#[cfg(unix)]
#[test]
fn trailer_joins_an_existing_trailer_block() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);
    let existing = "Co-authored-by: Codex <noreply@openai.com>";
    repo.commit("a.txt", &format!("subject\n\nbody\n\n{existing}"));

    let message = repo.head_message();
    assert!(message.contains(existing), "existing trailer kept");
    assert!(message.contains(trailer), "medulla trailer added");

    let existing_line = message.lines().position(|l| l == existing).unwrap();
    let medulla_line = message.lines().position(|l| l == trailer).unwrap();
    assert_eq!(
        medulla_line,
        existing_line + 1,
        "trailers must be adjacent, no blank line between: {message:?}"
    );
}

/// `core.hooksPath` redirects every hook, so the generated hook must chain to
/// whatever `prepare-commit-msg` the repository already had rather than
/// silently disabling it.
#[cfg(unix)]
#[test]
fn repository_own_hook_still_runs() {
    use std::os::unix::fs::PermissionsExt;

    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);

    let own = repo.repo.join(".git/hooks/prepare-commit-msg");
    std::fs::write(&own, "#!/bin/sh\nprintf 'repo-hook-ran\\n' >> \"$1\"\n").unwrap();
    std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o755)).unwrap();

    repo.commit("a.txt", "subject");

    let message = repo.head_message();
    assert!(
        message.contains("repo-hook-ran"),
        "repo's own hook must still run: {message:?}"
    );
    assert!(
        message.contains(trailer),
        "trailer still added: {message:?}"
    );
}

/// A repository hook that is *not* `prepare-commit-msg` must still run.
/// `core.hooksPath` redirects every hook, so shimming only the one we care
/// about would silently disable a repo's lint and validation gates.
#[cfg(unix)]
#[test]
fn other_repository_hooks_still_run() {
    use std::os::unix::fs::PermissionsExt;

    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);
    let marker = repo.repo.join("pre-commit-ran");

    let own = repo.repo.join(".git/hooks/pre-commit");
    std::fs::write(
        &own,
        format!("#!/bin/sh\ntouch {}\n", marker.to_string_lossy()),
    )
    .unwrap();
    std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o755)).unwrap();

    repo.commit("a.txt", "subject");

    assert!(
        marker.exists(),
        "the repository's own pre-commit hook must still run"
    );
}

/// A failing repository hook must still block the commit — the shim propagates
/// its exit code rather than swallowing it.
#[cfg(unix)]
#[test]
fn a_failing_repository_hook_still_blocks_the_commit() {
    use std::os::unix::fs::PermissionsExt;

    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);

    let own = repo.repo.join(".git/hooks/pre-commit");
    std::fs::write(&own, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o755)).unwrap();

    std::fs::write(repo.repo.join("a.txt"), "x").unwrap();
    repo.git(&["add", "a.txt"]);
    let output = std::process::Command::new("git")
        .args(["commit", "-m", "should be rejected"])
        .current_dir(&repo.repo)
        .env("MEDULLA_ATTRIBUTION", trailer)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", &repo.hook_dir)
        .env(
            "GIT_CONFIG_PARAMETERS",
            format!("'core.hooksPath={}'", repo.hook_dir.display()),
        )
        .env("MEDULLA_GIT_CONFIG_BASE_PARAMETERS", "")
        .output()
        .expect("git commit");

    assert!(
        !output.status.success(),
        "a failing pre-commit must reject the commit"
    );
}

/// A repository whose `core.hooksPath` is written with a tilde must still have
/// its hooks run. Git expands `~/` natively, but `git config --get` returns the
/// literal string — so the shim has to ask for the *path* form.
#[cfg(unix)]
#[test]
fn a_tilde_in_the_repository_hooks_path_is_expanded() {
    use std::os::unix::fs::PermissionsExt;

    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);

    // Point the repo at a hooks dir under a HOME we control, spelled with `~`.
    let home = repo._tmp.path().join("home");
    let hooks = home.join("tilde-hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let marker = repo.repo.join("tilde-hook-ran");
    let hook = hooks.join("pre-commit");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch {}\n", marker.to_string_lossy()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["config", "core.hooksPath", "~/tilde-hooks"]);

    std::fs::write(repo.repo.join("a.txt"), "x").unwrap();
    let output = std::process::Command::new("git")
        .args(["add", "a.txt"])
        .current_dir(&repo.repo)
        .env("HOME", &home)
        .output()
        .expect("git add");
    assert!(output.status.success());

    let output = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "subject"])
        .current_dir(&repo.repo)
        .env("HOME", &home)
        .env("MEDULLA_ATTRIBUTION", trailer)
        .env("MEDULLA_GIT_CONFIG_BASE_COUNT", "0")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", &repo.hook_dir)
        .env(
            "GIT_CONFIG_PARAMETERS",
            format!("'core.hooksPath={}'", repo.hook_dir.display()),
        )
        .env("MEDULLA_GIT_CONFIG_BASE_PARAMETERS", "")
        .output()
        .expect("git commit");
    assert!(
        output.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        marker.exists(),
        "a `~/`-spelled core.hooksPath must still have its hooks run"
    );
}

/// A merge message is composed by git over commits the harness did not author,
/// so it must not be attributed to Medulla.
#[cfg(unix)]
#[test]
fn merge_commits_are_not_attributed() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);
    repo.commit("base.txt", "base");
    let main = repo
        .git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string();

    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.commit("side.txt", "side change");
    repo.git(&["checkout", "-q", &main]);
    repo.commit("main.txt", "main change");
    repo.git(&["merge", "--no-ff", "-q", "-m", "merge side", "side"]);

    let message = repo.head_message();
    assert!(
        message.contains("merge side"),
        "merge happened: {message:?}"
    );
    assert!(
        !message.contains(trailer),
        "merge commit must not be attributed: {message:?}"
    );
}

/// When `MEDULLA_ATTRIBUTION` is unset the hook is inert, so clearing that one
/// variable disables attribution without unwinding the git-config injection.
#[cfg(unix)]
#[test]
fn hook_is_noop_when_attribution_env_is_empty() {
    let trailer = "Co-authored-by: Test <test@example.com>";
    let repo = HookRepo::new(trailer);

    std::fs::write(repo.repo.join("a.txt"), "x").unwrap();
    repo.git(&["add", "a.txt"]);
    let output = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "unattributed"])
        .current_dir(&repo.repo)
        .env_remove("MEDULLA_ATTRIBUTION")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", &repo.hook_dir)
        .output()
        .expect("git commit");
    assert!(
        output.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let message = repo.head_message();
    assert!(
        !message.contains(trailer),
        "no trailer without MEDULLA_ATTRIBUTION: {message:?}"
    );
}

/// Git hooks are not supported on non-Unix, so the hook env is empty there.
#[cfg(not(unix))]
#[test]
fn non_unix_returns_empty() {
    assert!(super::prepare_commit_msg::hook_env("test", &HashMap::new()).is_empty());
}

// ---------------------------------------------------------------------------
// attribution_env tests
// ---------------------------------------------------------------------------

/// The hook env is provider-independent — it is the mechanism of record for
/// every provider, including Claude, whose own `attribution.commit` setting is
/// advisory (the model can ignore it).
#[cfg(unix)]
#[test]
fn enabled_attribution_yields_hook_env() {
    let env = attribution_env(true, &HashMap::new());
    assert!(
        env.contains_key("MEDULLA_ATTRIBUTION"),
        "missing MEDULLA_ATTRIBUTION"
    );
    assert!(env.contains_key("GIT_CONFIG_VALUE_0"), "missing hooksPath");
}

/// Turning attribution off in config suppresses the hook env entirely.
#[test]
fn disabled_attribution_yields_no_hook_env() {
    assert!(attribution_env(false, &HashMap::new()).is_empty());
}

/// `cleanup_hook_dir` must remove a directory the caller owns.
#[cfg(unix)]
#[test]
fn cleanup_hook_dir_removes_the_directory() {
    let root = tempfile::tempdir().unwrap();
    let hook_dir = root.path().join("caller-owned-hooks");
    std::fs::create_dir(&hook_dir).unwrap();
    std::fs::write(hook_dir.join("shim"), "test").unwrap();
    assert!(hook_dir.exists(), "hook dir must exist before cleanup");

    super::prepare_commit_msg::cleanup_hook_dir(&hook_dir);
    assert!(!hook_dir.exists(), "hook dir must be removed after cleanup");
}

/// Cleanup of an empty path is a no-op, not a panic.
#[test]
fn cleanup_of_an_empty_path_is_a_noop() {
    super::prepare_commit_msg::cleanup_hook_dir(std::path::Path::new(""));
}

/// The process-wide hook directory is shared, not regenerated per call — this
/// is what makes concurrent spawns safe, since no caller can pull the directory
/// out from under another's still-running child.
#[cfg(unix)]
#[test]
fn hook_env_shares_one_directory_across_calls() {
    let first = super::prepare_commit_msg::hook_env("Co-authored-by: A <a@e.g>", &HashMap::new());
    let second = super::prepare_commit_msg::hook_env("Co-authored-by: B <b@e.g>", &HashMap::new());
    assert_eq!(
        first.get("GIT_CONFIG_VALUE_0"),
        second.get("GIT_CONFIG_VALUE_0"),
        "hook dir must be shared"
    );
    // The trailer is per-call even though the directory is not.
    assert_ne!(
        first.get("MEDULLA_ATTRIBUTION"),
        second.get("MEDULLA_ATTRIBUTION")
    );
    assert!(std::path::Path::new(first["GIT_CONFIG_VALUE_0"].as_str()).exists());
}
